//! Minimal base64 (RFC 4648) encoder.
//!
//! Markdown reports embed screenshots as `data:image/png;base64,…` so the
//! exported `.md` is a single self-contained file that a bug tracker will show
//! inline.

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Encodes `data` as standard (padded) base64.
pub fn encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[((triple >> 18) & 0x3F) as usize] as char);
        out.push(ALPHABET[((triple >> 12) & 0x3F) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[((triple >> 6) & 0x3F) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(triple & 0x3F) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// A self-contained `data:` URI for an encoded PNG.
pub fn png_data_uri(png_bytes: &[u8]) -> String {
    let mut uri = String::with_capacity(png_bytes.len() / 3 * 4 + 24);
    uri.push_str("data:image/png;base64,");
    uri.push_str(&encode(png_bytes));
    uri
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_the_rfc_4648_vectors() {
        for (input, expected) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
            ("hello world", "aGVsbG8gd29ybGQ="),
        ] {
            assert_eq!(encode(input.as_bytes()), expected, "input {input:?}");
        }
    }

    #[test]
    fn covers_all_byte_values() {
        let data: Vec<u8> = (0..=255u8).collect();
        let encoded = encode(&data);
        // 256 bytes = 85 triples + 1 leftover byte -> 4 chars per triple plus
        // one padded group.
        assert_eq!(encoded.len(), 344, "256 bytes -> 344 base64 chars");
        assert!(encoded.ends_with("=="), "one leftover byte needs two pads");
        assert!(encoded.starts_with("AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8g"));
    }

    #[test]
    fn builds_a_png_data_uri() {
        let uri = png_data_uri(&[0x89, b'P', b'N', b'G']);
        assert_eq!(uri, "data:image/png;base64,iVBORw==");
    }
}