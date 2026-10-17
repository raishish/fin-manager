//! The recovery key (SPEC §7): 256 random bits that also unlock the data key.
//!
//! Shown as 56 Crockford base32 characters in 14 groups of 4: the 32 key bytes plus a
//! 3-byte SHA-256 checksum, so a mistyped key is caught before the slow unlock is
//! tried. Input ignores case, spaces and dashes, and reads `O` as `0` and `I`/`L` as
//! `1`, which are the usual misreadings of a printed key.

use std::fmt;

use sha2::{Digest, Sha256};
use zeroize::{Zeroize, Zeroizing};

use crate::storage::StorageError;

const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const KEY_BYTES: usize = 32;
const CHECK_BYTES: usize = 3;
/// 35 bytes = 280 bits = 56 characters of 5 bits.
const CHARS: usize = (KEY_BYTES + CHECK_BYTES) * 8 / 5;
const GROUP: usize = 4;
pub const GROUPS: usize = CHARS / GROUP;
/// How many groups the user types back to confirm they saved the key.
pub const CHALLENGE_GROUPS: usize = 3;

/// A recovery key. The bytes are wiped when it is dropped; `Debug` never shows them.
#[derive(Clone)]
pub struct RecoveryKey {
    bytes: Zeroizing<[u8; KEY_BYTES]>,
}

/// Why typed text isn't a recovery key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryKeyError {
    /// A character that isn't in the key's alphabet.
    InvalidCharacter,
    /// Too few or too many characters.
    WrongLength,
    /// Every character is valid but the key doesn't add up: a typo.
    Mistyped,
}

impl fmt::Display for RecoveryKeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidCharacter => "the recovery key contains a character it can't have",
            Self::WrongLength => "a recovery key has 56 characters",
            Self::Mistyped => "the recovery key has a typo",
        })
    }
}

impl std::error::Error for RecoveryKeyError {}

impl RecoveryKey {
    pub fn generate() -> Result<Self, StorageError> {
        let mut bytes = Zeroizing::new([0u8; KEY_BYTES]);
        getrandom::fill(bytes.as_mut())?;
        Ok(Self { bytes })
    }

    /// Reads a key typed or pasted by the user.
    pub fn parse(input: &str) -> Result<Self, RecoveryKeyError> {
        let mut values = Zeroizing::new(Vec::with_capacity(CHARS));
        for c in input.chars() {
            if c.is_whitespace() || c == '-' {
                continue;
            }
            values.push(decode_char(c).ok_or(RecoveryKeyError::InvalidCharacter)?);
        }
        if values.len() != CHARS {
            return Err(RecoveryKeyError::WrongLength);
        }
        let mut raw = Zeroizing::new([0u8; KEY_BYTES + CHECK_BYTES]);
        let mut acc: u32 = 0;
        let mut bits = 0;
        let mut out = 0;
        for &v in values.iter() {
            acc = (acc << 5) | u32::from(v);
            bits += 5;
            if bits >= 8 {
                bits -= 8;
                raw[out] = (acc >> bits) as u8;
                out += 1;
            }
        }
        acc.zeroize();
        let mut bytes = Zeroizing::new([0u8; KEY_BYTES]);
        bytes.copy_from_slice(&raw[..KEY_BYTES]);
        if raw[KEY_BYTES..] != checksum(&bytes) {
            return Err(RecoveryKeyError::Mistyped);
        }
        Ok(Self { bytes })
    }

    /// The secret handed to storage.
    pub fn as_bytes(&self) -> &[u8] {
        self.bytes.as_ref()
    }

    /// The 14 groups of 4 characters, for display.
    pub fn groups(&self) -> Vec<Zeroizing<String>> {
        let text = self.encode();
        text.as_bytes()
            .chunks(GROUP)
            .map(|g| Zeroizing::new(String::from_utf8_lossy(g).into_owned()))
            .collect()
    }

    /// The groups joined with dashes, e.g. for a printable sheet.
    pub fn display(&self) -> Zeroizing<String> {
        let groups = self.groups();
        let mut out = Zeroizing::new(String::with_capacity(CHARS + GROUPS));
        for (i, g) in groups.iter().enumerate() {
            if i > 0 {
                out.push('-');
            }
            out.push_str(g);
        }
        out
    }

    fn encode(&self) -> Zeroizing<String> {
        let mut raw = Zeroizing::new([0u8; KEY_BYTES + CHECK_BYTES]);
        raw[..KEY_BYTES].copy_from_slice(self.bytes.as_ref());
        raw[KEY_BYTES..].copy_from_slice(&checksum(&self.bytes));
        let mut text = Zeroizing::new(String::with_capacity(CHARS));
        let mut acc: u32 = 0;
        let mut bits = 0;
        for &b in raw.iter() {
            acc = (acc << 8) | u32::from(b);
            bits += 8;
            while bits >= 5 {
                bits -= 5;
                text.push(char::from(ALPHABET[((acc >> bits) & 31) as usize]));
            }
        }
        acc.zeroize();
        text
    }
}

impl fmt::Debug for RecoveryKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RecoveryKey(..)")
    }
}

impl PartialEq for RecoveryKey {
    fn eq(&self, other: &Self) -> bool {
        // Not secret-dependent in any way that matters: both sides are held locally.
        self.bytes.as_ref() == other.bytes.as_ref()
    }
}

impl Eq for RecoveryKey {}

fn checksum(key: &[u8; KEY_BYTES]) -> [u8; CHECK_BYTES] {
    let digest = Sha256::digest(key);
    [digest[0], digest[1], digest[2]]
}

fn decode_char(c: char) -> Option<u8> {
    let c = match c.to_ascii_uppercase() {
        'O' => '0',
        'I' | 'L' => '1',
        c => c,
    };
    ALPHABET
        .iter()
        .position(|&a| char::from(a) == c)
        .map(|i| i as u8)
}

/// Which groups the user must type back to show they saved the key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Challenge {
    /// Zero-based group positions, ascending and distinct.
    pub groups: [usize; CHALLENGE_GROUPS],
}

impl Challenge {
    pub fn random() -> Result<Self, StorageError> {
        let mut picked: Vec<usize> = Vec::with_capacity(CHALLENGE_GROUPS);
        while picked.len() < CHALLENGE_GROUPS {
            let mut b = [0u8; 1];
            getrandom::fill(&mut b)?;
            // Reject the top of the byte range so every group is equally likely.
            if usize::from(b[0]) >= 256 - 256 % GROUPS {
                continue;
            }
            let g = usize::from(b[0]) % GROUPS;
            if !picked.contains(&g) {
                picked.push(g);
            }
        }
        picked.sort_unstable();
        Ok(Self {
            groups: [picked[0], picked[1], picked[2]],
        })
    }

    /// True if each answer matches its group, read with the same leniency as
    /// [`RecoveryKey::parse`].
    pub fn check(&self, key: &RecoveryKey, answers: &[&str]) -> bool {
        if answers.len() != CHALLENGE_GROUPS {
            return false;
        }
        let groups = key.groups();
        self.groups.iter().zip(answers).all(|(&g, answer)| {
            let typed: Option<Vec<u8>> = answer
                .chars()
                .filter(|c| !c.is_whitespace() && *c != '-')
                .map(decode_char)
                .collect();
            let expected: Vec<u8> = groups[g].chars().filter_map(decode_char).collect();
            typed.is_some_and(|t| t == expected)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_round_trip_through_their_text() {
        for _ in 0..50 {
            let key = RecoveryKey::generate().unwrap();
            let text = key.display();
            assert_eq!(text.len(), CHARS + GROUPS - 1);
            assert_eq!(key.groups().len(), 14);
            assert_eq!(RecoveryKey::parse(&text).unwrap(), key);
        }
    }

    #[test]
    fn input_is_read_leniently() {
        let key = RecoveryKey::generate().unwrap();
        let text = key.display().replace('-', " ").to_lowercase();
        assert_eq!(RecoveryKey::parse(&text).unwrap(), key);
        let squashed: String = key.display().chars().filter(|&c| c != '-').collect();
        assert_eq!(RecoveryKey::parse(&squashed).unwrap(), key);
        // O for 0 and I/L for 1, as printed keys are commonly misread.
        let misread = key.display().replace('0', "O").replace('1', "l");
        assert_eq!(RecoveryKey::parse(&misread).unwrap(), key);
    }

    #[test]
    fn typos_are_caught_before_unlocking() {
        let key = RecoveryKey::generate().unwrap();
        let text = key.display().to_string();
        let mut chars: Vec<char> = text.chars().collect();
        let i = chars.iter().position(|&c| c != '-').unwrap();
        chars[i] = if chars[i] == 'A' { 'B' } else { 'A' };
        let typo: String = chars.into_iter().collect();
        assert_eq!(RecoveryKey::parse(&typo), Err(RecoveryKeyError::Mistyped));
        assert_eq!(
            RecoveryKey::parse(&text[..text.len() - 1]),
            Err(RecoveryKeyError::WrongLength)
        );
        assert_eq!(
            RecoveryKey::parse(&format!("U{}", &text[1..])),
            Err(RecoveryKeyError::InvalidCharacter)
        );
        assert_eq!(
            RecoveryKey::parse("ÄBCD"),
            Err(RecoveryKeyError::InvalidCharacter)
        );
    }

    #[test]
    fn debug_never_shows_the_key() {
        let key = RecoveryKey::generate().unwrap();
        let debug = format!("{key:?}");
        assert_eq!(debug, "RecoveryKey(..)");
        assert!(!debug.contains(key.groups()[0].as_str()));
    }

    #[test]
    fn challenges_pick_three_distinct_groups() {
        for _ in 0..200 {
            let c = Challenge::random().unwrap();
            assert!(c.groups.windows(2).all(|w| w[0] < w[1]));
            assert!(c.groups[2] < GROUPS);
        }
    }

    #[test]
    fn challenges_accept_only_the_right_groups() {
        let key = RecoveryKey::generate().unwrap();
        let other = RecoveryKey::generate().unwrap();
        let c = Challenge { groups: [0, 6, 13] };
        let g = key.groups();
        let right = [g[0].as_str(), g[6].as_str(), g[13].as_str()];
        assert!(c.check(&key, &right));
        let lower: Vec<String> = right.iter().map(|s| s.to_lowercase()).collect();
        let lower: Vec<&str> = lower.iter().map(String::as_str).collect();
        assert!(c.check(&key, &lower));
        let h = other.groups();
        assert!(!c.check(&key, &[h[0].as_str(), h[6].as_str(), h[13].as_str()]));
        assert!(!c.check(&key, &right[..2]));
        assert!(!c.check(&key, &[right[0], right[1], "!!!!"]));
    }
}
