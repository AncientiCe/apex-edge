//! Transcoding between UTF-8 receipt text and the printer's single-byte code page.
//!
//! Thermal printers are not UTF-8 devices. Send them `café` as UTF-8 and they print
//! `cafÃ©`, which is the single most common cosmetic bug in receipt printing. Both
//! encoders select code page 437 and route text through here first.
//!
//! CP437 has no euro sign, and printing `?3.50` where a price should be is worse than
//! printing `EUR3.50`, so a handful of characters degrade to letters instead of the
//! placeholder.

/// The Latin part of code page 437, which is all a receipt needs; the box-drawing and
/// maths halves are deliberately omitted so this table stays reviewable.
///
/// One table serves both directions, so the printer and `tools/virtual-printer` cannot
/// disagree about what a byte means.
const CP437_HIGH: &[(u8, char)] = &[
    (0x80, 'Ç'),
    (0x81, 'ü'),
    (0x82, 'é'),
    (0x83, 'â'),
    (0x84, 'ä'),
    (0x85, 'à'),
    (0x86, 'å'),
    (0x87, 'ç'),
    (0x88, 'ê'),
    (0x89, 'ë'),
    (0x8A, 'è'),
    (0x8B, 'ï'),
    (0x8C, 'î'),
    (0x8D, 'ì'),
    (0x8E, 'Ä'),
    (0x8F, 'Å'),
    (0x90, 'É'),
    (0x91, 'æ'),
    (0x92, 'Æ'),
    (0x93, 'ô'),
    (0x94, 'ö'),
    (0x95, 'ò'),
    (0x96, 'û'),
    (0x97, 'ù'),
    (0x98, 'ÿ'),
    (0x99, 'Ö'),
    (0x9A, 'Ü'),
    (0x9B, '¢'),
    (0x9C, '£'),
    (0x9D, '¥'),
    (0x9F, 'ƒ'),
    (0xA0, 'á'),
    (0xA1, 'í'),
    (0xA2, 'ó'),
    (0xA3, 'ú'),
    (0xA4, 'ñ'),
    (0xA5, 'Ñ'),
    (0xA6, 'ª'),
    (0xA7, 'º'),
    (0xA8, '¿'),
    (0xAA, '¬'),
    (0xAB, '½'),
    (0xAC, '¼'),
    (0xAD, '¡'),
    (0xAE, '«'),
    (0xAF, '»'),
    (0xE1, 'ß'),
    (0xE6, 'µ'),
    (0xF1, '±'),
    (0xF8, '°'),
    (0xFA, '·'),
    (0xFD, '²'),
];

/// Encodes `text` into code page 437 bytes.
///
/// Characters with no representation become `?`, which is visible on the receipt and
/// therefore reportable, rather than a stray byte the printer may interpret as a
/// command.
pub fn encode(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\u{20AC}' => out.extend_from_slice(b"EUR"),
            '\u{2013}' | '\u{2014}' => out.push(b'-'),
            '\u{2018}' | '\u{2019}' => out.push(b'\''),
            '\u{201C}' | '\u{201D}' => out.push(b'"'),
            '\u{2026}' => out.extend_from_slice(b"..."),
            // Layout owns line breaks; text smuggling one in would shift every
            // following line of the receipt.
            '\n' | '\r' => out.push(b' '),
            c if c.is_ascii() => out.push(c as u8),
            c => out.push(high_byte(c).unwrap_or(b'?')),
        }
    }
    out
}

/// Turns code page 437 bytes back into text.
///
/// This is the tooling direction: `tools/virtual-printer` reads a byte stream off the
/// wire and has to show a human what the paper would say.
pub fn decode(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| match byte {
            0x00..=0x7F => *byte as char,
            _ => high_char(*byte).unwrap_or('\u{FFFD}'),
        })
        .collect()
}

fn high_byte(ch: char) -> Option<u8> {
    CP437_HIGH
        .iter()
        .find(|(_, mapped)| *mapped == ch)
        .map(|(byte, _)| *byte)
}

fn high_char(byte: u8) -> Option<char> {
    CP437_HIGH
        .iter()
        .find(|(mapped, _)| *mapped == byte)
        .map(|(_, ch)| *ch)
}

#[cfg(test)]
mod tests {
    use super::{decode, encode};

    #[test]
    fn ascii_passes_straight_through() {
        assert_eq!(encode("TOTAL 3.50"), b"TOTAL 3.50".to_vec());
    }

    #[test]
    fn european_letters_become_one_byte_each() {
        assert_eq!(encode("Grüße"), vec![b'G', b'r', 0x81, 0xE1, b'e']);
    }

    #[test]
    fn typographic_punctuation_degrades_to_its_ascii_twin() {
        assert_eq!(encode("don\u{2019}t \u{2014} now"), b"don't - now".to_vec());
    }

    #[test]
    fn an_embedded_newline_cannot_inject_an_extra_printed_line() {
        assert_eq!(encode("two\nlines"), b"two lines".to_vec());
    }

    #[test]
    fn accented_text_survives_a_round_trip_through_the_code_page() {
        // The virtual printer decodes what the encoder produced, so a mismatch here
        // would show every European store a receipt full of replacement characters.
        for original in ["Crème brûlée", "Grüße", "niño", "Ålesund"] {
            assert_eq!(decode(&encode(original)), original);
        }
    }

    #[test]
    fn a_byte_outside_the_table_decodes_to_a_visible_replacement() {
        assert_eq!(decode(&[b'A', 0xDB]), "A\u{FFFD}");
    }
}
