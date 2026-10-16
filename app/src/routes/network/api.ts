// Typed wrappers around the Tauri commands behind the network log and crash-report pages.
import { invoke } from '@tauri-apps/api/core';

export type NetworkOutcome =
	| { kind: 'completed'; status: number; bytes: number }
	| { kind: 'blocked'; reason: string }
	| { kind: 'rejected'; reason: string }
	| { kind: 'failed'; error: string };

export type NetworkLogEntry = {
	id: number;
	atUnixMs: number;
	method: string;
	origin: string;
	path: string;
	queryOmitted: boolean;
	purpose: string;
	redirectOf: number | null;
	outcome: NetworkOutcome;
};

export type CrashSettings = { enabled: boolean; dir: string };
export type SavedReport = { id: string; path: string };

/** Entries newer than `sinceId` (all of them when omitted), oldest first. */
export const networkLog = (sinceId?: number) =>
	invoke<NetworkLogEntry[]>('network_log', { sinceId: sinceId ?? null });

export const networkAllowlist = () => invoke<string[]>('network_allowlist');

export const crashSettings = () => invoke<CrashSettings>('crash_settings');
export const setCrashReportsEnabled = (enabled: boolean) =>
	invoke<CrashSettings>('set_crash_reports_enabled', { enabled });
export const crashReports = () => invoke<SavedReport[]>('crash_reports');
/** The report file's exact contents. */
export const crashReport = (id: string) => invoke<string>('crash_report', { id });
export const deleteCrashReport = (id: string) => invoke<void>('delete_crash_report', { id });

/** Whether anything may have left the machine: blocked and rejected requests sent nothing. */
export const wasSent = (outcome: NetworkOutcome) =>
	outcome.kind === 'completed' || outcome.kind === 'failed';

export function describeOutcome(outcome: NetworkOutcome): string {
	switch (outcome.kind) {
		case 'completed':
			return `${outcome.status} · ${formatBytes(outcome.bytes)}`;
		case 'blocked':
			return `Blocked: ${outcome.reason}`;
		case 'rejected':
			return `Rejected: ${outcome.reason}`;
		case 'failed':
			return `Failed: ${outcome.error}`;
	}
}

function formatBytes(bytes: number): string {
	if (bytes < 1024) return `${bytes} B`;
	if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
	return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}
