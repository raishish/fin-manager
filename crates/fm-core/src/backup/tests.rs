//! Backups of a real encrypted ledger, end to end. Argon2 runs with test parameters.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use tempfile::TempDir;

use super::*;
use crate::storage::KdfParams;

const P: KdfParams = KdfParams::INSECURE_TEST;
const PASSWORD: &[u8] = b"velvet anchor mosaic tundra";
const NEW_PASSWORD: &[u8] = b"purple monkey dishwasher";
const RECOVERY: &[u8] = b"RECOVERY-KEY-BYTES-FOR-THE-TESTS";
/// 2026-10-16T12:00:00Z.
const T0: u64 = 1_792_152_000;

fn at(secs: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(secs)
}

struct Fixture {
    _tmp: TempDir,
    root: PathBuf,
    folder: PathBuf,
    ledger: Unlocked,
}

impl Fixture {
    /// A ledger with a password, a recovery key and some data, and an empty backup
    /// folder next to it.
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(tmp.path()).unwrap();
        let folder = root.join("backups");
        fs::create_dir(&folder).unwrap();
        let vault = Vault::at(&root.join("ledger"));
        let mut ledger = vault.create_with_params(PASSWORD, P).unwrap();
        ledger
            .set_recovery_key_with_params(PASSWORD, RECOVERY, P)
            .unwrap();
        put(&ledger, "note", "before the backup");
        Self {
            _tmp: tmp,
            root,
            folder,
            ledger,
        }
    }

    fn fresh_vault(&self, name: &str) -> Vault {
        Vault::at(&self.root.join(name))
    }
}

fn put(ledger: &Unlocked, key: &str, value: &str) {
    ledger
        .connection()
        .execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2) \
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            [key, value],
        )
        .unwrap();
}

fn get(ledger: &Unlocked, key: &str) -> Option<String> {
    ledger
        .connection()
        .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
        .ok()
}

fn names(folder: &Path) -> Vec<String> {
    let mut out: Vec<String> = fs::read_dir(folder)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    out.sort();
    out
}

// ---- done when: restore round trip with the password and with the recovery key -------

#[test]
fn restore_round_trip_with_the_password_and_with_the_recovery_key() {
    let f = Fixture::new();
    let backup = back_up(&f.ledger, &f.folder, at(T0)).unwrap();
    put(&f.ledger, "note", "after the backup");

    for (name, secret, use_recovery) in [
        ("with-password", PASSWORD, false),
        ("with-recovery-key", RECOVERY, true),
    ] {
        let vault = f.fresh_vault(name);
        restore(&backup.path, &vault).unwrap();
        assert_eq!(vault.state(), VaultState::Ready, "{name}");
        let restored = if use_recovery {
            vault.unlock_with_recovery_key(secret).unwrap()
        } else {
            vault.unlock(secret).unwrap()
        };
        assert_eq!(
            get(&restored, "note").as_deref(),
            Some("before the backup"),
            "{name}: the backup holds the data as it was"
        );
    }
}

#[test]
fn restored_files_are_private() {
    let f = Fixture::new();
    let backup = back_up(&f.ledger, &f.folder, at(T0)).unwrap();
    let vault = f.fresh_vault("restored");
    restore(&backup.path, &vault).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(vault.dir()), 0o700);
        assert_eq!(mode(vault.keyfile_path()), 0o600);
        assert_eq!(mode(vault.db_path()), 0o600);
        assert_eq!(mode(&backup.path), 0o600, "the backup file itself");
    }
}

#[test]
fn a_backup_from_before_a_password_change_needs_the_old_password() {
    let mut f = Fixture::new();
    let backup = back_up(&f.ledger, &f.folder, at(T0)).unwrap();
    f.ledger
        .set_password_with_params(PASSWORD, NEW_PASSWORD, P)
        .unwrap();
    let vault = f.fresh_vault("restored");
    restore(&backup.path, &vault).unwrap();
    assert!(vault.unlock(NEW_PASSWORD).is_err());
    vault.unlock(PASSWORD).unwrap();
}

// ---- restore never overwrites ---------------------------------------------------------

#[test]
fn restore_refuses_where_a_ledger_exists() {
    let f = Fixture::new();
    let backup = back_up(&f.ledger, &f.folder, at(T0)).unwrap();
    // The live ledger (open), and a locked one.
    assert!(matches!(
        restore(&backup.path, f.ledger.vault()),
        Err(BackupError::LedgerExists)
    ));
    let other = f.fresh_vault("other");
    drop(other.create_with_params(PASSWORD, P).unwrap());
    let before = fs::read(other.keyfile_path()).unwrap();
    assert!(matches!(
        restore(&backup.path, &other),
        Err(BackupError::LedgerExists)
    ));
    assert_eq!(fs::read(other.keyfile_path()).unwrap(), before);
}

#[test]
fn restore_never_touches_a_folder_with_other_files() {
    let f = Fixture::new();
    let backup = back_up(&f.ledger, &f.folder, at(T0)).unwrap();
    let vault = f.fresh_vault("busy");
    fs::create_dir(vault.dir()).unwrap();
    fs::write(vault.dir().join("ledger.db"), b"").unwrap();
    fs::write(vault.dir().join("something-else"), b"keep me").unwrap();
    assert!(matches!(
        restore(&backup.path, &vault),
        Err(BackupError::LedgerExists)
    ));
    assert_eq!(
        fs::read(vault.dir().join("something-else")).unwrap(),
        b"keep me"
    );
    assert_eq!(fs::read(vault.dir().join("ledger.db")).unwrap(), b"");
}

#[test]
fn restore_into_an_empty_folder_works() {
    let f = Fixture::new();
    let backup = back_up(&f.ledger, &f.folder, at(T0)).unwrap();
    let vault = f.fresh_vault("empty");
    fs::create_dir(vault.dir()).unwrap();
    fs::write(vault.dir().join("ledger.lock"), b"").unwrap();
    restore(&backup.path, &vault).unwrap();
    vault.unlock(PASSWORD).unwrap();
}

#[test]
fn a_damaged_backup_restores_nothing() {
    let f = Fixture::new();
    let backup = back_up(&f.ledger, &f.folder, at(T0)).unwrap();
    let bytes = fs::read(&backup.path).unwrap();

    let truncated = f.root.join("truncated.fmbackup");
    fs::write(&truncated, &bytes[..bytes.len() / 2]).unwrap();
    let flipped = f.root.join("flipped.fmbackup");
    let mut bad = bytes.clone();
    let middle = bad.len() / 2;
    bad[middle] ^= 0xff;
    fs::write(&flipped, &bad).unwrap();
    let text = f.root.join("notes.txt");
    fs::write(&text, b"not a backup at all, just some notes").unwrap();

    for (file, want) in [
        (&truncated, "Truncated"),
        (&flipped, "Corrupt"),
        (&text, "NotABackup"),
    ] {
        let vault = f.fresh_vault("target");
        let err = restore(file, &vault).unwrap_err();
        assert!(format!("{err:?}").starts_with(want), "{file:?}: {err:?}");
        assert!(!vault.dir().exists(), "nothing left behind");
        let leftovers: Vec<String> = names(&f.root)
            .into_iter()
            .filter(|n| n.contains("restoring"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }
}

// ---- making backups -------------------------------------------------------------------

#[test]
fn a_backup_is_a_snapshot() {
    let f = Fixture::new();
    let backup = back_up(&f.ledger, &f.folder, at(T0)).unwrap();
    assert_eq!(backup.file_name, "fin-manager-20261016T120000Z.fmbackup");
    assert_eq!(list(&f.folder).unwrap(), vec![backup.clone()]);
    // The ledger keeps working while and after the backup is made.
    put(&f.ledger, "note", "later");
    assert_eq!(get(&f.ledger, "note").as_deref(), Some("later"));
}

#[test]
fn two_backups_in_the_same_second_both_survive() {
    let f = Fixture::new();
    let a = back_up(&f.ledger, &f.folder, at(T0)).unwrap();
    let b = back_up(&f.ledger, &f.folder, at(T0)).unwrap();
    assert_ne!(a.path, b.path);
    assert_eq!(b.file_name, "fin-manager-20261016T120000Z-1.fmbackup");
}

#[test]
fn the_folder_must_exist_be_absolute_and_be_outside_the_ledger() {
    let f = Fixture::new();
    let gone = f.root.join("unplugged-drive");
    assert!(matches!(
        back_up(&f.ledger, &gone, at(T0)),
        Err(BackupError::FolderMissing(_))
    ));
    assert!(!gone.exists(), "a missing folder is never created");
    assert!(matches!(
        back_up(&f.ledger, Path::new("relative/folder"), at(T0)),
        Err(BackupError::FolderNotAllowed(_))
    ));
    let inside = f.ledger.vault().dir().join("backups");
    fs::create_dir(&inside).unwrap();
    for folder in [inside.as_path(), f.ledger.vault().dir()] {
        assert!(matches!(
            back_up(&f.ledger, folder, at(T0)),
            Err(BackupError::FolderNotAllowed(_))
        ));
    }
    // A folder that contains the ledger (like the home folder) is fine.
    back_up(&f.ledger, &f.root, at(T0)).unwrap();
}

#[test]
fn stale_temporary_files_are_ignored_and_cleaned_up() {
    let f = Fixture::new();
    let stale = f.folder.join(".fin-manager-20261001T000000Z.fmbackup.tmp");
    let fresh = f.folder.join(".fin-manager-20261016T115959Z.fmbackup.tmp");
    for p in [&stale, &fresh] {
        fs::write(p, b"half a backup").unwrap();
    }
    let old = SystemTime::now() - Duration::from_secs(3600);
    fs::File::options()
        .write(true)
        .open(&stale)
        .unwrap()
        .set_modified(old)
        .unwrap();
    assert!(list(&f.folder).unwrap().is_empty(), "list ignores them");
    assert!(
        rotate(&f.folder).unwrap().is_empty(),
        "rotation ignores them"
    );

    back_up(&f.ledger, &f.folder, SystemTime::now()).unwrap();
    assert!(!stale.exists(), "a stale temporary file is removed");
    assert!(fresh.exists(), "one that may still be in progress is kept");
}

// ---- rotation and deletion -------------------------------------------------------------

#[test]
fn rotation_keeps_7_4_12_and_never_touches_other_files() {
    let f = Fixture::new();
    let real = back_up(&f.ledger, &f.folder, at(T0)).unwrap();
    let content = fs::read(&real.path).unwrap();
    // A year and a bit of daily backups (copies of a real one, named by date).
    for d in 1..400u64 {
        let t = i64::try_from(T0 - d * 86_400).unwrap();
        fs::write(f.folder.join(name::backup_name(t, 0)), &content).unwrap();
    }
    let others = [
        "notes.txt",
        "fin-manager-20261016T120000Z.fmbackup.bak",
        "copy of fin-manager-20251016T120000Z.fmbackup",
        "fin-manager-2025-01-01.fmbackup",
        "Fin-Manager-20250101T000000Z.fmbackup",
    ];
    for other in others {
        fs::write(f.folder.join(other), b"not ours").unwrap();
    }
    fs::create_dir(f.folder.join("fin-manager-20250101T000000Z-7.fmbackup")).unwrap();

    rotate(&f.folder).unwrap();

    let kept = list(&f.folder).unwrap();
    assert!(kept.len() <= DAILY + WEEKLY + MONTHLY, "{}", kept.len());
    assert!(kept.len() >= MONTHLY);
    assert_eq!(kept[0].path, real.path, "the newest is kept");
    for other in others {
        assert_eq!(
            fs::read(f.folder.join(other)).unwrap(),
            b"not ours",
            "{other}"
        );
    }
    assert!(
        f.folder
            .join("fin-manager-20250101T000000Z-7.fmbackup")
            .is_dir()
    );
}

#[test]
fn delete_all_removes_only_our_backups() {
    let f = Fixture::new();
    back_up(&f.ledger, &f.folder, at(T0)).unwrap();
    back_up(&f.ledger, &f.folder, at(T0 + 86_400)).unwrap();
    fs::write(f.folder.join("notes.txt"), b"mine").unwrap();
    assert_eq!(delete_all(&f.folder).unwrap(), 2);
    assert_eq!(names(&f.folder), ["notes.txt"]);
}

// ---- settings and schedule ------------------------------------------------------------

#[test]
fn nothing_happens_until_a_folder_is_chosen() {
    let f = Fixture::new();
    assert_eq!(Settings::load(&f.ledger).unwrap(), Settings::default());
    assert!(daily(&f.ledger, at(T0)).is_none());
    assert!(after_import(&f.ledger, at(T0)).is_none());
    assert!(matches!(
        back_up_now(&f.ledger, at(T0)),
        Err(BackupError::NoFolder)
    ));
}

#[test]
fn daily_backups_run_once_a_day() {
    let f = Fixture::new();
    set_folder(&f.ledger, &f.folder).unwrap();
    let first = daily(&f.ledger, at(T0)).unwrap().unwrap();
    let settings = Settings::load(&f.ledger).unwrap();
    assert_eq!(settings.last_at, Some(first.created_unix));
    assert_eq!(settings.folder.as_deref(), Some(f.folder.as_path()));
    assert!(daily(&f.ledger, at(T0 + 3_600)).is_none());
    assert!(daily(&f.ledger, at(T0 + 86_400)).is_some());
    // A clock set back past the last backup makes one due.
    assert!(daily(&f.ledger, at(T0 - 3_600)).is_some());
}

#[test]
fn after_an_import_a_backup_is_always_made() {
    let f = Fixture::new();
    set_folder(&f.ledger, &f.folder).unwrap();
    daily(&f.ledger, at(T0)).unwrap().unwrap();
    after_import(&f.ledger, at(T0 + 60)).unwrap().unwrap();
    assert_eq!(
        list(&f.folder).unwrap().len(),
        1,
        "same day: the newest is kept"
    );
    assert_eq!(
        Settings::load(&f.ledger).unwrap().last_at,
        Some(i64::try_from(T0 + 60).unwrap())
    );
}

#[test]
fn a_failure_is_recorded_and_cleared_by_the_next_success() {
    let f = Fixture::new();
    set_folder(&f.ledger, &f.folder).unwrap();
    fs::remove_dir(&f.folder).unwrap(); // the drive was unplugged
    assert!(daily(&f.ledger, at(T0)).unwrap().is_err());
    let settings = Settings::load(&f.ledger).unwrap();
    assert!(settings.last_error.unwrap().contains("isn't available"));
    assert_eq!(settings.last_at, None);
    fs::create_dir(&f.folder).unwrap();
    daily(&f.ledger, at(T0 + 10)).unwrap().unwrap();
    assert_eq!(Settings::load(&f.ledger).unwrap().last_error, None);
}

#[test]
fn errors_never_mention_ledger_contents() {
    let f = Fixture::new();
    put(&f.ledger, "note", "SECRET-MERCHANT-NAME");
    let err = back_up(&f.ledger, &f.root.join("gone"), at(T0)).unwrap_err();
    assert!(!err.to_string().contains("SECRET"));
}
