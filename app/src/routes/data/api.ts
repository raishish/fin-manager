// Typed wrappers for the deletion commands in app/src-tauri/src/deletion.rs.

import { invoke } from '@tauri-apps/api/core';

export type AccountChoice = {
	id: number;
	name: string;
	institution: string | null;
	transactions: number;
};

export type FileChoice = {
	id: number;
	fileName: string | null;
	/** Unix seconds. */
	importedAt: number;
	transactions: number;
};

export type DataChoices = { accounts: AccountChoice[]; files: FileChoice[] };

export type Target =
	| { kind: 'import'; id: number }
	| { kind: 'account'; id: number }
	| { kind: 'range'; account: number | null; from: string; to: string };

export type Counts = {
	transactions: number;
	decisions: number;
	rules: number;
	imports: number;
	accounts: number;
	/** Rows deleted out of files that are kept, recorded so they never come back. */
	recordedRows: number;
};

export type PartialFile = {
	id: number;
	fileName: string | null;
	inScope: number;
	outOfScope: number;
	/** Belongs to the account being deleted, so it goes whole regardless. */
	forced: boolean;
};

export type Preview = { counts: Counts; partialFiles: PartialFile[] };

export type BackupOffer = { count: number; folder: string; message: string };

export type Deleted = { counts: Counts; backupOffer: BackupOffer | null };

/** The phrase typed to confirm "delete everything". */
export const CONFIRMATION = 'delete everything';

export const dataChoices = () => invoke<DataChoices>('data_choices');
export const previewDeletion = (target: Target, deletePartialFiles: boolean) =>
	invoke<Preview>('preview_deletion', { target, deletePartialFiles });
export const deleteData = (target: Target, deletePartialFiles: boolean) =>
	invoke<Deleted>('delete_data', { target, deletePartialFiles });
export const deleteEverything = (confirmation: string, allowUnverifiedKeychain: boolean) =>
	invoke<BackupOffer | null>('delete_everything', { confirmation, allowUnverifiedKeychain });
export const deleteOldBackups = () => invoke<number>('delete_old_backups');
export const pendingBackupOffer = () => invoke<BackupOffer | null>('pending_backup_offer');
