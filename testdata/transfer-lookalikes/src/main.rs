//! `fm-transfer-lookalikes`: writes `data/` (ledgers, truth, manifest).
//!
//! ```text
//! cargo run --release               # regenerate
//! cargo run --release -- --check    # fail if the committed files are out of date
//! ```

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data");
    let files = fm_transfer_lookalikes::render(fm_transfer_lookalikes::SEED);
    match std::env::args().nth(1).as_deref() {
        Some("--check") => {
            let stale: Vec<&str> = files
                .iter()
                .filter(|(p, b)| std::fs::read(out.join(p)).ok().as_deref() != Some(b.as_slice()))
                .map(|(p, _)| p.as_str())
                .collect();
            if stale.is_empty() {
                println!("{} files match", files.len());
                ExitCode::SUCCESS
            } else {
                eprintln!(
                    "out of date (run `cargo run --release`): {}",
                    stale.join(", ")
                );
                ExitCode::FAILURE
            }
        }
        None => {
            for (p, b) in &files {
                let target = out.join(p);
                let written = target
                    .parent()
                    .map(std::fs::create_dir_all)
                    .unwrap_or(Ok(()))
                    .and_then(|()| std::fs::write(&target, b));
                if let Err(e) = written {
                    eprintln!("can't write {}: {e}", target.display());
                    return ExitCode::FAILURE;
                }
            }
            println!("wrote {} files to {}", files.len(), out.display());
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("unknown argument {other}\nusage: fm-transfer-lookalikes [--check]");
            ExitCode::from(2)
        }
    }
}
