//! Downloads the CI test model through fm-netguard, allowing only
//! [`DOWNLOAD_ORIGINS`], and prints its verified path.
//!
//! ```text
//! cargo run -p fm-llm --example fetch_test_model -- <dir>
//! FM_LLM_TEST_MODEL=<printed path> cargo test -p fm-llm
//! ```

use std::path::PathBuf;
use std::process::ExitCode;

use fm_llm::{CI_TEST_MODEL, DOWNLOAD_ORIGINS, ensure_model};
use fm_netguard::{Allowlist, NetGuard, Outcome};

fn main() -> ExitCode {
    let Some(dir) = std::env::args_os().nth(1).map(PathBuf::from) else {
        eprintln!("usage: fetch_test_model <dir>");
        return ExitCode::FAILURE;
    };
    let mut allowlist = Allowlist::new();
    for origin in DOWNLOAD_ORIGINS {
        allowlist.allow(origin.parse().expect("valid origin"));
    }
    let guard = NetGuard::new(allowlist);
    let result = ensure_model(&guard, &CI_TEST_MODEL, &dir);
    for entry in guard.log() {
        let outcome = match &entry.outcome {
            Outcome::Completed { status, bytes } => format!("{status}, {bytes} bytes"),
            other => format!("{other:?}"),
        };
        eprintln!(
            "network: {} {}{} [{}] -> {outcome}",
            entry.method, entry.origin, entry.path, entry.purpose
        );
    }
    match result {
        Ok(path) => {
            println!("{}", path.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
