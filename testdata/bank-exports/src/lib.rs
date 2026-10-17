//! Bank export corpus (TD.2).
//!
//! Renders TD.1's synthetic ledgers as the files US banks and card issuers actually
//! let customers download (CSV in each institution's layout, OFX and QFX), each with
//! the output an importer should produce. `README.md` documents the corpus;
//! `layouts.json` is the machine-readable spec of every layout.

pub mod corpus;
pub mod expected;
pub mod layouts;
pub mod render;
pub mod source;
pub mod spec;

use std::collections::BTreeMap;

use fm_core::model::{AccountKind, Sha256Hash};
use serde::{Deserialize, Serialize};

use crate::render::FileCtx;
use crate::source::Source;

/// Bumped whenever a change alters the output.
pub const CORPUS_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileEntry {
    pub path: String,
    pub expected: String,
    pub layout: String,
    pub institution: String,
    pub format: String,
    pub persona: String,
    pub transactions: usize,
    pub sha256: Sha256Hash,
}

/// `manifest.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub corpus_version: u32,
    /// TD.1's manifest hash: the ledgers these files were rendered from.
    pub synthetic_manifest_sha256: Sha256Hash,
    pub institutions: Vec<String>,
    pub files: Vec<FileEntry>,
}

/// A fake account number: the real last digits behind a `9999` prefix, padded to
/// 12 digits (bank accounts) or 16 (cards). Never a real number.
pub fn fake_number(last: &str, kind: AccountKind) -> String {
    let len = if kind == AccountKind::CreditCard {
        16
    } else {
        12
    };
    format!("9999{}{last}", "0".repeat(len - 4 - last.len()))
}

/// A fake routing number: `999999` and the institution's index. Real ABA routing
/// numbers never start with 99.
pub fn fake_routing(institution: &str) -> String {
    let i = layouts::institutions()
        .iter()
        .position(|n| *n == institution)
        .unwrap_or(0);
    format!("999999{i:03}")
}

/// Every output file, by path relative to `data/`.
pub fn render_all(sources: &[Source], synthetic_manifest: &[u8]) -> BTreeMap<String, Vec<u8>> {
    let layouts = layouts::all();
    let mut files = BTreeMap::new();
    let mut entries = Vec::new();
    for f in corpus::files() {
        let layout = layouts
            .iter()
            .find(|l| l.id == f.layout)
            .unwrap_or_else(|| panic!("{}: unknown layout {}", f.name, f.layout));
        let src = sources
            .iter()
            .find(|s| s.persona == f.persona)
            .unwrap_or_else(|| panic!("{}: unknown persona", f.name));
        let account = src.account(f.account).clone();
        assert!(
            layout
                .account_kinds
                .contains(&render::kind_name(account.kind)),
            "{}: layout {} doesn't fit a {:?} account",
            f.name,
            layout.id,
            account.kind
        );
        let rows = source::rows(
            src,
            &account,
            f.start,
            f.end,
            f.as_of,
            layout.includes_pending,
        );
        let all = source::rows(
            src,
            &account,
            fm_synth::start(),
            fm_synth::end(),
            f.as_of,
            false,
        );
        let balance_as_of = all.last().and_then(|r| r.balance).unwrap_or(0);
        let balance_before = all
            .iter()
            .take_while(|r| r.posted_date < f.start)
            .last()
            .and_then(|r| r.balance)
            .unwrap_or_else(|| {
                all.first()
                    .and_then(|r| r.balance.map(|b| b - r.amount))
                    .unwrap_or(0)
            });
        let dir = layout.institution_key();
        let file = format!("{dir}/{}.{}", f.name, layout.extension);
        let ctx = FileCtx {
            file: file.clone(),
            persona: f.persona.to_string(),
            number: fake_number(
                account.masked_number.as_deref().unwrap_or("0000"),
                account.kind,
            ),
            routing: fake_routing(layout.institution),
            account,
            as_of: f.as_of,
            start: f.start,
            end: f.end,
            balance_as_of,
            balance_before,
        };
        let (bytes, expected) = render::render(layout, &ctx, &rows);
        let expected_path = format!("{file}.expected.json");
        entries.push(FileEntry {
            path: file.clone(),
            expected: expected_path.clone(),
            layout: layout.id.to_string(),
            institution: layout.institution.to_string(),
            format: layout.extension.to_string(),
            persona: f.persona.to_string(),
            transactions: expected.transactions.len(),
            sha256: Sha256Hash::of(&bytes),
        });
        assert!(
            files.insert(file, bytes).is_none(),
            "{}: duplicate file name",
            f.name
        );
        assert!(
            files
                .insert(expected_path, fm_synth::json::to_lines(&expected))
                .is_none(),
            "{}: duplicate expected file",
            f.name
        );
    }
    let mut institutions: Vec<String> = entries.iter().map(|e| e.institution.clone()).collect();
    institutions.sort();
    institutions.dedup();
    let manifest = Manifest {
        corpus_version: CORPUS_VERSION,
        synthetic_manifest_sha256: Sha256Hash::of(synthetic_manifest),
        institutions,
        files: entries,
    };
    files.insert(
        "manifest.json".to_string(),
        fm_synth::json::to_lines(&manifest),
    );
    files.insert(
        "layouts.json".to_string(),
        fm_synth::json::to_lines(&layouts),
    );
    files
}

/// Loads TD.1's committed data and renders the corpus.
pub fn render_from_committed() -> BTreeMap<String, Vec<u8>> {
    let dir = source::synthetic_data_dir();
    let sources = source::load(&dir);
    let manifest = std::fs::read(dir.join("manifest.json")).expect("TD.1 manifest is readable");
    render_all(&sources, &manifest)
}
