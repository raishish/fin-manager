//! Desktop app shell: Tauri 2 host for the Svelte UI, calling into fm-core.

mod crash;
mod network;

use fm_core::crash::{CrashReporter, CrashStore};
use fm_netguard::{Allowlist, NetGuard};

mod unlock;

use serde::Serialize;
use tauri::{Manager, State};

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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let dir = app.path().app_data_dir()?.join("crash-reports");
            let reporter = CrashReporter::new(CrashStore::new(dir), env!("CARGO_PKG_VERSION"));
            reporter.install_panic_hook();
            app.manage(reporter);
            app.manage(Net(NetGuard::new(Allowlist::new())));
            let ledger_dir = app.path().app_data_dir()?.join("ledger");
            app.manage(unlock::app_unlock(&ledger_dir));
            unlock::start_autolock_timer(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
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
        .build(tauri::generate_context!())
        .expect("error while building the Tauri application")
        .run(|app, event| {
            // Quitting locks: the database closes and the data key is wiped (SPEC §7).
            if let tauri::RunEvent::Exit = event {
                let unlock = app.state::<unlock::AppUnlock>();
                unlock.lock();
                unlock::sync_crash_terms(&unlock, &app.state::<CrashReporter>());
            }
        });
}

#[cfg(test)]
mod tests {
    #[test]
    fn core_info_comes_from_fm_core() {
        let info = super::core_info();
        assert_eq!(info.name, "fm-core");
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
    }
}
