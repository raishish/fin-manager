//! T5.5 through the real IPC path: the model commands before a ledger exists, the
//! onboarding steps, the first insight on the renter's TD.2 files, and the backup
//! reminder. Plus the ignored timing run behind "first insight within 10 minutes".

use serde_json::{Value, json};

use crate::tests::{create_confirmed, invoke, invoke_with, mock_app};
use crate::ui_tests::import_renter;
use crate::unlock::LOCKED;

fn today() -> String {
    crate::ledger_view::utc_date(fm_core::model::Timestamp::now()).to_string()
}

/// Before a ledger exists (and while locked) the model commands work, and nothing that
/// reads the ledger does. The download takes no URL or spec from the caller.
#[test]
fn the_model_download_needs_no_ledger_and_nothing_else_runs() {
    let dir = tempfile::tempdir().unwrap();
    let (_app, w) = mock_app(&std::fs::canonicalize(dir.path()).unwrap());
    assert_eq!(invoke(&w, "unlock_state").unwrap()["status"], "NoLedger");

    let status = invoke(&w, "model_status").unwrap();
    assert_eq!(status["state"], "missing");
    assert_eq!(status["modelId"], crate::model::model_spec().id.as_ref());
    // Extra arguments are ignored: the spec is model_spec(), never the caller's.
    let started = invoke_with(
        &w,
        "start_model_download",
        json!({ "url": "https://evil.example/model.gguf", "spec": { "size": 1 } }),
    )
    .unwrap();
    assert!(["downloading", "failed"].contains(&started["state"].as_str().unwrap()));
    assert_eq!(started["modelId"], crate::model::model_spec().id.as_ref());

    for cmd in [
        "onboarding_state",
        "first_insight",
        "backup_reminder",
        "snooze_backup_reminder",
    ] {
        assert_eq!(
            invoke_with(&w, cmd, json!({ "today": today(), "step": "data" })),
            Err(json!(LOCKED)),
            "{cmd} ran without an open ledger"
        );
    }
}

#[test]
fn onboarding_steps_and_the_first_insight() {
    let dir = tempfile::tempdir().unwrap();
    let (_app, w) = mock_app(&std::fs::canonicalize(dir.path()).unwrap());
    create_confirmed(&w);
    assert_eq!(
        invoke(&w, "onboarding_state").unwrap(),
        json!({ "step": "data", "done": false }),
        "a new ledger starts at bringing in data"
    );
    let empty = invoke_with(&w, "first_insight", json!({ "today": today() })).unwrap();
    assert_eq!(empty["empty"], true);

    import_renter(&w);
    let insight = invoke_with(&w, "first_insight", json!({ "today": today() })).unwrap();
    assert_eq!(insight["empty"], false);
    assert_eq!(insight["currency"], "USD");
    let months = insight["months"].as_array().unwrap();
    assert_eq!(months.len(), 12, "the last 12 months of data");
    assert_eq!(insight["lastMonth"], months[11]["month"]);
    let out = |v: &Value| v["moneyOut"]["amount"]["minor"].as_i64().unwrap();
    assert!(out(&insight) > 0, "money out shows before any category");
    assert_eq!(
        months.iter().map(out).sum::<i64>(),
        out(&insight),
        "the months add up (both from fm-core)"
    );
    let merchants: i64 = insight["merchants"]
        .as_array()
        .unwrap()
        .iter()
        .chain(
            insight["merchantsOther"]
                .as_object()
                .map(|_| &insight["merchantsOther"]),
        )
        .map(|m| m["moneyOut"]["minor"].as_i64().unwrap())
        .sum();
    assert_eq!(merchants, out(&insight), "merchants add up to money out");
    assert!(
        insight["transferPairs"].as_u64().unwrap() > 0,
        "detected transfers"
    );
    assert!(
        !insight["recurring"].as_array().unwrap().is_empty(),
        "rent and subscriptions"
    );

    // Steps move forward, survive a reload, and an unknown one is refused.
    for step in ["insight", "review", "backup", "ask"] {
        let s = invoke_with(&w, "set_onboarding_step", json!({ "step": step })).unwrap();
        assert_eq!(s["step"], step);
        assert_eq!(invoke(&w, "onboarding_state").unwrap()["step"], step);
    }
    assert_eq!(
        invoke_with(&w, "set_onboarding_step", json!({ "step": "skip" })).unwrap_err()["code"],
        "InvalidStep"
    );
    let done = invoke_with(&w, "set_onboarding_step", json!({ "step": "done" })).unwrap();
    assert_eq!(done, json!({ "step": "done", "done": true }));
}

#[test]
fn the_backup_reminder_until_a_folder_is_set() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let (_app, w) = mock_app(&root);
    create_confirmed(&w);
    let r = invoke(&w, "backup_reminder").unwrap();
    assert_eq!(r["reason"], "noFolder");

    let snoozed = invoke(&w, "snooze_backup_reminder").unwrap();
    assert!(snoozed["reason"].is_null(), "snoozed for a day");
    assert!(
        snoozed["snoozedUntil"].as_i64().unwrap() > fm_core::model::Timestamp::now().unix_seconds()
    );

    // A folder and a backup clear it for good (the snooze no longer matters).
    let folder = root.join("backups");
    std::fs::create_dir(&folder).unwrap();
    invoke_with(&w, "set_backup_folder", json!({ "folder": folder })).unwrap();
    invoke(&w, "backup_now").unwrap();
    assert!(invoke(&w, "backup_reminder").unwrap()["reason"].is_null());
}

/// The 10-minute target (SPEC decision 34), measured to the first insight: create the
/// ledger with the real key-derivation settings, confirm the recovery key, import the
/// renter's TD.2 files (12+ months), run transfers and rules, and build the first
/// insight, while a model download runs at the same time. Prints each step's time and
/// the peak resident memory. Run in release:
///
/// ```text
/// cargo test -p fm-app --release --lib first_insight_timing -- --ignored --nocapture
/// FM_LOAD_THREADS=8 cargo test ... # the same with every core busy
/// ```
#[test]
#[ignore = "timing run for the T5.5 evidence"]
fn first_insight_timing() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::time::{Duration, Instant};

    let busy = Arc::new(AtomicBool::new(true));
    let load: usize = std::env::var("FM_LOAD_THREADS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut loaders = Vec::new();
    for _ in 0..load {
        let busy = Arc::clone(&busy);
        loaders.push(std::thread::spawn(move || {
            let mut x = 0u64;
            while busy.load(Ordering::Relaxed) {
                x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            }
            x
        }));
    }
    // Peak RSS, sampled every 100 ms with `ps` (no unsafe code in this crate).
    let peak = Arc::new(AtomicU64::new(0));
    let sampler = {
        let (peak, busy) = (Arc::clone(&peak), Arc::clone(&busy));
        std::thread::spawn(move || {
            let pid = std::process::id().to_string();
            while busy.load(Ordering::Relaxed) {
                if let Ok(out) = std::process::Command::new("ps")
                    .args(["-o", "rss=", "-p", &pid])
                    .output()
                    && let Ok(kb) = String::from_utf8_lossy(&out.stdout).trim().parse::<u64>()
                {
                    peak.fetch_max(kb, Ordering::Relaxed);
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        })
    };

    // A background "model download" over loopback: 300 MB, generated in 1 MB chunks so
    // it doesn't sit in this process's memory, hashed as it streams.
    const CHUNKS: usize = 300;
    let chunk: Vec<u8> = (0..1024 * 1024u32).map(|i| (i % 253) as u8).collect();
    let model_dir = tempfile::tempdir().unwrap();
    let sha: [u8; 32] = {
        use sha2::Digest;
        let mut h = sha2::Sha256::new();
        for _ in 0..CHUNKS {
            h.update(&chunk);
        }
        h.finalize().into()
    };
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let size = (CHUNKS * chunk.len()) as u64;
    std::thread::spawn(move || {
        use std::io::{BufRead, BufReader, Write};
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap() > 0 && line != "\r\n" {
                line.clear();
            }
            let head =
                format!("HTTP/1.1 200 OK\r\nContent-Length: {size}\r\nConnection: close\r\n\r\n");
            let _ = stream.write_all(head.as_bytes());
            for _ in 0..CHUNKS {
                if stream.write_all(&chunk).is_err() {
                    break;
                }
            }
        }
    });
    let origin: fm_netguard::Origin = format!("http://127.0.0.1:{port}").parse().unwrap();
    let model = crate::model::ModelDownload::new(
        fm_llm::ModelSpec {
            id: "timing-model".into(),
            url: format!("{origin}/model.gguf").into(),
            file_name: "model.gguf".into(),
            size,
            sha256: sha,
        },
        model_dir.path(),
        Arc::new(fm_netguard::NetGuard::new(fm_netguard::Allowlist::new())),
        vec![origin],
    );

    let dir = tempfile::tempdir().unwrap();
    let (_app, w) = mock_app(&std::fs::canonicalize(dir.path()).unwrap());
    let start = Instant::now();
    let mut lap = start;
    let mut step = |name: &str| {
        let now = Instant::now();
        println!("T5.5-TIMING {name}: {:.2} s", (now - lap).as_secs_f64());
        lap = now;
    };
    model.start();
    create_confirmed(&w);
    step("create ledger + confirm recovery key (real Argon2 settings)");
    let files = import_renter(&w);
    step(&format!(
        "import {} TD.2 files, transfers and rules",
        files.len()
    ));
    let insight = invoke_with(&w, "first_insight", json!({ "today": today() })).unwrap();
    step("first insight");
    let total = start.elapsed();
    assert_eq!(insight["months"].as_array().unwrap().len(), 12);
    println!(
        "T5.5-TIMING machine work to first insight: {:.2} s (target: 10 min including the user's own steps)",
        total.as_secs_f64()
    );
    let deadline = Instant::now() + Duration::from_secs(300);
    while matches!(
        model.status(),
        crate::model::ModelStatus::Downloading { .. }
    ) && Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(50));
    }
    println!(
        "T5.5-TIMING background model download finished: {:?}",
        model.status()
    );
    busy.store(false, Ordering::Relaxed);
    sampler.join().unwrap();
    for l in loaders {
        l.join().unwrap();
    }
    println!(
        "T5.5-TIMING peak RSS: {} MB (CPU load threads: {load})",
        peak.load(Ordering::Relaxed) / 1024
    );
    assert!(total < Duration::from_secs(600));
}
