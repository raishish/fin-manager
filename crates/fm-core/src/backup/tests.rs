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
const HOUR: u64 = 3_600;
const DAY: u64 = 86_400;

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
        let ledger = new_ledger(&root.join("ledger"));
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

    fn id(&self) -> String {
        ledger_id(&self.ledger).unwrap()
    }
}

fn new_ledger(dir: &Path) -> Unlocked {
    let mut ledger = Vault::at(dir).create_with_params(PASSWORD, P).unwrap();
    ledger
        .set_recovery_key_with_params(PASSWORD, RECOVERY, P)
        .unwrap();
    put(&ledger, "note", "before the backup");
    ledger
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

fn backup_at(f: &Fixture, secs: u64) -> BackupInfo {
    back_up(&f.ledger, &f.folder, at(secs)).unwrap().info
}

/// Makes deleting backup files fail while `f` runs (a share that allows creating but
/// not deleting).
fn with_undeletable_files<T>(f: impl FnOnce() -> T) -> T {
    FAIL_REMOVE.with(|c| c.set(true));
    let out = f();
    FAIL_REMOVE.with(|c| c.set(false));
    out
}

// ---- done when: restore round trip with the password and with the recovery key -------

#[test]
fn restore_round_trip_with_the_password_and_with_the_recovery_key() {
    let f = Fixture::new();
    let backup = backup_at(&f, T0);
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
    let backup = backup_at(&f, T0);
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
    let backup = backup_at(&f, T0);
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
    let backup = backup_at(&f, T0);
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
    let backup = backup_at(&f, T0);
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
fn restore_keeps_the_lock_file_and_works_in_an_empty_folder() {
    let f = Fixture::new();
    let backup = backup_at(&f, T0);
    let vault = f.fresh_vault("empty");
    fs::create_dir(vault.dir()).unwrap();
    fs::write(vault.dir().join("ledger.lock"), b"").unwrap();
    restore(&backup.path, &vault).unwrap();
    assert!(vault.dir().join("ledger.lock").exists(), "never deleted");
    vault.unlock(PASSWORD).unwrap();
}

/// Review finding 2: restore takes the same lock as storage. While another instance
/// holds it, restore refuses; while restore holds it, storage can't create a ledger.
#[test]
fn restore_and_other_instances_exclude_each_other() {
    let f = Fixture::new();
    let backup = backup_at(&f, T0);
    let vault = f.fresh_vault("contended");

    let held = vault.lock_for_restore().unwrap(); // another instance, mid-create
    assert!(matches!(
        vault.create_with_params(PASSWORD, P),
        Err(StorageError::InUse)
    ));
    assert!(matches!(
        restore(&backup.path, &vault),
        Err(BackupError::Storage(StorageError::InUse))
    ));
    assert_eq!(vault.state(), VaultState::Missing);
    assert!(vault.dir().join("ledger.lock").exists());
    drop(held);

    restore(&backup.path, &vault).unwrap();
    vault.unlock(PASSWORD).unwrap();
}

#[test]
fn a_damaged_backup_restores_nothing() {
    let f = Fixture::new();
    let backup = backup_at(&f, T0);
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
        assert_eq!(vault.state(), VaultState::Missing, "no ledger left behind");
        let leftovers: Vec<String> = names(&f.root)
            .into_iter()
            .filter(|n| n.contains("restoring"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }
}

// Review finding 4 (a FIFO is refused at once instead of hanging) is tested in
// tests/backup_fifo.rs: it starts a process, which must not happen in this binary
// (see that file).

/// Review finding 7: staging folders left by an interrupted restore are cleaned up.
#[test]
fn stale_restore_folders_are_cleaned_up() {
    let f = Fixture::new();
    let backup = backup_at(&f, T0);
    let stale = f.root.join(".target.restoring-1000");
    let fresh = f.root.join(".target.restoring-9999999999");
    for dir in [&stale, &fresh] {
        fs::create_dir(dir).unwrap();
        fs::write(dir.join("ledger.db"), b"partial").unwrap();
    }
    let old = SystemTime::now() - Duration::from_secs(HOUR);
    fs::File::open(&stale).unwrap().set_modified(old).unwrap();
    restore(&backup.path, &f.fresh_vault("target")).unwrap();
    assert!(!stale.exists(), "a stale staging folder is removed");
    assert!(fresh.exists(), "one that may still be in use is kept");
}

// ---- making backups -------------------------------------------------------------------

#[test]
fn a_backup_is_a_snapshot() {
    let f = Fixture::new();
    let backup = backup_at(&f, T0);
    assert_eq!(
        backup.file_name,
        format!("fin-manager-{}-20261016T120000Z.fmbackup", f.id())
    );
    assert_eq!(list(&f.folder).unwrap(), vec![backup.clone()]);
    put(&f.ledger, "note", "later");
    assert_eq!(get(&f.ledger, "note").as_deref(), Some("later"));
}

#[test]
fn two_backups_in_the_same_second_both_survive() {
    let f = Fixture::new();
    let a = backup_at(&f, T0);
    let b = backup_at(&f, T0);
    assert_ne!(a.path, b.path);
    assert!(b.file_name.ends_with("-20261016T120000Z-1.fmbackup"));
}

/// Review finding 6: moving into place never replaces a file that appeared meanwhile.
#[test]
fn moving_into_place_never_replaces() {
    let f = Fixture::new();
    let (from, to) = (f.root.join("from"), f.root.join("to"));
    fs::write(&from, b"new").unwrap();
    fs::write(&to, b"was here first").unwrap();
    let err = move_no_replace(&from, &to).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
    assert_eq!(fs::read(&to).unwrap(), b"was here first");
    assert_eq!(fs::read(&from).unwrap(), b"new");
    fs::remove_file(&to).unwrap();
    move_no_replace(&from, &to).unwrap();
    assert_eq!(fs::read(&to).unwrap(), b"new");
    assert!(!from.exists());
}

/// T3.6: the post-import backup can be stopped too; it leaves nothing and records no
/// failure, so the next one runs normally.
#[test]
fn a_cancelled_post_import_backup_leaves_nothing() {
    let f = Fixture::new();
    set_folder(&f.ledger, &f.folder).unwrap();
    assert!(matches!(
        after_import_cancellable(&f.ledger, at(T0), &|| true),
        Some(Err(BackupError::Interrupted))
    ));
    assert!(names(&f.folder).is_empty(), "{:?}", names(&f.folder));
    let settings = Settings::load(&f.ledger).unwrap();
    assert_eq!(settings.last_error, None);
    assert_eq!(settings.last_attempt, None, "not counted as an attempt");
    after_import_cancellable(&f.ledger, at(T0 + 5), &|| false)
        .unwrap()
        .unwrap();
    assert_eq!(names(&f.folder).len(), 1);
}

/// Review finding 5: a backup can be stopped (the app is locking) and leaves nothing.
#[test]
fn a_cancelled_backup_leaves_nothing() {
    let f = Fixture::new();
    let err = back_up_cancellable(&f.ledger, &f.folder, at(T0), &|| true).unwrap_err();
    assert!(matches!(err, BackupError::Interrupted));
    assert!(names(&f.folder).is_empty(), "{:?}", names(&f.folder));
    // Not counted as a failure: no back-off.
    set_folder(&f.ledger, &f.folder).unwrap();
    assert!(matches!(
        daily(&f.ledger, at(T0), &|| true),
        Some(Err(BackupError::Interrupted))
    ));
    assert!(daily(&f.ledger, at(T0 + 5), &|| false).unwrap().is_ok());
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
    back_up(&f.ledger, &f.root, at(T0)).unwrap();
}

#[test]
fn stale_temporary_files_are_ignored_and_cleaned_up() {
    let f = Fixture::new();
    let id = f.id();
    let stale = f.folder.join(name::tmp_name(&id, 1_790_000_000, 0));
    let fresh = f.folder.join(name::tmp_name(&id, 1_792_151_999, 0));
    for p in [&stale, &fresh] {
        fs::write(p, b"half a backup").unwrap();
    }
    let old = SystemTime::now() - Duration::from_secs(HOUR);
    fs::File::options()
        .write(true)
        .open(&stale)
        .unwrap()
        .set_modified(old)
        .unwrap();
    assert!(list(&f.folder).unwrap().is_empty(), "list ignores them");
    assert!(
        rotate(&f.folder, &id).unwrap().is_empty(),
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
    let id = f.id();
    let real = backup_at(&f, T0);
    let content = fs::read(&real.path).unwrap();
    for d in 1..400u64 {
        let t = i64::try_from(T0 - d * DAY).unwrap();
        fs::write(f.folder.join(name::backup_name(&id, t, 0)), &content).unwrap();
    }
    let others = [
        "notes.txt",
        "fin-manager-20261016T120000Z.fmbackup",
        "copy of fin-manager-0a1b2c3d4e-20251016T120000Z.fmbackup",
        "fin-manager-0a1b2c3d4e-2025-01-01.fmbackup",
    ];
    for other in others {
        fs::write(f.folder.join(other), b"not ours").unwrap();
    }
    let look_alike_dir = f.folder.join(name::backup_name(&id, 1_700_000_000, 7));
    fs::create_dir(&look_alike_dir).unwrap();

    rotate(&f.folder, &id).unwrap();

    let kept = list_mine(&f.folder, &id).unwrap();
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
    assert!(look_alike_dir.is_dir());
}

/// Review finding 3: two ledgers sharing a folder (two Macs on one Dropbox folder)
/// never rotate or delete each other's backups.
#[test]
fn ledgers_sharing_a_folder_leave_each_other_alone() {
    let f = Fixture::new();
    let other = new_ledger(&f.root.join("other-mac"));
    let (mine, theirs) = (f.id(), ledger_id(&other).unwrap());
    assert_ne!(mine, theirs);
    for d in 0..30u64 {
        back_up(&other, &f.folder, at(T0 - d * DAY)).unwrap();
    }
    let theirs_before = list_mine(&f.folder, &theirs).unwrap();
    for d in 0..30u64 {
        backup_at(&f, T0 - d * DAY + 60);
    }
    assert_eq!(list_mine(&f.folder, &theirs).unwrap(), theirs_before);
    assert_eq!(
        list(&f.folder).unwrap().len(),
        2 * list_mine(&f.folder, &mine).unwrap().len()
    );

    assert_eq!(delete_all(&f.folder, &mine).unwrap(), theirs_before.len());
    assert!(list_mine(&f.folder, &mine).unwrap().is_empty());
    assert_eq!(list_mine(&f.folder, &theirs).unwrap(), theirs_before);
}

#[test]
fn a_restored_ledger_can_take_a_new_id() {
    let f = Fixture::new();
    let backup = backup_at(&f, T0);
    let vault = f.fresh_vault("restored");
    restore(&backup.path, &vault).unwrap();
    let restored = vault.unlock(PASSWORD).unwrap();
    assert_eq!(
        ledger_id(&restored).unwrap(),
        f.id(),
        "the same until told otherwise"
    );
    let new_id = reassign_ledger_id(&restored).unwrap();
    assert_ne!(new_id, f.id());
    assert_eq!(ledger_id(&restored).unwrap(), new_id);
}

#[test]
fn delete_all_removes_only_this_ledgers_backups() {
    let f = Fixture::new();
    backup_at(&f, T0);
    backup_at(&f, T0 + DAY);
    fs::write(f.folder.join("notes.txt"), b"mine").unwrap();
    assert_eq!(delete_all(&f.folder, &f.id()).unwrap(), 2);
    assert_eq!(names(&f.folder), ["notes.txt"]);
}

// ---- settings and schedule ------------------------------------------------------------

#[test]
fn nothing_happens_until_a_folder_is_chosen() {
    let f = Fixture::new();
    assert_eq!(Settings::load(&f.ledger).unwrap(), Settings::default());
    assert!(daily(&f.ledger, at(T0), &never).is_none());
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
    let first = daily(&f.ledger, at(T0), &never).unwrap().unwrap();
    let settings = Settings::load(&f.ledger).unwrap();
    assert_eq!(settings.last_at, Some(first.info.created_unix));
    assert_eq!(settings.folder.as_deref(), Some(f.folder.as_path()));
    assert!(daily(&f.ledger, at(T0 + HOUR), &never).is_none());
    assert!(daily(&f.ledger, at(T0 + DAY), &never).is_some());
    assert!(
        daily(&f.ledger, at(T0 - HOUR), &never).is_some(),
        "clock set back"
    );
}

#[test]
fn after_an_import_a_backup_is_always_made() {
    let f = Fixture::new();
    set_folder(&f.ledger, &f.folder).unwrap();
    daily(&f.ledger, at(T0), &never).unwrap().unwrap();
    after_import(&f.ledger, at(T0 + 60)).unwrap().unwrap();
    assert_eq!(
        list(&f.folder).unwrap().len(),
        1,
        "same day: the newest is kept"
    );
}

/// Review finding 1: a folder where old backups can't be deleted still gets one backup
/// a day, with a warning, not one every few seconds until the disk fills.
#[test]
fn a_rotation_failure_is_a_warning_not_a_retry_loop() {
    let f = Fixture::new();
    set_folder(&f.ledger, &f.folder).unwrap();
    // An earlier backup today, which the next rotation will want to delete.
    backup_at(&f, T0);
    let made = with_undeletable_files(|| {
        let made = daily(&f.ledger, at(T0 + 120), &never).unwrap().unwrap();
        // The app's timer keeps ticking every 5 s: nothing more is due.
        for tick in 1..=100u64 {
            assert!(daily(&f.ledger, at(T0 + 120 + tick * 5), &never).is_none());
        }
        made
    });
    assert!(made.warning.is_some());
    let settings = Settings::load(&f.ledger).unwrap();
    assert_eq!(settings.last_at, Some(made.info.created_unix));
    assert!(
        settings
            .last_warning
            .unwrap()
            .contains("couldn't be removed")
    );
    assert_eq!(settings.last_error, None);
    assert_eq!(list(&f.folder).unwrap().len(), 2, "exactly one new backup");
    // The next success, with deleting possible again, clears the warning.
    daily(&f.ledger, at(T0 + DAY + 180), &never)
        .unwrap()
        .unwrap();
    assert_eq!(Settings::load(&f.ledger).unwrap().last_warning, None);
}

/// Review finding 1: after any failure, automatic backups wait an hour.
#[test]
fn failures_back_off_for_an_hour() {
    let f = Fixture::new();
    set_folder(&f.ledger, &f.folder).unwrap();
    fs::remove_dir(&f.folder).unwrap(); // the drive was unplugged
    assert!(daily(&f.ledger, at(T0), &never).unwrap().is_err());
    let settings = Settings::load(&f.ledger).unwrap();
    assert!(settings.last_error.unwrap().contains("isn't available"));
    assert_eq!(settings.last_at, None);
    assert_eq!(settings.last_attempt, Some(i64::try_from(T0).unwrap()));

    fs::create_dir(&f.folder).unwrap(); // back, but no retry for an hour
    for tick in 1..=(HOUR / 5 - 1) {
        assert!(
            daily(&f.ledger, at(T0 + tick * 5), &never).is_none(),
            "tick {tick}"
        );
    }
    daily(&f.ledger, at(T0 + HOUR), &never).unwrap().unwrap();
    assert_eq!(Settings::load(&f.ledger).unwrap().last_error, None);
    // "Back up now" and a new folder don't wait.
    fs::remove_dir_all(&f.folder).unwrap();
    assert!(back_up_now(&f.ledger, at(T0 + 2 * HOUR)).is_err());
    fs::create_dir(&f.folder).unwrap();
    back_up_now(&f.ledger, at(T0 + 2 * HOUR + 1)).unwrap();
}

#[test]
fn errors_never_mention_ledger_contents() {
    let f = Fixture::new();
    put(&f.ledger, "note", "SECRET-MERCHANT-NAME");
    let err = back_up(&f.ledger, &f.root.join("gone"), at(T0)).unwrap_err();
    assert!(!err.to_string().contains("SECRET"));
}
