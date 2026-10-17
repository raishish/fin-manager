//! Turning file bytes into text.
//!
//! OFX 2.x is UTF-8 XML. OFX 1.x headers say `ENCODING:USASCII` or `UTF-8`, and
//! `CHARSET:1252` (Windows-1252) or `NONE`. Banks don't always tell the truth, so: strip a
//! UTF-8 byte-order mark, use the bytes as UTF-8 if they are valid UTF-8, and otherwise
//! read them as Windows-1252, which maps every byte to a character.

/// Windows-1252 code points for bytes 0x80–0x9F. The five bytes it leaves undefined
/// (0x81, 0x8D, 0x8F, 0x90, 0x9D) map to the C1 control of the same value, as browsers do.
const CP1252_HIGH: [char; 32] = [
    '\u{20AC}', '\u{0081}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}', '\u{2021}',
    '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{008D}', '\u{017D}', '\u{008F}',
    '\u{0090}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}', '\u{2013}', '\u{2014}',
    '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}', '\u{0153}', '\u{009D}', '\u{017E}', '\u{0178}',
];

fn windows_1252(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&b| match b {
            0x80..=0x9F => CP1252_HIGH[usize::from(b - 0x80)],
            _ => char::from(b),
        })
        .collect()
}

/// The file's text: UTF-8 when it is valid UTF-8 (without a leading BOM), otherwise
/// Windows-1252.
pub(crate) fn decode(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_owned(),
        Err(_) => windows_1252(bytes),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_passes_through_and_loses_its_bom() {
        assert_eq!(decode("CAFÉ".as_bytes()), "CAFÉ");
        assert_eq!(decode(b"\xEF\xBB\xBFOFX"), "OFX");
    }

    #[test]
    fn invalid_utf8_is_read_as_windows_1252() {
        // 0xC9 is É in Windows-1252 (and an invalid lone byte in UTF-8); 0x92 is ’.
        assert_eq!(decode(b"CAF\xC9 JOE\x92S"), "CAFÉ JOE’S");
        assert_eq!(decode(b"\x80 \x9F \x81"), "€ Ÿ \u{81}");
        let all: Vec<u8> = (0..=255).collect();
        assert_eq!(decode(&all).chars().count(), 256);
    }
}
