//! Transfer detection (T1.4).
mod detect;
mod pairing;
#[cfg(test)]
mod tests;
pub use detect::{DetectError, MATCHER_VERSION, TransferReport, detect};
pub use pairing::{Pair, PairKind, find_pairs};
