//! A small, dependency-free DEFLATE (RFC 1951) + zlib (RFC 1950) writer.
//!
//! The report writers embed screenshots, so the engine needs a real compressor:
//! PNG `IDAT` and PDF `/FlateDecode` streams both take zlib-wrapped DEFLATE.
//! Rather than pull a compression crate into the WASM payload, this module
//! implements the one encoding mode a screenshot compressor actually needs —
//! **fixed Huffman codes with greedy LZ77 matching** — plus a stored-block
//! fallback for incompressible data.
//!
//! Correctness is pinned by the `flate2` dev-dependency: `tests/encoding.rs`
//! inflates every stream this module produces and compares it byte-for-byte
//! with the input, and the PNG encoder round-trips through the `png` crate.

use std::sync::OnceLock;

/// DEFLATE's maximum match length.
const MAX_MATCH: usize = 258;
/// DEFLATE's minimum match length.
const MIN_MATCH: usize = 3;
/// Sliding-window size.
const WINDOW: usize = 32_768;
const WINDOW_MASK: usize = WINDOW - 1;
/// Hash-chain buckets for the 3-byte match hash.
const HASH_BITS: usize = 15;
const HASH_SIZE: usize = 1 << HASH_BITS;
/// How many chain candidates to test before settling for the best so far —
/// bounds worst-case time on highly repetitive input.
const MAX_CHAIN: usize = 24;
const NIL: u32 = u32::MAX;

/// Match-length code bases (RFC 1951 §3.2.5).
const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
/// Extra match-length bits, index-aligned with [`LENGTH_BASE`].
const LENGTH_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
/// Distance code bases (RFC 1951 §3.2.5).
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
/// Extra distance bits, index-aligned with [`DIST_BASE`].
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

/// LSB-first bit sink. DEFLATE packs bits starting at the least significant bit
/// of each byte; Huffman codes are emitted most-significant-bit first, which
/// [`BitWriter::put_code`] handles.
struct BitWriter {
    out: Vec<u8>,
    buffer: u32,
    bits: u32,
}

impl BitWriter {
    fn new() -> Self {
        BitWriter {
            out: Vec::new(),
            buffer: 0,
            bits: 0,
        }
    }

    /// Appends `count` bits of `value`, least significant bit first.
    fn put_bits(&mut self, value: u32, count: u32) {
        if count == 0 {
            return;
        }
        debug_assert!(count <= 24, "bit runs stay inside the buffer");
        self.buffer |= (value & ((1u32 << count) - 1)) << self.bits;
        self.bits += count;
        while self.bits >= 8 {
            self.out.push((self.buffer & 0xFF) as u8);
            self.buffer >>= 8;
            self.bits -= 8;
        }
    }

    /// Appends a Huffman code, most significant bit first.
    fn put_code(&mut self, code: u32, count: u32) {
        if count == 0 {
            return;
        }
        let mut reversed = 0u32;
        for bit in 0..count {
            reversed = (reversed << 1) | ((code >> bit) & 1);
        }
        self.put_bits(reversed, count);
    }

    /// Pads the final partial byte with zero bits.
    fn finish(mut self) -> Vec<u8> {
        if self.bits > 0 {
            self.out.push((self.buffer & 0xFF) as u8);
        }
        self.out
    }
}

/// The fixed-Huffman literal/length code for `symbol`, and its bit length.
fn fixed_literal_code(symbol: u16) -> (u32, u32) {
    match symbol {
        0..=143 => (0x30 + symbol as u32, 8),
        144..=255 => (0x190 + (symbol as u32 - 144), 9),
        256..=279 => (symbol as u32 - 256, 7),
        _ => (0xC0 + (symbol as u32 - 280), 8),
    }
}

/// The length-code symbol and extra bits for a match length of `len`.
fn length_code(len: usize) -> (u16, u32, u32) {
    let mut index = 0;
    for i in 0..LENGTH_BASE.len() {
        if LENGTH_BASE[i] as usize <= len {
            index = i;
        } else {
            break;
        }
    }
    let extra = len - LENGTH_BASE[index] as usize;
    (257 + index as u16, extra as u32, LENGTH_EXTRA[index] as u32)
}

/// The distance-code symbol and extra bits for a match distance of `dist`.
fn distance_code(dist: usize) -> (u16, u32, u32) {
    let mut index = 0;
    for i in 0..DIST_BASE.len() {
        if DIST_BASE[i] as usize <= dist {
            index = i;
        } else {
            break;
        }
    }
    let extra = dist - DIST_BASE[index] as usize;
    (index as u16, extra as u32, DIST_EXTRA[index] as u32)
}

/// 3-byte rolling hash for the match finder.
fn hash3(data: &[u8], at: usize) -> usize {
    let a = data[at] as usize;
    let b = data[at + 1] as usize;
    let c = data[at + 2] as usize;
    ((a << 10) ^ (b << 5) ^ c) & (HASH_SIZE - 1)
}

/// Greedy LZ77 over a 32 KB sliding window, emitted as fixed Huffman codes.
///
/// Lazy matching (one position of lookahead) is enough to avoid the classic
/// greedy mistake of taking a length-3 match at `i` when a much longer one
/// starts at `i + 1`, which is what makes flat-colour screenshot regions
/// collapse to a handful of bytes.
pub fn deflate_fixed(data: &[u8]) -> Vec<u8> {
    let mut writer = BitWriter::new();
    writer.put_bits(1, 1); // BFINAL
    writer.put_bits(1, 2); // BTYPE = 01, fixed Huffman

    let emit_literal = |writer: &mut BitWriter, byte: u8| {
        let (code, bits) = fixed_literal_code(byte as u16);
        writer.put_code(code, bits);
    };

    let mut head = vec![NIL; HASH_SIZE];
    let mut prev = vec![NIL; WINDOW];
    let mut pos = 0usize;

    // Returns (best_length, best_distance) for matches starting at `at`.
    let find_match = |head: &[u32], prev: &[u32], at: usize| -> (usize, usize) {
        if at + MIN_MATCH > data.len() {
            return (0, 0);
        }
        let limit = MAX_MATCH.min(data.len() - at);
        let mut candidate = head[hash3(data, at)];
        let mut best_len = 0usize;
        let mut best_dist = 0usize;
        let mut depth = 0;
        while candidate != NIL && depth < MAX_CHAIN {
            let c = candidate as usize;
            let dist = at - c;
            if dist == 0 || dist > WINDOW {
                break;
            }
            let mut len = 0usize;
            while len < limit && data[c + len] == data[at + len] {
                len += 1;
            }
            if len > best_len {
                best_len = len;
                best_dist = dist;
                if len >= limit {
                    break;
                }
            }
            candidate = prev[c & WINDOW_MASK];
            depth += 1;
        }
        (best_len, best_dist)
    };

    while pos < data.len() {
        let (mut best_len, mut best_dist) = find_match(&head, &prev, pos);
        if best_len < MIN_MATCH {
            emit_literal(&mut writer, data[pos]);
            if pos + MIN_MATCH <= data.len() {
                let hash = hash3(data, pos);
                prev[pos & WINDOW_MASK] = head[hash];
                head[hash] = pos as u32;
            }
            pos += 1;
            continue;
        }

        // Lazy match: prefer a strictly longer match starting one byte later.
        if best_len < MAX_MATCH && pos + 1 + MIN_MATCH <= data.len() {
            // Register `pos` so the lookahead at `pos + 1` can chain back to it.
            let hash = hash3(data, pos);
            prev[pos & WINDOW_MASK] = head[hash];
            head[hash] = pos as u32;

            let (next_len, next_dist) = find_match(&head, &prev, pos + 1);
            if next_len > best_len {
                emit_literal(&mut writer, data[pos]);
                pos += 1;
                best_len = next_len;
                best_dist = next_dist;
                if best_len < MIN_MATCH {
                    continue;
                }
            }
        }

        // Emit the match.
        debug_assert!(best_len >= MIN_MATCH && best_dist >= 1);
        let (symbol, extra, extra_bits) = length_code(best_len);
        let (code, bits) = fixed_literal_code(symbol);
        writer.put_code(code, bits);
        writer.put_bits(extra, extra_bits);
        let (dsymbol, dextra, dextra_bits) = distance_code(best_dist);
        writer.put_code(dsymbol as u32, 5);
        writer.put_bits(dextra, dextra_bits);

        // Register every position the match covers so later matches can find it.
        for offset in 0..best_len {
            let at = pos + offset;
            if at + MIN_MATCH <= data.len() {
                let hash = hash3(data, at);
                prev[at & WINDOW_MASK] = head[hash];
                head[hash] = at as u32;
            }
        }
        pos += best_len;
    }

    // End of block.
    writer.put_code(0, 7);
    writer.finish()
}

/// Stored (uncompressed) DEFLATE blocks — the fallback for data that LZ77
/// cannot shrink (already-random noise, encrypted blobs).
pub fn deflate_stored(data: &[u8]) -> Vec<u8> {
    const MAX_BLOCK: usize = 65_535;
    let mut out = Vec::with_capacity(data.len() + data.len() / MAX_BLOCK * 5 + 5);
    if data.is_empty() {
        out.extend_from_slice(&[0x01, 0x00, 0x00, 0xFF, 0xFF]);
        return out;
    }
    let blocks = data.len().div_ceil(MAX_BLOCK);
    for (index, chunk) in data.chunks(MAX_BLOCK).enumerate() {
        let last = index + 1 == blocks;
        out.push(if last { 0x01 } else { 0x00 });
        let len = chunk.len() as u16;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(chunk);
    }
    out
}

/// zlib (RFC 1950) wrapper around whichever DEFLATE encoding is smaller —
/// what PNG `IDAT` and PDF `/FlateDecode` streams expect.
pub fn zlib_compress(data: &[u8]) -> Vec<u8> {
    // 0x78 0x01: 32 KB window, default compression level, no preset dictionary.
    let deflated = deflate_fixed(data);
    let stored = deflate_stored(data);
    let body = if stored.len() < deflated.len() {
        stored
    } else {
        deflated
    };
    let mut out = Vec::with_capacity(body.len() + 6);
    out.push(0x78);
    out.push(0x01);
    out.extend_from_slice(&body);
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

/// Adler-32 checksum (RFC 1950 §9) — the zlib trailer.
pub fn adler32(data: &[u8]) -> u32 {
    const BASE: u32 = 65_521;
    // 5552 is the largest block that cannot overflow a u32 accumulator.
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    for chunk in data.chunks(5552) {
        for &byte in chunk {
            a += byte as u32;
            b += a;
        }
        a %= BASE;
        b %= BASE;
    }
    (b << 16) | a
}

/// CRC-32 (the PNG polynomial, reflected) — the PNG chunk checksum.
pub fn crc32(data: &[u8]) -> u32 {
    static TABLE: OnceLock<[u32; 256]> = OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut table = [0u32; 256];
        for (index, slot) in table.iter_mut().enumerate() {
            let mut value = index as u32;
            for _ in 0..8 {
                value = if value & 1 == 1 {
                    0xEDB8_8320 ^ (value >> 1)
                } else {
                    value >> 1
                };
            }
            *slot = value;
        }
        table
    });
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc = table[((crc ^ byte as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::ZlibDecoder;
    use std::io::Read;

    fn inflate(compressed: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        ZlibDecoder::new(compressed)
            .read_to_end(&mut out)
            .expect("flate2 must inflate our zlib stream");
        out
    }

    /// Deterministic pseudo-random bytes (no rand dependency).
    fn noise(len: usize) -> Vec<u8> {
        let mut state = 0x1234_5678u32;
        (0..len)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (state >> 24) as u8
            })
            .collect()
    }

    #[test]
    fn round_trips_through_flate2() {
        let cases: Vec<Vec<u8>> = vec![
            Vec::new(),
            vec![0],
            b"short".to_vec(),
            b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_vec(),
            // Long flat runs with sporadic changes — the screenshot case.
            (0..50_000u32)
                .map(|i| if i % 997 == 0 { 7 } else { 0 })
                .collect(),
            b"the quick brown fox jumps over the lazy dog. ".repeat(400),
            noise(20_000),
            noise(40).iter().cycle().take(30_000).cloned().collect(),
        ];
        for case in cases {
            let compressed = zlib_compress(&case);
            assert_eq!(
                inflate(&compressed),
                case,
                "round-trip failed for a {}-byte input",
                case.len()
            );
        }
    }

    #[test]
    fn compresses_flat_screenshot_like_data() {
        // 200 x 200 x 4 = 160 KB of mostly-white pixels with a few dark rows.
        let mut image = vec![255u8; 200 * 200 * 4];
        for row in (0..200).step_by(10) {
            for x in 0..200 {
                let at = (row * 200 + x) * 4;
                image[at] = 0;
                image[at + 1] = 0;
                image[at + 2] = 0;
            }
        }
        let compressed = zlib_compress(&image);
        assert!(
            compressed.len() * 20 < image.len(),
            "expected a >20x win, got {} bytes from {}",
            compressed.len(),
            image.len()
        );
        assert_eq!(inflate(&compressed), image);
    }

    #[test]
    fn falls_back_to_stored_for_incompressible_input() {
        let data = noise(300_000);
        let compressed = zlib_compress(&data);
        // Stored blocks add at most 5 bytes per 64 KB block, plus the 6-byte
        // zlib header/trailer.
        let budget = data.len() + data.len().div_ceil(65_535) * 5 + 6;
        assert!(
            compressed.len() <= budget,
            "stored fallback exceeded {budget} bytes: {}",
            compressed.len()
        );
        assert_eq!(inflate(&compressed), data);
    }

    #[test]
    fn matches_the_published_checksum_vectors() {
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(adler32(b""), 1);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn length_and_distance_codes_cover_their_ranges() {
        assert_eq!(length_code(3), (257, 0, 0));
        assert_eq!(length_code(10), (264, 0, 0));
        assert_eq!(length_code(258), (285, 0, 0));
        assert_eq!(length_code(227), (284, 0, 5));
        assert_eq!(distance_code(1), (0, 0, 0));
        assert_eq!(distance_code(5), (4, 0, 1));
        assert_eq!(distance_code(24577), (29, 0, 13));
    }
}