//! Adversarial import fixtures (TD.4). The fixtures are the plain files under `cases/`
//! and `expected.json`; this crate only holds the checks in `tests/`.

use std::path::PathBuf;

/// The folder holding `expected.json` and `cases/`.
pub fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}
