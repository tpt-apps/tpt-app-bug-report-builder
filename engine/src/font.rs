//! A built-in 5x7 bitmap font.
//!
//! The engine rasterises text annotations itself so a report can be assembled
//! entirely offline (no browser, no font files, no allocator-heavy glyph
//! shaping). The glyph source lives in `font5x7.txt` — one entry per
//! character: a decimal ASCII code line followed by seven 5-character rows
//! where `#` is ink and everything else is blank.
//!
//! A 5x7 cell is small but covers the printable ASCII range, which is all a
//! short annotation label needs; anything outside it falls back to `?`.

use std::sync::OnceLock;

/// Glyph cell width in font units.
pub const GLYPH_W: u32 = 5;
/// Glyph cell height in font units (the row count in the data file).
pub const GLYPH_H: u32 = 7;

/// Number of rows of art per glyph.
const ROWS: usize = GLYPH_H as usize;

const FONT_DATA: &str = include_str!("font5x7.txt");

/// Parses the font file into `(ascii code, rows)` pairs.
///
/// Blank lines are separators and are ignored, so the data file can group
/// glyphs readably.
fn parse() -> Vec<(u8, [u8; ROWS])> {
    let mut glyphs = Vec::new();
    let mut lines = FONT_DATA.lines().filter(|line| !line.trim().is_empty());
    while let Some(code_line) = lines.next() {
        let code: u8 = code_line
            .trim()
            .parse()
            .unwrap_or_else(|_| panic!("font5x7: `{code_line}` is not an ASCII code"));
        let mut rows = [0u8; ROWS];
        for (row, slot) in rows.iter_mut().enumerate() {
            let art = match lines.next() {
                Some(line) => line,
                None => panic!("font5x7: character {code} ends after {row} rows"),
            };
            assert_eq!(
                art.chars().count(),
                GLYPH_W as usize,
                "font5x7: `{art}` for character {code} is not {GLYPH_W} columns"
            );
            let mut bits = 0u8;
            for (col, ch) in art.chars().enumerate() {
                if ch == '#' {
                    bits |= 1 << (GLYPH_W - 1 - col as u32);
                }
            }
            *slot = bits;
        }
        glyphs.push((code, rows));
    }
    glyphs
}

/// `code -> rows` lookup, built once on first use.
fn table() -> &'static [Option<[u8; ROWS]>; 128] {
    static TABLE: OnceLock<[Option<[u8; ROWS]>; 128]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table: [Option<[u8; ROWS]>; 128] = [None; 128];
        for (code, rows) in parse() {
            table[code as usize] = Some(rows);
        }
        table
    })
}

/// The 7 row bitmaps for `c`, most-significant bit leftmost. Characters the
/// font does not cover render as `?`.
pub fn glyph(c: char) -> [u8; ROWS] {
    let table = table();
    let code = c as u32;
    if code < 128 {
        if let Some(rows) = table[code as usize] {
            return rows;
        }
    }
    table[b'?' as usize].unwrap_or([0x0E, 0x11, 0x01, 0x02, 0x04, 0x00, 0x04])
}

/// True when the font has a glyph for `c` (so callers can fall back to a
/// transliteration instead of `?`).
pub fn has_glyph(c: char) -> bool {
    let code = c as u32;
    code < 128 && table()[code as usize].is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_glyph_parses_at_five_columns() {
        // `parse` asserts the column count on every row, so simply touching the
        // table validates the whole data file.
        let glyphs = parse();
        assert!(
            glyphs.len() >= 90,
            "expected the printable ASCII range, found {} glyphs",
            glyphs.len()
        );
        for (code, rows) in &glyphs {
            if *code != b' ' {
                assert!(
                    rows.iter().any(|row| *row != 0),
                    "character {code} is blank"
                );
            }
            assert!(rows.iter().all(|row| row >> GLYPH_W == 0), "code {code} overflows");
        }
    }

    #[test]
    fn covers_digits_letters_and_punctuation() {
        for ch in "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz.,:;!?-()/".chars()
        {
            assert!(has_glyph(ch), "missing glyph for {ch:?}");
        }
        assert_eq!(glyph(' '), [0; 7], "space is blank");
        assert!(!has_glyph('\u{4e2d}'), "CJK is out of font range");
        assert_eq!(glyph('\u{4e2d}'), glyph('?'), "unknown falls back to ?");
    }

    #[test]
    fn a_is_an_a_shape() {
        // Row 0 is the apex, row 3 is the crossbar, and both outer columns are
        // inked at the bottom — the minimum that makes an 'A'.
        let a = glyph('A');
        assert_eq!(a[0].count_ones(), 3, "apex row of A");
        assert_eq!(a[3], 0b11111, "crossbar of A");
        assert_eq!(a[6] & 0b10001, 0b10001, "both legs of A");
    }
}