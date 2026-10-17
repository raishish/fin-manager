//! Restoring from a FIFO is refused at once instead of hanging (T5.2 review finding 4).
//!
//! This test lives in its own test binary, so it runs in its own process (T5.9). It
//! starts `mkfifo`, and a child process shares every open file of the process that
//! starts it until its exec closes them. That briefly includes a ledger's `flock`, so
//! a test in the same process that releases a ledger lock and takes it again can find
//! it still held (`InUse`). In fm-core's unit-test binary that made the backup tests
//! flaky under load. Keep every process spawn out of `src/`.

#![cfg(unix)]

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use fm_core::backup::{BackupError, restore};
use fm_core::storage::Vault;

#[test]
fn restore_refuses_a_fifo_without_hanging() {
    let tmp = tempfile::tempdir().unwrap();
    let fifo = tmp.path().join("not-a-file.fmbackup");
    let made = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap();
    assert!(made.success());
    let vault = Vault::at(&tmp.path().join("target"));
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || tx.send(restore(&fifo, &vault).map_err(|e| e.to_string())));
    let result = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("restore hung on a FIFO");
    assert_eq!(result.unwrap_err(), BackupError::NotABackup.to_string());
    assert!(!tmp.path().join("target").join("ledger.keys").exists());
}
