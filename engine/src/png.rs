//! Minimal PNG (RFC 2083) writer: 8-bit RGBA, no interlace, one `IDAT`.
//!
//! Every row is run through the PNG filter heuristic (the libpng "minimum sum
//! of absolute differences" rule) before being handed to the DEFLATE writer, so
//! flat screenshot regions turn into runs of zero bytes and compress hard.

use crate::deflate::{crc32, zlib_compress};
use crate::image::RgbaImage;

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
const BIT_DEPTH: u8 = 8;
const COLOR_TYPE_RGBA: u8 = 6;

/// Encodes `image` as a PNG byte stream.
pub fn encode(image: &RgbaImage) -> Vec<u8> {
    let bpp = 4usize;
    let stride = image.width as usize * bpp;
    let mut raw = Vec::with_capacity((stride + 1) * image.height as usize);
    let mut prior = vec![0u8; stride];
    let mut candidate = vec![0u8; stride];
    let mut best = vec![0u8; stride];

    for row in 0..image.height as usize {
        let line = &image.data[row * stride..(row + 1) * stride];
        let mut best_kind = 0u8;
        let mut best_score = u64::MAX;
        for kind in [0u8, 1, 2, 4] {
            filter_row(kind, bpp, line, &prior, &mut candidate);
            let score: u64 = candidate
                .iter()
                .map(|byte| {
                    let signed = *byte as i8 as i16;
                    signed.unsigned_abs() as u64
                })
                .sum();
            if score < best_score {
                best_score = score;
                best_kind = kind;
                best.copy_from_slice(&candidate);
            }
        }
        raw.push(best_kind);
        raw.extend_from_slice(&best);
        prior.copy_from_slice(line);
    }

    let mut out = Vec::with_capacity(raw.len() / 2 + 128);
    out.extend_from_slice(&SIGNATURE);
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&image.width.to_be_bytes());
    ihdr.extend_from_slice(&image.height.to_be_bytes());
    ihdr.push(BIT_DEPTH);
    ihdr.push(COLOR_TYPE_RGBA);
    ihdr.push(0); // deflate
    ihdr.push(0); // adaptive filtering
    ihdr.push(0); // no interlace
    write_chunk(&mut out, b"IHDR", &ihdr);
    write_chunk(&mut out, b"IDAT", &zlib_compress(&raw));
    write_chunk(&mut out, b"IEND", &[]);
    out
}

/// Applies PNG filter `kind` to `line` (with `prior` as the row above) into
/// `out`.
fn filter_row(kind: u8, bpp: usize, line: &[u8], prior: &[u8], out: &mut [u8]) {
    for index in 0..line.len() {
        let left = if index >= bpp { line[index - bpp] as i16 } else { 0 };
        let above = prior[index] as i16;
        let upper_left = if index >= bpp { prior[index - bpp] as i16 } else { 0 };
        let value = line[index] as i16;
        let predicted = match kind {
            1 => left,                                  // Sub
            2 => above,                                 // Up
            4 => paeth(left, above, upper_left),        // Paeth
            _ => 0,                                     // None
        };
        out[index] = (value - predicted).rem_euclid(256) as u8;
    }
}

/// The PNG Paeth predictor.
fn paeth(a: i16, b: i16, c: i16) -> i16 {
    let p = a + b - c;
    let pa = (p - a).abs();
    let pb = (p - b).abs();
    let pc = (p - c).abs();
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

/// Writes one length/type/data/CRC chunk.
fn write_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let mut crc_input = Vec::with_capacity(4 + data.len());
    crc_input.extend_from_slice(kind);
    crc_input.extend_from_slice(data);
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::RgbaImage;

    #[test]
    fn writes_a_decodable_png() {
        let mut image = RgbaImage::new(9, 5).expect("image");
        for y in 0..5 {
            for x in 0..9 {
                image.set(x, y, [(x * 28) as u8, (y * 51) as u8, 128, 255]);
            }
        }
        let png_bytes = encode(&image);
        assert_eq!(&png_bytes[..8], &SIGNATURE);
        let decoder = png::Decoder::new(png_bytes.as_slice());
        let mut reader = decoder.read_info().expect("png header");
        let mut buffer = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buffer).expect("png data");
        assert_eq!((info.width, info.height), (9, 5));
        assert_eq!(info.color_type, png::ColorType::Rgba);
        assert_eq!(&buffer[..info.buffer_size()], &image.data[..]);
    }

    #[test]
    fn flat_images_compress_small() {
        let image = RgbaImage::filled(320, 200, [255, 255, 255, 255]).expect("image");
        let png_bytes = encode(&image);
        assert!(
            png_bytes.len() < image.data.len() / 50,
            "flat 320x200 PNG is {} bytes",
            png_bytes.len()
        );
    }

    #[test]
    fn every_chunk_carries_a_valid_crc() {
        let image = RgbaImage::filled(4, 4, [1, 2, 3, 4]).expect("image");
        let png_bytes = encode(&image);
        let mut at = 8usize;
        let mut seen = Vec::new();
        while at + 12 <= png_bytes.len() {
            let len = u32::from_be_bytes(png_bytes[at..at + 4].try_into().unwrap()) as usize;
            let kind = &png_bytes[at + 4..at + 8];
            let crc_at = at + 8 + len;
            let stored = u32::from_be_bytes(png_bytes[crc_at..crc_at + 4].try_into().unwrap());
            assert_eq!(crc32(&png_bytes[at + 4..crc_at]), stored, "CRC for {:?}", kind);
            seen.push(String::from_utf8_lossy(kind).to_string());
            at = crc_at + 4;
        }
        assert_eq!(seen, vec!["IHDR", "IDAT", "IEND"]);
        assert_eq!(at, png_bytes.len(), "no trailing bytes");
    }
}