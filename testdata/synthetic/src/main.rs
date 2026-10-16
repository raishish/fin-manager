//! `fm-synth`: writes the synthetic ledgers and their ground truth.
//!
//! ```text
//! cargo run --release                  # regenerate data/ with the default seed
//! cargo run --release -- --check       # fail if data/ differs from a fresh run
//! cargo run --release -- --seed 7 --out /tmp/synth
//! ```

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut seed = fm_synth::DEFAULT_SEED;
    let mut out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data");
    let mut check = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--seed" => match args.next().and_then(|s| s.parse().ok()) {
                Some(s) => seed = s,
                None => return usage("--seed needs a number"),
            },
            "--out" => match args.next() {
                Some(p) => out = PathBuf::from(p),
                None => return usage("--out needs a folder"),
            },
            "--check" => check = true,
            other => return usage(&format!("unknown argument {other}")),
        }
    }

    let files = fm_synth::render(seed);
    if check {
        let mut stale = Vec::new();
        for (path, bytes) in &files {
            if std::fs::read(out.join(path)).ok().as_deref() != Some(bytes.as_slice()) {
                stale.push(path.as_str());
            }
        }
        if stale.is_empty() {
            println!("{} files match seed {seed}", files.len());
            return ExitCode::SUCCESS;
        }
        eprintln!(
            "out of date (run `cargo run --release` to regenerate): {}",
            stale.join(", ")
        );
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
    println!(
        "wrote {} files to {} (seed {seed})",
        files.len(),
        out.display()
    );
    ExitCode::SUCCESS
}

fn usage(why: &str) -> ExitCode {
    eprintln!("{why}\nusage: fm-synth [--seed N] [--out DIR] [--check]");
    ExitCode::from(2)
}
