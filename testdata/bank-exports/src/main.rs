//! `fm-bank-exports`: writes the bank export corpus from TD.1's committed ledgers.
//!
//! ```text
//! cargo run --release               # regenerate data/
//! cargo run --release -- --check    # fail if data/ differs from a fresh run
//! ```

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data");
    let check = match std::env::args().nth(1).as_deref() {
        None => false,
        Some("--check") => true,
        Some(other) => {
            eprintln!("unknown argument {other}\nusage: fm-bank-exports [--check]");
            return ExitCode::from(2);
        }
    };
    let files = fm_bank_exports::render_from_committed();
    if check {
        let stale: Vec<&str> = files
            .iter()
            .filter(|(path, bytes)| {
                std::fs::read(out.join(path)).ok().as_deref() != Some(bytes.as_slice())
            })
            .map(|(path, _)| path.as_str())
            .collect();
        if stale.is_empty() {
            println!("{} files match", files.len());
            return ExitCode::SUCCESS;
        }
        eprintln!(
            "out of date (run `cargo run --release`): {}",
            stale.join(", ")
        );
        return ExitCode::FAILURE;
    }
    if out.exists()
        && let Err(e) = std::fs::remove_dir_all(&out)
    {
        eprintln!("can't clear {}: {e}", out.display());
        return ExitCode::FAILURE;
    }
    for (path, bytes) in &files {
        let target = out.join(path);
        if let Some(dir) = target.parent()
            && let Err(e) = std::fs::create_dir_all(dir)
        {
            eprintln!("can't create {}: {e}", dir.display());
            return ExitCode::FAILURE;
        }
        if let Err(e) = std::fs::write(&target, bytes) {
            eprintln!("can't write {}: {e}", target.display());
            return ExitCode::FAILURE;
        }
    }
    println!("wrote {} files to {}", files.len(), out.display());
    ExitCode::SUCCESS
}
