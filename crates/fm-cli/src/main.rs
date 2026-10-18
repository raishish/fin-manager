//! Developer and evaluation command-line tool.
//!
//! ```text
//! fm-cli models list
//! fm-cli models fetch [NAME...]        download candidates (verified) into the models folder
//! fm-cli models probe [NAME...]        load each and show its chat template
//! fm-cli eval categorize NAME [--cpu] [--limit N] [--threads N] [--variants a,b] [--out RUN]
//!                              [--check-whole | --no-check]   every answer, or none, also
//!                              against the whole prompt (default: the first 3 per household)
//! fm-cli eval questions NAME [--cpu] [--threads N]   TD.3 question types (proxy)
//! fm-cli eval queries NAME [--cpu] [--threads N]     TD.3 question → query (part b)
//! fm-cli eval fallback NAME [--cpu] [--limit N] [--threads N] [--out RUN]
//!                                      T2.5's fallback end to end on TD.1: correction rate
//! fm-cli eval report                   eval/runs/* → eval/report.md, eval/summary.json
//! ```
//!
//! Model files live in `$FM_MODELS_DIR`, or `eval/models` (ignored by git) without it.

mod candidates;
mod eval_categorize;
mod eval_fallback;
mod eval_query;
mod eval_questions;
mod fallback;
mod prompt;
mod report;
mod td1;

use std::path::PathBuf;
use std::process::ExitCode;

use fm_llm::{DOWNLOAD_ORIGINS, ensure_model};
use fm_netguard::{Allowlist, NetGuard, Outcome};

/// `eval/`: the report and runs. Model files go in `eval/models`, ignored by git.
fn eval_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../eval")
}

/// Where model files are: `$FM_MODELS_DIR`, or `eval/models`.
fn models_dir() -> PathBuf {
    std::env::var_os("FM_MODELS_DIR").map_or_else(|| eval_dir().join("models"), PathBuf::from)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["models", "list"] => {
            for c in candidates::CANDIDATES {
                println!("{:<20} {:<10} {}", c.name, c.licence, c.spec.url);
            }
            ExitCode::SUCCESS
        }
        ["models", "fetch", names @ ..] => fetch(names),
        ["models", "probe", names @ ..] => probe(names),
        ["eval", "categorize", name, rest @ ..] => eval(name, rest, Part::Categorize),
        ["eval", "questions", name, rest @ ..] => eval(name, rest, Part::QuestionTypes),
        ["eval", "queries", "--oracle"] => {
            let dir = eval_dir().join("runs/oracle");
            std::fs::create_dir_all(&dir).expect("create the run directory");
            println!("{}", eval_query::oracle(&dir));
            ExitCode::SUCCESS
        }
        ["eval", "queries", name, rest @ ..] => eval(name, rest, Part::Queries),
        ["eval", "fallback", name, rest @ ..] => eval(name, rest, Part::Fallback),
        ["eval", "report"] => report::write(&eval_dir()),
        _ => {
            eprintln!("usage: fm-cli models list | models fetch [NAME...]");
            ExitCode::FAILURE
        }
    }
}

fn fetch(names: &[&str]) -> ExitCode {
    let chosen: Vec<&candidates::Candidate> = if names.is_empty() {
        candidates::main_candidates().iter().collect()
    } else {
        let mut out = Vec::new();
        for n in names {
            match candidates::by_name(n) {
                Some(c) => out.push(c),
                None => {
                    eprintln!("unknown model {n:?}; see `fm-cli models list`");
                    return ExitCode::FAILURE;
                }
            }
        }
        out
    };
    let mut allowlist = Allowlist::new();
    for origin in DOWNLOAD_ORIGINS {
        allowlist.allow(origin.parse().expect("valid origin"));
    }
    let guard = NetGuard::new(allowlist);
    let dir = models_dir();
    let mut ok = true;
    for c in chosen {
        eprintln!("{}: fetching {} bytes", c.name, c.spec.size);
        match ensure_model(&guard, &c.spec, &dir) {
            Ok(path) => println!("{}: {}", c.name, path.display()),
            Err(e) => {
                eprintln!("{}: {e}", c.name);
                ok = false;
            }
        }
    }
    for entry in guard.log() {
        if let Outcome::Completed { status, bytes } = entry.outcome {
            eprintln!(
                "network: {} {} -> {status}, {bytes} bytes",
                entry.method, entry.origin
            );
        } else {
            eprintln!(
                "network: {} {} -> {:?}",
                entry.method, entry.origin, entry.outcome
            );
        }
    }
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn probe(names: &[&str]) -> ExitCode {
    for c in candidates::CANDIDATES
        .iter()
        .filter(|c| names.is_empty() || names.contains(&c.name))
    {
        let path = models_dir().join(c.spec.file_name.as_ref());
        if !path.exists() {
            println!("{}: not downloaded", c.name);
            continue;
        }
        let t = std::time::Instant::now();
        match fm_llm::Model::load(&path, &c.spec) {
            Ok(model) => {
                let loaded = t.elapsed();
                match model.chat_prompt("SYSTEM-TEXT", "\u{1}") {
                    Ok(p) => println!("{}: loaded in {loaded:?}; template: {:?}", c.name, p),
                    Err(e) => println!("{}: loaded in {loaded:?}; template error: {e}", c.name),
                }
            }
            Err(e) => println!("{}: load error: {e}", c.name),
        }
    }
    ExitCode::SUCCESS
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Part {
    Categorize,
    QuestionTypes,
    Queries,
    Fallback,
}

/// How the llama.cpp in this binary was built: `GGML_CPU_REPACK` from the CMake cache of
/// the newest llama.cpp build next to it (`target/<profile>/build/`). `None` when there's
/// none, e.g. a binary copied elsewhere.
fn cpu_repack() -> Option<String> {
    let build = std::env::current_exe().ok()?.parent()?.join("build");
    let newest = std::fs::read_dir(build)
        .ok()?
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("llama-cpp-sys-2-")
        })
        .map(|e| e.path().join("out/build/CMakeCache.txt"))
        .filter_map(|p| Some((p.metadata().ok()?.modified().ok()?, p)))
        .max()?
        .1;
    std::fs::read_to_string(newest)
        .ok()?
        .lines()
        .find_map(|l| l.strip_prefix("GGML_CPU_REPACK:BOOL="))
        .map(str::to_string)
}

/// `eval categorize`, `eval questions` and `eval queries` for one model.
fn eval(name: &str, rest: &[&str], part: Part) -> ExitCode {
    let Some(candidate) = candidates::by_name(name) else {
        eprintln!("unknown model {name:?}; see `fm-cli models list`");
        return ExitCode::FAILURE;
    };
    let mut cpu = false;
    let mut limit = None;
    let mut threads = None;
    let mut variants = prompt::VARIANTS.to_vec();
    let mut out: Option<String> = None;
    let mut check_whole = false;
    let mut no_check = false;
    let mut args = rest.iter();
    while let Some(arg) = args.next() {
        match *arg {
            "--cpu" => cpu = true,
            "--check-whole" => check_whole = true,
            "--no-check" => no_check = true,
            "--limit" => limit = args.next().and_then(|v| v.parse().ok()),
            "--threads" => threads = args.next().and_then(|v| v.parse().ok()),
            "--out" => out = args.next().map(|v| (*v).to_string()),
            "--variants" => {
                variants = args
                    .next()
                    .map(|v| {
                        v.split(',')
                            .filter_map(prompt::Variant::from_name)
                            .collect()
                    })
                    .unwrap_or_default();
            }
            other => {
                eprintln!("unknown option {other:?}");
                return ExitCode::FAILURE;
            }
        }
    }
    let path = models_dir().join(candidate.spec.file_name.as_ref());
    let t = std::time::Instant::now();
    let model = match fm_llm::Model::load_with(
        &path,
        &candidate.spec,
        &fm_llm::LoadOptions { gpu: !cpu },
    ) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("{name}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let load_ms = t.elapsed().as_secs_f64() * 1000.0;
    let run_dir = eval_dir().join("runs").join(out.unwrap_or_else(|| {
        if cpu {
            format!("{name}-cpu")
        } else {
            name.to_string()
        }
    }));
    std::fs::create_dir_all(&run_dir).expect("create the run directory");
    let mut meta = serde_json::json!({
        "model": name, "model_id": candidate.spec.id, "url": candidate.spec.url,
        "sha256": fm_llm::hex(&candidate.spec.sha256), "size": candidate.spec.size,
        "licence": candidate.licence, "provenance": candidate.provenance,
        "parameters": candidate.parameters, "cpu_only": cpu,
        "gpu_offload_supported": fm_llm::gpu_offload_supported().unwrap_or(false),
        "load_ms": load_ms, "limit": limit, "threads": threads,
        "cpu_repack": cpu_repack(),
    });
    let file = if part == Part::QuestionTypes {
        meta["questions"] = eval_questions::run(&model, &run_dir, candidate.answer_lead, threads);
        "questions-meta.json"
    } else if part == Part::Queries {
        meta["queries"] = eval_query::run(&model, &run_dir, candidate.answer_lead, threads);
        "queries-meta.json"
    } else if part == Part::Fallback {
        let settings = eval_fallback::Settings {
            run_dir: &run_dir,
            limit,
            threads,
            answer_lead: candidate.answer_lead,
        };
        meta["fallback"] = eval_fallback::run(&model, &settings);
        println!(
            "{}",
            serde_json::to_string_pretty(&meta["fallback"]).unwrap()
        );
        "fallback-meta.json"
    } else {
        let scratch = std::env::temp_dir();
        let households: Vec<td1::Household> = td1::PERSONAS
            .iter()
            .map(|p| td1::starting_state(p, &scratch))
            .collect();
        let settings = eval_categorize::Settings {
            run_dir: &run_dir,
            variants: &variants,
            limit,
            threads,
            answer_lead: candidate.answer_lead,
            check_whole,
            no_check,
        };
        meta["categorize"] = eval_categorize::run(&model, &households, &settings);
        "meta.json"
    };
    std::fs::write(
        run_dir.join(file),
        serde_json::to_string_pretty(&meta).unwrap(),
    )
    .expect("write the run summary");
    drop(model);
    ExitCode::SUCCESS
}
