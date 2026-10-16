//! Desktop app shell: Tauri 2 host for the Svelte UI, calling into fm-core.

use serde::Serialize;

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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![core_info])
        .run(tauri::generate_context!())
        .expect("error while running the Tauri application");
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
