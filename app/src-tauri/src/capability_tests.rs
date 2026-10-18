//! T5.12: the app's real permissions, from the ACL that tauri-build generates out of
//! `capabilities/` (not the mock runtime's empty one). The main window may listen to and
//! stop listening to events, which the unlock guard needs to hear the core's auto-lock;
//! every other Tauri plugin command stays denied. App commands are gated separately, in
//! Rust (`unlock::gated`).

use serde_json::{Value, json};
use tauri::ipc::{CallbackFn, InvokeBody};
use tauri::test::{INVOKE_KEY, MockRuntime, get_ipc_response, mock_builder};
use tauri::webview::InvokeRequest;

/// The permissions the main window is granted. Adding one means changing this list on
/// purpose.
const MAIN_PERMISSIONS: &[&str] = &[
    "core:event:allow-listen",
    "core:event:allow-unlisten",
    // T5.5: the native folder picker for the backup folder. Open only: no save dialog,
    // message boxes or file access.
    "dialog:allow-open",
];

fn window(label: &str) -> (tauri::App<MockRuntime>, tauri::WebviewWindow<MockRuntime>) {
    let app = mock_builder()
        .build(crate::context())
        .expect("the app's real context, with its generated ACL");
    let window = tauri::WebviewWindowBuilder::new(&app, label, Default::default())
        .build()
        .unwrap();
    (app, window)
}

fn call(w: &tauri::WebviewWindow<MockRuntime>, cmd: &str, args: Value) -> Result<Value, Value> {
    let request = InvokeRequest {
        cmd: cmd.into(),
        callback: CallbackFn(0),
        error: CallbackFn(1),
        url: "tauri://localhost".parse().unwrap(),
        body: InvokeBody::Json(args),
        headers: Default::default(),
        invoke_key: INVOKE_KEY.to_string(),
    };
    get_ipc_response(w, request).map(|b| b.deserialize().unwrap())
}

fn denied(result: Result<Value, Value>) -> bool {
    match result {
        Ok(_) => false,
        Err(e) => e.as_str().is_some_and(|m| m.contains("not allowed")),
    }
}

#[test]
fn the_main_window_may_listen_for_events_and_nothing_else() {
    let (_app, main) = window("main");
    let listen = json!({ "event": "unlock://locked", "target": { "kind": "Any" }, "handler": 7 });
    let id = call(&main, "plugin:event|listen", listen.clone())
        .expect("the unlock guard can listen for unlock://locked");
    call(
        &main,
        "plugin:event|unlisten",
        json!({ "event": "unlock://locked", "eventId": id }),
    )
    .expect("and stop listening");

    // The folder picker (T5.5) passes the ACL. The mock runtime has no dialog to show, so
    // the call fails further on; what matters is that it isn't "not allowed".
    assert!(
        !denied(call(
            &main,
            "plugin:dialog|open",
            json!({ "options": { "directory": true } })
        )),
        "dialog|open is allowed for main"
    );

    // Everything else of Tauri's stays denied, so the file can't silently grow.
    for (cmd, args) in [
        (
            "plugin:event|emit",
            json!({ "event": "x", "payload": null }),
        ),
        ("plugin:dialog|save", json!({ "options": {} })),
        ("plugin:dialog|message", json!({ "message": "x" })),
        ("plugin:window|close", json!({ "label": "main" })),
        ("plugin:webview|print", json!({ "label": "main" })),
        ("plugin:path|resolve_directory", json!({ "directory": 1 })),
    ] {
        assert!(
            denied(call(&main, cmd, args)),
            "{cmd} must be denied for main"
        );
    }

    // The grant is for the main window only.
    let (_app, other) = window("other");
    assert!(denied(call(&other, "plugin:event|listen", listen)));
}

/// The capability files themselves: one, for "main", with exactly
/// [`MAIN_PERMISSIONS`].
#[test]
fn the_capability_file_grants_exactly_the_event_permissions() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("capabilities");
    let files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(files, ["main.json"]);
    let main: Value =
        serde_json::from_slice(&std::fs::read(dir.join("main.json")).unwrap()).unwrap();
    assert_eq!(main["identifier"], "main");
    assert_eq!(main["windows"], json!(["main"]));
    assert_eq!(main["permissions"], json!(MAIN_PERMISSIONS));
    assert!(
        main.get("remote").is_none(),
        "no remote URLs get any permission"
    );
}
