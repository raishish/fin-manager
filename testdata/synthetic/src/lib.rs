//! Synthetic ledger generator (TD.1).
//!
//! Builds 24-month ledgers for a few invented households, in the `fm_core::model` JSON
//! form, together with their ground truth: the right category for every transaction,
//! transfer pairs, recurring series and one-offs. Output depends only on the seed.
//! `README.md` documents the files.

pub mod cal;
pub mod catalog;
pub mod describe;
pub mod json;
pub mod personas;
pub mod rng;
pub mod sim;
pub mod truth;

use std::collections::BTreeMap;

use fm_core::model::{Date, Ledger, Sha256Hash, Taxonomy, TaxonomyVersion};
use serde::{Deserialize, Serialize};

use crate::truth::PersonaTruth;

/// The seed the committed data was generated with.
pub const DEFAULT_SEED: u64 = 20_261_016;

/// The 24 months every persona covers.
pub fn start() -> Date {
    cal::ymd(2024, 10, 1)
}

pub fn end() -> Date {
    cal::ymd(2026, 9, 30)
}

/// The shipped default category list (T2.1). The truth refers to its keys.
pub fn taxonomy() -> Taxonomy {
    let t: Taxonomy = serde_json::from_str(include_str!(
        "../../../crates/fm-core/assets/taxonomy/default-v1.json"
    ))
    .expect("default taxonomy parses");
    t.validate().expect("default taxonomy is valid");
    t
}

/// One generated persona.
pub struct Generated {
    pub key: &'static str,
    pub ledger: Ledger,
    pub truth: PersonaTruth,
}

/// Generates every persona.
pub fn generate(seed: u64) -> Vec<Generated> {
    let taxonomy = taxonomy();
    personas::ALL
        .iter()
        .map(|p| {
            let (ledger, truth) = (p.build)(seed).finish(p.description, &taxonomy);
            Generated {
                key: p.key,
                ledger,
                truth,
            }
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileEntry {
    pub path: String,
    pub sha256: Sha256Hash,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersonaEntry {
    pub key: String,
    pub description: String,
    pub accounts: usize,
    pub transactions: usize,
    pub ledger: FileEntry,
    pub truth: FileEntry,
}

/// `manifest.json`: what was generated, from what, with file hashes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub generator_version: u32,
    pub seed: u64,
    pub start: Date,
    pub end: Date,
    pub taxonomy_version: TaxonomyVersion,
    pub personas: Vec<PersonaEntry>,
}

/// Every output file, by path relative to the data folder.
pub fn render(seed: u64) -> BTreeMap<String, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut entries = Vec::new();
    for g in generate(seed) {
        let ledger_path = format!("{}/ledger.json", g.key);
        let truth_path = format!("{}/truth.json", g.key);
        let ledger_bytes = json::to_lines(&g.ledger);
        let truth_bytes = json::to_lines(&g.truth);
        entries.push(PersonaEntry {
            key: g.key.to_string(),
            description: g.truth.description.clone(),
            accounts: g.ledger.accounts.len(),
            transactions: g.ledger.transactions.len(),
            ledger: FileEntry {
                path: ledger_path.clone(),
                sha256: Sha256Hash::of(&ledger_bytes),
            },
            truth: FileEntry {
                path: truth_path.clone(),
                sha256: Sha256Hash::of(&truth_bytes),
            },
        });
        files.insert(ledger_path, ledger_bytes);
        files.insert(truth_path, truth_bytes);
    }
    let manifest = Manifest {
        generator_version: sim::GENERATOR_VERSION,
        seed,
        start: start(),
        end: end(),
        taxonomy_version: taxonomy().version,
        personas: entries,
    };
    let mut bytes = serde_json::to_vec_pretty(&manifest).expect("manifest serialises");
    bytes.push(b'\n');
    files.insert("manifest.json".to_string(), bytes);
    files
}
