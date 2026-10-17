//! Desktop app shell: Tauri 2 host for the Svelte UI, calling into fm-core.

mod backup;
mod categories;
mod connectors;
mod crash;
mod dashboard;
mod deletion;
mod importing;
mod ledger_view;
mod network;
mod review;

use std::sync::Arc;

use fm_core::crash::{CrashReporter, CrashStore};
use fm_netguard::{Allowlist, NetGuard};

mod unlock;

use serde::Serialize;
use tauri::ipc::Invoke;
use tauri::{Manager, Runtime, State};

/// What the UI shows to prove it can reach the core.
#[derive(Debug, Serialize)]
pub struct CoreInfo {
    name: &'static str,
    version: &'static str,
}

/// Returns the core crate's identity. The UI's first call into fm-core.
#[tauri::command]
fn core_info() -> CoreInfo {
    CoreInfo {
        name: fm_core::NAME,
        version: env!("CARGO_PKG_VERSION"),
    }
}

/// The app's one network client. Every request any feature makes goes through it, so its
/// log is the whole picture. Nothing is allowed until a feature adds an origin.
struct Net(Arc<NetGuard>);

#[tauri::command]
fn network_log(net: State<'_, Net>, since_id: Option<u64>) -> Vec<network::NetworkLogEntry> {
    network::entries_since(&net.0, since_id)
}

#[tauri::command]
fn network_allowlist(net: State<'_, Net>) -> Vec<String> {
    network::allowed_origins(&net.0)
}

#[tauri::command]
fn crash_settings(reporter: State<'_, CrashReporter>) -> crash::CrashSettings {
    crash::settings(reporter.store())
}

#[tauri::command]
fn set_crash_reports_enabled(
    reporter: State<'_, CrashReporter>,
    enabled: bool,
) -> Result<crash::CrashSettings, String> {
    crash::set_enabled(reporter.store(), enabled)
}

#[tauri::command]
fn crash_reports(reporter: State<'_, CrashReporter>) -> Result<Vec<crash::SavedReport>, String> {
    crash::list(reporter.store())
}

#[tauri::command]
fn crash_report(reporter: State<'_, CrashReporter>, id: String) -> Result<String, String> {
    crash::read(reporter.store(), &id)
}

#[tauri::command]
fn delete_crash_report(reporter: State<'_, CrashReporter>, id: String) -> Result<(), String> {
    crash::delete(reporter.store(), &id)
}

/// Every command the app registers (keep in step with [`handler`]). Apart from
/// [`unlock::ALLOWED_WHILE_LOCKED`], all of them are refused until the ledger is
/// unlocked (`unlock::gated`); the test below checks each one.
#[cfg(test)]
const COMMANDS: &[&str] = &[
    "core_info",
    "network_log",
    "network_allowlist",
    "crash_settings",
    "set_crash_reports_enabled",
    "crash_reports",
    "crash_report",
    "delete_crash_report",
    "unlock_state",
    "password_strength",
    "create_ledger",
    "finish_create",
    "unlock_with_password",
    "unlock_with_recovery_key",
    "set_password_after_recovery",
    "unlock_with_touch_id",
    "recovery_to_confirm",
    "confirm_recovery_key",
    "change_password",
    "set_autolock_minutes",
    "lock",
    "activity",
    "backup_status",
    "set_backup_folder",
    "backup_now",
    "restore_backup",
    "data_choices",
    "preview_deletion",
    "delete_data",
    "delete_everything",
    "delete_old_backups",
    "pending_backup_offer",
    "simplefin_status",
    "simplefin_inspect_token",
    "simplefin_connect",
    "simplefin_sync",
    "simplefin_disconnect",
    "dashboard",
    "review_queue",
    "transactions",
    "category_picker",
    "preview_rule",
    "correct_transaction",
    "suggestions",
    "approve_decisions",
    "reject_decisions",
    "accept_suggestion",
    "dismiss_suggestion",
    "taxonomy",
    "edit_taxonomy",
    "rules",
    "set_rule_enabled",
    "delete_rule",
    "accounts",
    "import_file",
];

/// The app's command handler, behind the unlock gate.
fn handler<R: Runtime>() -> impl Fn(Invoke<R>) -> bool + Send + Sync + 'static {
    unlock::gated(tauri::generate_handler![
        core_info,
        network_log,
        network_allowlist,
        crash_settings,
        set_crash_reports_enabled,
        crash_reports,
        crash_report,
        delete_crash_report,
        unlock::commands::unlock_state,
        unlock::commands::password_strength,
        unlock::commands::create_ledger,
        unlock::commands::finish_create,
        unlock::commands::unlock_with_password,
        unlock::commands::unlock_with_recovery_key,
        unlock::commands::set_password_after_recovery,
        unlock::commands::unlock_with_touch_id,
        unlock::commands::recovery_to_confirm,
        unlock::commands::confirm_recovery_key,
        unlock::commands::change_password,
        unlock::commands::set_autolock_minutes,
        unlock::commands::lock,
        unlock::commands::activity,
        backup::commands::backup_status,
        backup::commands::set_backup_folder,
        backup::commands::backup_now,
        backup::commands::restore_backup,
        deletion::commands::data_choices,
        deletion::commands::preview_deletion,
        deletion::commands::delete_data,
        deletion::commands::delete_everything,
        deletion::commands::delete_old_backups,
        deletion::commands::pending_backup_offer,
        connectors::commands::simplefin_status,
        connectors::commands::simplefin_inspect_token,
        connectors::commands::simplefin_connect,
        connectors::commands::simplefin_sync,
        connectors::commands::simplefin_disconnect,
        dashboard::commands::dashboard,
        review::commands::review_queue,
        review::commands::transactions,
        review::commands::category_picker,
        review::commands::preview_rule,
        review::commands::correct_transaction,
        review::commands::suggestions,
        review::commands::approve_decisions,
        review::commands::reject_decisions,
        review::commands::accept_suggestion,
        review::commands::dismiss_suggestion,
        categories::commands::taxonomy,
        categories::commands::edit_taxonomy,
        categories::commands::rules,
        categories::commands::set_rule_enabled,
        categories::commands::delete_rule,
        importing::commands::accounts,
        importing::commands::import_file,
    ])
}

/// The app's context: its config, assets and the permission ACL generated from
/// `capabilities/`. Expanded once (the macro embeds Info.plist on macOS), so the tests use
/// the same one (`capability_tests`).
fn context<R: Runtime>() -> tauri::Context<R> {
    tauri::generate_context!()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let dir = app.path().app_data_dir()?.join("crash-reports");
            let reporter = CrashReporter::new(CrashStore::new(dir), env!("CARGO_PKG_VERSION"));
            reporter.install_panic_hook();
            let ledger_dir = app.path().app_data_dir()?.join("ledger");
            let net = Arc::new(NetGuard::new(Allowlist::new()));
            app.manage(unlock::app_unlock(
                &ledger_dir,
                reporter.clone(),
                connectors::BridgeAllowance::new(Arc::clone(&net)),
            ));
            app.manage(reporter);
            app.manage(Net(net));
            app.manage(connectors::Busy::default());
            unlock::start_autolock_timer(app.handle().clone());
            Ok(())
        })
        .invoke_handler(handler())
        .build(context())
        .expect("error while building the Tauri application")
        .run(|app, event| {
            // Quitting locks (the database closes and the data key is wiped) and deletes
            // the Touch ID Keychain item (SPEC §7).
            if let tauri::RunEvent::Exit = event {
                app.state::<unlock::AppUnlock>().shut_down();
            }
        });
}

#[cfg(test)]
mod capability_tests;
#[cfg(test)]
mod ipc_contract_tests;
#[cfg(test)]
mod ui_tests;

#[cfg(test)]
pub(crate) mod tests {
    use fm_core::crash::{CrashReporter, CrashStore};
    use fm_netguard::{Allowlist, NetGuard};
    use sha2::{Digest, Sha256};
    use tauri::Manager;
    use tauri::ipc::{CallbackFn, InvokeBody};
    use tauri::test::{INVOKE_KEY, get_ipc_response, mock_builder, mock_context, noop_assets};
    use tauri::webview::InvokeRequest;

    use super::*;

    #[test]
    fn core_info_comes_from_fm_core() {
        let info = super::core_info();
        assert_eq!(info.name, "fm-core");
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
    }

    pub(crate) type Webview = tauri::WebviewWindow<tauri::test::MockRuntime>;

    pub(crate) fn invoke(
        webview: &Webview,
        cmd: &str,
    ) -> Result<serde_json::Value, serde_json::Value> {
        invoke_with(webview, cmd, serde_json::json!({}))
    }

    pub(crate) fn invoke_with(
        webview: &Webview,
        cmd: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, serde_json::Value> {
        let request = InvokeRequest {
            cmd: cmd.into(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: "tauri://localhost".parse().unwrap(),
            body: InvokeBody::Json(args),
            headers: Default::default(),
            invoke_key: INVOKE_KEY.to_string(),
        };
        get_ipc_response(webview, request).map(|body| body.deserialize().unwrap())
    }

    /// The app on the mock runtime, with its data under `dir`.
    pub(crate) fn mock_app(
        dir: &std::path::Path,
    ) -> (tauri::App<tauri::test::MockRuntime>, Webview) {
        let app = mock_builder()
            .invoke_handler(handler())
            .build(mock_context(noop_assets()))
            .unwrap();
        let reporter = CrashReporter::new(CrashStore::new(dir.join("crash")), "test");
        let net = Arc::new(NetGuard::new(Allowlist::new()));
        app.manage(unlock::app_unlock(
            &dir.join("ledger"),
            reporter.clone(),
            connectors::BridgeAllowance::new(Arc::clone(&net)),
        ));
        app.manage(reporter);
        app.manage(Net(net));
        app.manage(connectors::Busy::default());
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        (app, webview)
    }

    fn ledger_files(dir: &std::path::Path) -> Vec<(String, String)> {
        let mut files: Vec<(String, String)> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap())
            .filter(|e| e.file_name() != "ledger.lock")
            .map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                let bytes = std::fs::read(e.path()).unwrap();
                (
                    name,
                    format!(
                        "{} bytes, sha256 {:?}",
                        bytes.len(),
                        Sha256::digest(&bytes).as_slice()
                    ),
                )
            })
            .collect();
        files.sort();
        files
    }

    /// Creates and confirms a ledger through the IPC path.
    pub(crate) fn create_confirmed(webview: &Webview) {
        let shown = invoke_with(
            webview,
            "create_ledger",
            serde_json::json!({ "password": "velvet anchor mosaic tundra" }),
        )
        .unwrap();
        let answers: Vec<&serde_json::Value> = shown["challenge"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| &shown["groups"][i.as_u64().unwrap() as usize])
            .collect();
        invoke_with(
            webview,
            "confirm_recovery_key",
            serde_json::json!({ "answers": answers }),
        )
        .unwrap();
    }

    /// T5.3: "delete everything" through the IPC path: the typed phrase is checked in
    /// Rust, the ledger is gone afterwards, and the backup offer can be accepted once,
    /// with no ledger left, acting only on what the app offered.
    #[test]
    fn delete_everything_then_the_backup_offer() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let folder = root.join("backups");
        std::fs::create_dir(&folder).unwrap();
        let (_app, w) = mock_app(&root);
        create_confirmed(&w);
        invoke_with(
            &w,
            "set_backup_folder",
            serde_json::json!({ "folder": folder }),
        )
        .unwrap();
        invoke(&w, "backup_now").unwrap();
        let ledger_dir = root.join("ledger");

        let err = invoke_with(
            &w,
            "delete_everything",
            serde_json::json!({ "confirmation": "delete", "allowUnverifiedKeychain": false }),
        )
        .unwrap_err();
        assert_eq!(err["code"], "ConfirmationMismatch");
        assert!(ledger_dir.join("ledger.keys").exists(), "nothing touched");
        assert_eq!(
            invoke(&w, "delete_old_backups").unwrap_err()["code"],
            "NoOffer",
            "no offer before a deletion"
        );

        let confirmed = |allow: bool| {
            invoke_with(
                &w,
                "delete_everything",
                serde_json::json!({
                    "confirmation": "Delete everything",
                    "allowUnverifiedKeychain": allow,
                }),
            )
        };
        // An unsigned macOS build can't check the Keychain for a Touch ID key a signed
        // build may have stored: nothing is destroyed until the user goes ahead. Linux
        // has no Keychain, so there is nothing to check.
        let offer = if cfg!(target_os = "macos") {
            let err = confirmed(false).unwrap_err();
            assert_eq!(err["code"], "KeychainUnverifiable");
            assert!(err["message"].as_str().unwrap().contains("Keychain Access"));
            assert!(
                ledger_dir.join("ledger.keys").exists(),
                "nothing destroyed yet"
            );
            confirmed(true).unwrap()
        } else {
            confirmed(false).unwrap()
        };
        assert_eq!(offer["count"], 1);
        assert!(offer["message"].as_str().unwrap().contains("older backups"));
        assert!(!ledger_dir.exists(), "ledger gone");
        assert_eq!(invoke(&w, "unlock_state").unwrap()["status"], "NoLedger");
        // Gated commands are refused again: there is nothing open.
        assert_eq!(
            invoke(&w, "data_choices").unwrap_err(),
            serde_json::json!("locked")
        );

        assert_eq!(invoke(&w, "pending_backup_offer").unwrap()["count"], 1);
        assert_eq!(invoke(&w, "delete_old_backups").unwrap(), 1);
        assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 0);
        assert_eq!(
            invoke(&w, "delete_old_backups").unwrap_err()["code"],
            "NoOffer"
        );
    }

    /// T5.3: preview and delete through the IPC path. The cascade itself is tested in
    /// depth in fm-core (deletion::tests); this checks the commands: the preview is
    /// what the deletion then removes, and bad input gets a clear error.
    #[test]
    fn preview_and_delete_an_account_through_the_app() {
        use fm_core::model::{Account, AccountId, AccountKind, Currency, Timestamp};
        let dir = tempfile::tempdir().unwrap();
        let (app, w) = mock_app(&std::fs::canonicalize(dir.path()).unwrap());
        create_confirmed(&w);
        app.state::<unlock::AppUnlock>()
            .write_ledger(|open| {
                open.ledger().insert_account(&Account {
                    id: AccountId(7),
                    name: "Old card".into(),
                    kind: AccountKind::CreditCard,
                    currency: Currency::new("USD").unwrap(),
                    institution: None,
                    masked_number: None,
                    external_id: None,
                    own_account: true,
                    created_at: Timestamp::now(),
                })
            })
            .unwrap()
            .unwrap();
        let account = serde_json::json!({
            "target": { "kind": "account", "id": 7 },
            "deletePartialFiles": false,
        });
        let preview = invoke_with(&w, "preview_deletion", account.clone()).unwrap();
        assert_eq!(preview["counts"]["accounts"], 1);
        assert_eq!(
            invoke(&w, "data_choices").unwrap()["accounts"]
                .as_array()
                .unwrap()
                .len(),
            1,
            "the preview deleted nothing"
        );
        let deleted = invoke_with(&w, "delete_data", account.clone()).unwrap();
        assert_eq!(deleted["counts"], preview["counts"]);
        assert!(deleted["backupOffer"].is_null(), "no backup folder set");
        assert_eq!(
            invoke(&w, "data_choices").unwrap()["accounts"],
            serde_json::json!([])
        );

        let err = invoke_with(&w, "delete_data", account).unwrap_err();
        assert_eq!(err["code"], "NotFound");
        let bad_date = serde_json::json!({
            "target": { "kind": "range", "account": null, "from": "March", "to": "2026-03-31" },
            "deletePartialFiles": false,
        });
        assert_eq!(
            invoke_with(&w, "preview_deletion", bad_date).unwrap_err()["code"],
            "InvalidDate"
        );
        let backwards = serde_json::json!({
            "target": { "kind": "range", "account": null, "from": "2026-04-01", "to": "2026-03-01" },
            "deletePartialFiles": false,
        });
        assert_eq!(
            invoke_with(&w, "preview_deletion", backwards).unwrap_err()["code"],
            "EmptyRange"
        );
    }

    /// T5.2: `restore_backup` is the one data-touching command reachable while locked.
    /// Through the real IPC path: it is refused whenever a ledger exists (open or
    /// locked) and leaves every file as it was; on a fresh install it restores, and the
    /// ledger then opens with the password.
    #[test]
    fn restore_backup_only_runs_on_a_fresh_install() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let folder = root.join("backups");
        std::fs::create_dir(&folder).unwrap();
        let password = serde_json::json!({ "password": "velvet anchor mosaic tundra" });

        // An install with a ledger and a backup.
        let (_a, a) = mock_app(&root.join("a"));
        let shown = invoke_with(&a, "create_ledger", password.clone()).unwrap();
        let answers: Vec<&serde_json::Value> = shown["challenge"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| &shown["groups"][i.as_u64().unwrap() as usize])
            .collect();
        invoke_with(
            &a,
            "confirm_recovery_key",
            serde_json::json!({ "answers": answers }),
        )
        .unwrap();
        invoke_with(
            &a,
            "set_backup_folder",
            serde_json::json!({ "folder": folder }),
        )
        .unwrap();
        let made = invoke(&a, "backup_now").unwrap();
        let file = folder.join(made["fileName"].as_str().unwrap());
        let restore = serde_json::json!({ "file": file });
        let ledger_dir = root.join("a").join("ledger");
        let before = ledger_files(&ledger_dir);

        // Refused while open, and while locked; nothing on disk changes.
        for state in ["unlocked", "locked"] {
            if state == "locked" {
                invoke(&a, "lock").unwrap();
            }
            let err = invoke_with(&a, "restore_backup", restore.clone()).unwrap_err();
            assert_eq!(err["code"], "LedgerExists", "{state}");
            assert_eq!(
                ledger_files(&ledger_dir),
                before,
                "{state}: files untouched"
            );
        }

        // A fresh install restores it, and it opens with the password.
        let (_b, b) = mock_app(&root.join("b"));
        assert_eq!(invoke(&b, "unlock_state").unwrap()["status"], "NoLedger");
        invoke_with(&b, "restore_backup", restore.clone()).unwrap();
        assert_eq!(invoke(&b, "unlock_state").unwrap()["status"], "Locked");
        // Same keyfile. (The database differs: after copying, `backup_now` records
        // the backup time in the original. Unlocking below proves the copy.)
        let restored = ledger_files(&root.join("b").join("ledger"));
        let keyfile =
            |files: &[(String, String)]| files.iter().find(|(n, _)| n == "ledger.keys").cloned();
        assert_eq!(keyfile(&restored), keyfile(&before));
        invoke_with(&b, "unlock_with_password", password).unwrap();
        assert_eq!(invoke(&b, "unlock_state").unwrap()["status"], "Unlocked");
        // Now that b has a ledger, restoring again is refused too.
        let err = invoke_with(&b, "restore_backup", restore).unwrap_err();
        assert_eq!(err["code"], "LedgerExists");

        // The restored ledger takes its own backup id: a's backup shows as another
        // ledger's, and b's own backups never rotate a's away.
        invoke_with(
            &b,
            "set_backup_folder",
            serde_json::json!({ "folder": folder }),
        )
        .unwrap();
        let mine = invoke(&b, "backup_now").unwrap();
        assert_ne!(mine["fileName"], made["fileName"]);
        let status = invoke(&b, "backup_status").unwrap();
        let listed: Vec<(String, bool)> = status["backups"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| {
                (
                    v["fileName"].as_str().unwrap().to_string(),
                    v["mine"].as_bool().unwrap(),
                )
            })
            .collect();
        assert!(listed.contains(&(made["fileName"].as_str().unwrap().into(), false)));
        assert!(listed.contains(&(mine["fileName"].as_str().unwrap().into(), true)));
    }

    /// T5.2 review finding 5: a lock raises the interrupt flag, and a backup holding
    /// the session sees it and stops, leaving nothing behind; the flag is lowered
    /// once the lock is done.
    #[test]
    fn a_lock_interrupts_a_running_backup() {
        use std::sync::atomic::Ordering;
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let folder = root.join("backups");
        std::fs::create_dir(&folder).unwrap();
        let (app, webview) = mock_app(&root);
        let shown = invoke_with(
            &webview,
            "create_ledger",
            serde_json::json!({ "password": "velvet anchor mosaic tundra" }),
        )
        .unwrap();
        let answers: Vec<&serde_json::Value> = shown["challenge"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| &shown["groups"][i.as_u64().unwrap() as usize])
            .collect();
        invoke_with(
            &webview,
            "confirm_recovery_key",
            serde_json::json!({ "answers": answers }),
        )
        .unwrap();
        invoke_with(
            &webview,
            "set_backup_folder",
            serde_json::json!({ "folder": folder }),
        )
        .unwrap();

        let unlock = app.state::<unlock::AppUnlock>();
        unlock.interrupt.store(true, Ordering::SeqCst); // a lock is waiting
        assert!(matches!(
            unlock.daily_backup(),
            Some(Err(fm_core::backup::BackupError::Interrupted))
        ));
        assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 0);
        unlock.interrupt.store(false, Ordering::SeqCst);

        unlock.lock();
        assert!(!unlock.interrupt.load(Ordering::SeqCst));
        assert_eq!(
            invoke(&webview, "unlock_state").unwrap()["status"],
            "Locked"
        );
    }

    /// Review finding 2: every command outside the unlock flow is refused while locked,
    /// through the real IPC path. The gate is deny-by-default, so this also covers
    /// commands added later.
    #[test]
    fn every_command_is_refused_while_locked() {
        let dir = tempfile::tempdir().unwrap();
        let (_app, webview) = mock_app(dir.path());

        for allowed in unlock::ALLOWED_WHILE_LOCKED {
            assert!(COMMANDS.contains(allowed), "{allowed} is not registered");
        }
        let gated: Vec<&str> = COMMANDS
            .iter()
            .copied()
            .filter(|c| !unlock::ALLOWED_WHILE_LOCKED.contains(c))
            .collect();
        assert!(gated.contains(&"network_log") && gated.contains(&"change_password"));
        assert!(gated.contains(&"backup_status") && gated.contains(&"backup_now"));
        assert!(!gated.contains(&"restore_backup"));
        for cmd in &gated {
            assert_eq!(
                invoke(&webview, cmd),
                Err(serde_json::json!(unlock::LOCKED)),
                "{cmd} ran while locked"
            );
        }
        // An unknown command is refused the same way.
        assert_eq!(
            invoke(&webview, "not_a_command"),
            Err(serde_json::json!(unlock::LOCKED))
        );
        // The unlock flow itself still runs.
        let state = invoke(&webview, "unlock_state").unwrap();
        assert_eq!(state["status"], "NoLedger");
    }

    /// T5.4: the SimpleFIN commands through the IPC path. The connector itself is
    /// tested against a fake bridge in fm-core (connectors::simplefin::tests).
    #[test]
    fn simplefin_commands_answer_with_codes_and_allow_nothing_unasked() {
        let dir = tempfile::tempdir().unwrap();
        let (_app, w) = mock_app(&std::fs::canonicalize(dir.path()).unwrap());
        assert_eq!(
            invoke(&w, "simplefin_status").unwrap_err(),
            serde_json::json!("locked")
        );
        create_confirmed(&w);
        let status = invoke(&w, "simplefin_status").unwrap();
        assert_eq!(status["connected"], false);
        assert_eq!(status["requestsLeft"], 24);
        assert_eq!(status["requestsPerDay"], 24);

        let err = invoke_with(
            &w,
            "simplefin_connect",
            serde_json::json!({ "setupToken": "not a token", "confirmedHost": "" }),
        )
        .unwrap_err();
        assert_eq!(err["code"], "InvalidToken");
        // The demo token's host, shown for the user to confirm; nothing is sent.
        let demo = "aHR0cHM6Ly9iZXRhLWJyaWRnZS5zaW1wbGVmaW4ub3JnL3NpbXBsZWZpbi9jbGFpbS9ERU1PLXYyLTUyMjI1MEZCMzBBNDNGRUNEQjBD";
        let token = invoke_with(
            &w,
            "simplefin_inspect_token",
            serde_json::json!({ "setupToken": demo }),
        )
        .unwrap();
        assert_eq!(
            token,
            serde_json::json!({ "host": "beta-bridge.simplefin.org", "simplefin": true })
        );
        let err = invoke_with(
            &w,
            "simplefin_connect",
            serde_json::json!({ "setupToken": demo, "confirmedHost": "evil.example" }),
        )
        .unwrap_err();
        assert_eq!(err["code"], "HostNotConfirmed");
        // A token for this machine is refused before anything is sent.
        let local = "aHR0cHM6Ly9sb2NhbGhvc3QvY2xhaW0veA=="; // https://localhost/claim/x
        assert_eq!(
            invoke_with(
                &w,
                "simplefin_inspect_token",
                serde_json::json!({ "setupToken": local }),
            )
            .unwrap_err()["code"],
            "InvalidToken"
        );
        assert_eq!(
            invoke(&w, "simplefin_sync").unwrap_err()["code"],
            "NotConnected"
        );
        assert_eq!(
            invoke(&w, "simplefin_disconnect").unwrap()["connected"],
            false
        );
        assert_eq!(
            invoke(&w, "network_allowlist").unwrap(),
            serde_json::json!([])
        );
        assert_eq!(
            invoke(&w, "network_log").unwrap(),
            serde_json::json!([]),
            "nothing sent"
        );
    }
}
