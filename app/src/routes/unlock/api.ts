// Typed wrappers for the unlock commands in app/src-tauri/src/unlock.rs.

import { invoke } from '@tauri-apps/api/core';

export type Status =
	| 'NoLedger'
	| 'Incomplete'
	| 'Unrecoverable'
	| 'Locked'
	| 'NeedsNewPassword'
	| 'ConfirmRecoveryKey'
	| 'Unlocked';

export type UnlockState = {
	status: Status;
	/** Null when Touch ID may be offered; otherwise why not. */
	touchIdBlocked: string | null;
	/** True once the user has a recovery key; null while locked. */
	hasRecoveryKey: boolean | null;
	autolockMinutes: number;
	autolockChoices: number[];
};

export type Strength = {
	score: number;
	chars: number;
	acceptable: boolean;
	warning: string | null;
	suggestions: string[];
	minChars: number;
};

/** A recovery key to show once, and the zero-based groups to type back. */
export type RecoveryToConfirm = {
	groups: string[];
	challenge: number[];
};

export type UnlockErrorCode =
	| 'WeakPassword'
	| 'WrongSecret'
	| 'RecoveryKey'
	| 'ConfirmationMismatch'
	| 'TouchIdBlocked'
	| 'TouchId'
	| 'WrongState'
	| 'Storage';

export type UnlockError = {
	code: UnlockErrorCode;
	message: string;
	strength?: Strength;
};

export function isUnlockError(e: unknown): e is UnlockError {
	return typeof e === 'object' && e !== null && 'code' in e && 'message' in e;
}

export function errorMessage(e: unknown): string {
	return isUnlockError(e) ? e.message : String(e);
}

export const unlockState = () => invoke<UnlockState>('unlock_state');
export const passwordStrength = (password: string) =>
	invoke<Strength>('password_strength', { password });
export const createLedger = (password: string) =>
	invoke<RecoveryToConfirm>('create_ledger', { password });
export const finishCreate = (password: string) =>
	invoke<RecoveryToConfirm | null>('finish_create', { password });
export const unlockWithPassword = (password: string) =>
	invoke<RecoveryToConfirm | null>('unlock_with_password', { password });
export const unlockWithRecoveryKey = (recoveryKey: string) =>
	invoke<void>('unlock_with_recovery_key', { recoveryKey });
export const setPasswordAfterRecovery = (password: string, compromised: boolean) =>
	invoke<RecoveryToConfirm>('set_password_after_recovery', { password, compromised });
export const unlockWithTouchId = () => invoke<void>('unlock_with_touch_id');
export const recoveryToConfirm = () => invoke<RecoveryToConfirm | null>('recovery_to_confirm');
export const confirmRecoveryKey = (answers: string[]) =>
	invoke<void>('confirm_recovery_key', { answers });
export const changePassword = (current: string, password: string, compromised: boolean) =>
	invoke<RecoveryToConfirm | null>('change_password', { current, password, compromised });
export const setAutolockMinutes = (minutes: number) =>
	invoke<void>('set_autolock_minutes', { minutes });
export const lockNow = () => invoke<void>('lock');
export const reportActivity = () => invoke<void>('activity');

/** Emitted by the app when auto-lock closes the ledger. */
export const LOCKED_EVENT = 'unlock://locked';
