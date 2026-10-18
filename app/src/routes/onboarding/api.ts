// Typed wrappers for the first-run commands (T5.5): app/src-tauri/src/{model,onboarding}.rs.

import { invoke } from '@tauri-apps/api/core';
import { localToday, type Line, type Money, type Recurring } from '../dashboard/ledger';

export type ModelStatus =
	| { state: 'missing'; modelId: string; total: number }
	| { state: 'downloading'; modelId: string; bytes: number; total: number }
	| { state: 'ready'; modelId: string }
	| { state: 'failed'; modelId: string; code: string; message: string };

/** The steps after the ledger exists, in order (app/src-tauri/src/onboarding.rs STEPS). */
export const STEPS = ['data', 'insight', 'review', 'backup', 'ask', 'done'] as const;
export type Step = (typeof STEPS)[number];
export type Onboarding = { step: Step; done: boolean };

export type FlowMonth = {
	month: string;
	partial: boolean;
	moneyIn: Line;
	moneyOut: Line;
	transfersIn: Line;
	transfersOut: Line;
	pending: Line;
	netFlow: Money;
};
export type MerchantOut = {
	label: string;
	fromDescription: boolean;
	moneyOut: Money;
	count: number;
	transactionIds: number[];
};
export type Insight = {
	empty: boolean;
	currency: string | null;
	firstMonth: string | null;
	lastMonth: string | null;
	months: FlowMonth[];
	moneyIn: Line | null;
	moneyOut: Line | null;
	transfersOut: Line | null;
	pending: Line | null;
	merchants: MerchantOut[];
	merchantsOther: MerchantOut | null;
	recurring: Recurring[];
	transferPairs: number;
	transactions: number;
};

export type ReminderReason = 'noFolder' | 'failed' | 'noBackupYet' | 'stale';
export type Reminder = { reason: ReminderReason | null; snoozedUntil: number | null };

export const modelStatus = () => invoke<ModelStatus>('model_status');
/** Starts the model download in the background; does nothing if it is running or done. */
export const startModelDownload = () => invoke<ModelStatus>('start_model_download');
export const onboardingState = () => invoke<Onboarding>('onboarding_state');
export const setOnboardingStep = (step: Step) =>
	invoke<Onboarding>('set_onboarding_step', { step });
export const firstInsight = () => invoke<Insight>('first_insight', { today: localToday() });
export const backupReminder = () => invoke<Reminder>('backup_reminder');
export const snoozeBackupReminder = () => invoke<Reminder>('snooze_backup_reminder');
