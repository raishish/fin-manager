use std::fmt;
use std::str::FromStr;

use sha2::{Digest, Sha256};

use super::ModelError;

/// A SHA-256 digest: of a raw import's bytes, or of a GGUF model file.
///
/// Serialised as 64 lowercase hex digits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Sha256Hash([u8; 32]);

impl Sha256Hash {
    /// The SHA-256 of `bytes`.
    pub fn of(bytes: &[u8]) -> Sha256Hash {
        Sha256Hash(Sha256::digest(bytes).into())
    }

    pub const fn from_bytes(digest: [u8; 32]) -> Sha256Hash {
        Sha256Hash(digest)
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for Sha256Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_hex(f, &self.0)
    }
}

impl FromStr for Sha256Hash {
    type Err = ModelError;

    /// Parses exactly 64 lowercase hex digits.
    fn from_str(s: &str) -> Result<Sha256Hash, ModelError> {
        let bytes = decode_hex(s).ok_or(ModelError::InvalidHash)?;
        bytes
            .try_into()
            .map(Sha256Hash)
            .map_err(|_| ModelError::InvalidHash)
    }
}

serde_as_str!(Sha256Hash);

/// Writes `bytes` as lowercase hex.
pub(crate) fn write_hex(f: &mut impl fmt::Write, bytes: &[u8]) -> fmt::Result {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    for &b in bytes {
        f.write_char(char::from(DIGITS[usize::from(b >> 4)]))?;
        f.write_char(char::from(DIGITS[usize::from(b & 0x0f)]))?;
    }
    Ok(())
}

/// Decodes lowercase hex; `None` on odd length, uppercase or any other character.
pub(crate) fn decode_hex(s: &str) -> Option<Vec<u8>> {
    fn nibble(c: u8) -> Option<u8> {
        match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            _ => None,
        }
    }
    let (pairs, rest) = s.as_bytes().as_chunks::<2>();
    if !rest.is_empty() {
        return None;
    }
    pairs
        .iter()
        .map(|&[hi, lo]| Some((nibble(hi)? << 4) | nibble(lo)?))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_digests() {
        assert_eq!(
            Sha256Hash::of(b"").to_string(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            Sha256Hash::of(b"abc").to_string(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn parse_round_trips_and_rejects_bad_hex() {
        let h = Sha256Hash::of(b"abc");
        assert_eq!(h.to_string().parse::<Sha256Hash>().unwrap(), h);
        let upper = h.to_string().to_uppercase();
        assert_eq!(upper.parse::<Sha256Hash>(), Err(ModelError::InvalidHash));
        assert!("".parse::<Sha256Hash>().is_err());
        assert!("ab".parse::<Sha256Hash>().is_err());
        assert!(format!("{h}0").parse::<Sha256Hash>().is_err());
        assert!(format!("{h}00").parse::<Sha256Hash>().is_err());
        assert!(
            h.to_string()
                .replace('a', "g")
                .parse::<Sha256Hash>()
                .is_err()
        );
    }

    #[test]
    fn hex_round_trips_every_byte() {
        let all: Vec<u8> = (0..=255).collect();
        let mut s = String::new();
        write_hex(&mut s, &all).unwrap();
        assert_eq!(decode_hex(&s).unwrap(), all);
        assert_eq!(decode_hex("é"), None);
    }
}
