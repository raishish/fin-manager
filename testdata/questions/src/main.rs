//! `fm-questions`: writes `data/questions.json` from TD.1's committed data.
//!
//! ```text
//! cargo run --release               # regenerate
//! cargo run --release -- --check    # fail if the committed file is out of date
//! ```

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data/questions.json");
    let bytes = fm_questions::render();
    match std::env::args().nth(1).as_deref() {
        Some("--check") => {
            if std::fs::read(&path).ok().as_deref() == Some(bytes.as_slice()) {
                println!("data/questions.json matches");
                ExitCode::SUCCESS
            } else {
                eprintln!("data/questions.json is out of date: run `cargo run --release`");
                ExitCode::FAILURE
            }
        }
        None => {
            if let Err(e) = std::fs::create_dir_all(path.parent().expect("has a folder"))
                .and_then(|()| std::fs::write(&path, &bytes))
            {
                eprintln!("can't write {}: {e}", path.display());
                return ExitCode::FAILURE;
            }
            println!("wrote {}", path.display());
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("unknown argument {other}\nusage: fm-questions [--check]");
            ExitCode::from(2)
        }
    }
}
