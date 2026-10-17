// Typed wrappers for the backup commands in app/src-tauri/src/backup.rs.

import { invoke } from '@tauri-apps/api/core';

export type Backup = {
	fileName: string;
	/** False for a backup of another ledger sharing the folder: shown, never touched. */
	mine: boolean;
	/** Unix seconds, UTC. */
	createdAt: number;
	bytes: number;
};

export type BackupStatus = {
	folder: string | null;
	lastAt: number | null;
	lastError: string | null;
	/** The last backup was made, but something after it went wrong. */
	lastWarning: string | null;
	/** Newest first, this ledger's and others'. */
	backups: Backup[];
	/** Set when the folder can't be read right now (e.g. an unplugged drive). */
	folderError: string | null;
};

export const backupStatus = () => invoke<BackupStatus>('backup_status');
export const setBackupFolder = (folder: string) => invoke<void>('set_backup_folder', { folder });
export const backupNow = () => invoke<Backup>('backup_now');
export const restoreBackup = (file: string) => invoke<void>('restore_backup', { file });

export function formatTime(unixSeconds: number): string {
	return new Date(unixSeconds * 1000).toLocaleString();
}

export function formatBytes(bytes: number): string {
	if (bytes < 1024 * 1024) return `${Math.max(1, Math.round(bytes / 1024))} KB`;
	return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}
