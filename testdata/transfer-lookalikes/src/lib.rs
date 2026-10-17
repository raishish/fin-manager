//! Transfer look-alike corpus (TD.6).
//!
//! Three households' ledgers (2025, `fm_core::model` JSON) built to test transfer
//! detection's precision: many same-amount, near-date pairs across own accounts that are
//! NOT transfers, next to true transfers that are hard to see (weak wording, long-weekend
//! gaps, equal amounts days apart, card-to-card). Each ledger's truth lists every true
//! pair and every look-alike candidate. `README.md` documents the patterns.

pub mod book;
mod households;
mod patterns;
pub mod truth;

use std::collections::BTreeMap;

use fm_core::model::Sha256Hash;
use serde::{Deserialize, Serialize};

/// The seed the committed data was built with.
pub const SEED: u64 = 20_261_017;

/// Bumped whenever a change alters the output.
pub const VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub key: String,
    pub description: String,
    pub transactions: usize,
    pub transfer_pairs: usize,
    pub lookalikes: usize,
    pub ledger: String,
    pub ledger_sha256: Sha256Hash,
    pub truth: String,
    pub truth_sha256: Sha256Hash,
}

/// `manifest.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub version: u32,
    pub seed: u64,
    pub candidate_window_days: i32,
    pub households: Vec<Entry>,
    /// Look-alike candidates and true pairs per pattern, over all households.
    pub lookalikes_by_pattern: BTreeMap<String, usize>,
    pub pairs_by_pattern: BTreeMap<String, usize>,
}

/// Every output file, by path relative to `data/`.
pub fn render(seed: u64) -> BTreeMap<String, Vec<u8>> {
    let taxonomy = fm_synth::taxonomy();
    let (start, end) = (
        fm_synth::cal::ymd(2025, 1, 1),
        fm_synth::cal::ymd(2025, 12, 31),
    );
    let mut files = BTreeMap::new();
    let mut entries = Vec::new();
    let mut by_pattern: BTreeMap<String, usize> = BTreeMap::new();
    let mut pairs_by_pattern: BTreeMap<String, usize> = BTreeMap::new();
    for h in households::ALL {
        let (ledger, truth) = (h.build)(seed).finish(h.description, &taxonomy, start, end);
        for c in &truth.lookalikes {
            *by_pattern.entry(c.pattern.clone()).or_default() += 1;
        }
        for p in &truth.transfer_pairs {
            *pairs_by_pattern.entry(p.pattern.clone()).or_default() += 1;
        }
        let ledger_bytes = fm_synth::json::to_lines(&ledger);
        let truth_bytes = fm_synth::json::to_lines(&truth);
        let (lp, tp) = (
            format!("{}/ledger.json", h.key),
            format!("{}/truth.json", h.key),
        );
        entries.push(Entry {
            key: h.key.to_string(),
            description: h.description.to_string(),
            transactions: ledger.transactions.len(),
            transfer_pairs: truth.transfer_pairs.len(),
            lookalikes: truth.lookalikes.len(),
            ledger: lp.clone(),
            ledger_sha256: Sha256Hash::of(&ledger_bytes),
            truth: tp.clone(),
            truth_sha256: Sha256Hash::of(&truth_bytes),
        });
        files.insert(lp, ledger_bytes);
        files.insert(tp, truth_bytes);
    }
    let manifest = Manifest {
        version: VERSION,
        seed,
        candidate_window_days: book::CANDIDATE_WINDOW_DAYS,
        households: entries,
        lookalikes_by_pattern: by_pattern,
        pairs_by_pattern,
    };
    files.insert(
        "manifest.json".to_string(),
        fm_synth::json::to_lines(&manifest),
    );
    files
}
