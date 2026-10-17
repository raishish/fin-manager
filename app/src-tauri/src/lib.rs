//! Desktop app shell: Tauri 2 host for the Svelte UI, calling into fm-core.

mod crash;
mod network;

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
struct Net(NetGuard);

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
    ])
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let dir = app.path().app_data_dir()?.join("crash-reports");
            let reporter = CrashReporter::new(CrashStore::new(dir), env!("CARGO_PKG_VERSION"));
            reporter.install_panic_hook();
            let ledger_dir = app.path().app_data_dir()?.join("ledger");
            app.manage(unlock::app_unlock(&ledger_dir, reporter.clone()));
            app.manage(reporter);
            app.manage(Net(NetGuard::new(Allowlist::new())));
            unlock::start_autolock_timer(app.handle().clone());
            Ok(())
        })
        .invoke_handler(handler())
        .build(tauri::generate_context!())
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
mod tests {
    use fm_core::crash::{CrashReporter, CrashStore};
    use fm_netguard::{Allowlist, NetGuard};
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

    fn invoke(
        webview: &tauri::WebviewWindow<tauri::test::MockRuntime>,
        cmd: &str,
    ) -> Result<serde_json::Value, serde_json::Value> {
        let request = InvokeRequest {
            cmd: cmd.into(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: "tauri://localhost".parse().unwrap(),
            body: InvokeBody::Json(serde_json::json!({})),
            headers: Default::default(),
            invoke_key: INVOKE_KEY.to_string(),
        };
        get_ipc_response(webview, request).map(|body| body.deserialize().unwrap())
    }

    /// Review finding 2: every command outside the unlock flow is refused while locked,
    /// through the real IPC path. The gate is deny-by-default, so this also covers
    /// commands added later.
    #[test]
    fn every_command_is_refused_while_locked() {
        let dir = tempfile::tempdir().unwrap();
        let app = mock_builder()
            .invoke_handler(handler())
            .build(mock_context(noop_assets()))
            .unwrap();
        let reporter = CrashReporter::new(CrashStore::new(dir.path().join("crash")), "test");
        app.manage(unlock::app_unlock(
            &dir.path().join("ledger"),
            reporter.clone(),
        ));
        app.manage(reporter);
        app.manage(Net(NetGuard::new(Allowlist::new())));
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();

        for allowed in unlock::ALLOWED_WHILE_LOCKED {
            assert!(COMMANDS.contains(allowed), "{allowed} is not registered");
        }
        let gated: Vec<&str> = COMMANDS
            .iter()
            .copied()
            .filter(|c| !unlock::ALLOWED_WHILE_LOCKED.contains(c))
            .collect();
        assert!(gated.contains(&"network_log") && gated.contains(&"change_password"));
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
}
