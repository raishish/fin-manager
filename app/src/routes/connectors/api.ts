// Typed wrappers for the SimpleFIN commands in app/src-tauri/src/connectors.rs.

import { invoke } from '@tauri-apps/api/core';

export type SimpleFinStatus = {
	connected: boolean;
	/** The bridge's origin, e.g. https://beta-bridge.simplefin.org. */
	bridge: string | null;
	/** Unix seconds up to which transactions have been fetched. */
	syncedTo: number | null;
	/** Unix seconds of the last update that reached SimpleFIN. */
	lastSync: number | null;
	requestsLeft: number;
	requestsPerDay: number;
	/** Unix seconds; set when today's requests are used up. */
	nextAllowed: number | null;
	/** SimpleFIN's own messages from the last update, shown as they are. */
	messages: string[];
};

export type TokenInfo = {
	/** host[:port] the setup token connects to. */
	host: string;
	/** True for SimpleFIN's own servers (simplefin.org). */
	simplefin: boolean;
};

export type SyncResult = {
	inserted: number;
	duplicates: number;
	postedFromPending: number;
	accountsCreated: number;
	messages: string[];
	/** False when today's requests ran out first; the next update carries on. */
	complete: boolean;
	syncedTo: number | null;
	requestsLeft: number;
	nextAllowed: number | null;
	backupError: string | null;
};

export type ConnectorError = {
	code: string;
	message: string;
	/** For QuotaReached: Unix seconds. */
	nextAllowed: number | null;
};

export function isConnectorError(e: unknown): e is ConnectorError {
	return typeof e === 'object' && e !== null && 'code' in e && 'message' in e;
}

/** `confirmedHost`: the host the user confirmed, when connecting. */
export function connectorErrorMessage(e: unknown, confirmedHost?: string): string {
	if (!isConnectorError(e)) return String(e);
	if (e.code === 'BridgeHostChanged' && confirmedHost) {
		return (
			`The bridge answered with a different host than the one you confirmed ` +
			`(${confirmedHost}), so nothing was saved. If you didn't expect this, disable the ` +
			`token in SimpleFIN.`
		);
	}
	if (e.code === 'QuotaReached' && e.nextAllowed) {
		return `${e.message} (after ${formatTime(e.nextAllowed)}).`;
	}
	return e.message;
}

export const simplefinStatus = () => invoke<SimpleFinStatus>('simplefin_status');
export const simplefinInspectToken = (setupToken: string) =>
	invoke<TokenInfo>('simplefin_inspect_token', { setupToken });
export const simplefinConnect = (setupToken: string, confirmedHost: string) =>
	invoke<SimpleFinStatus>('simplefin_connect', { setupToken, confirmedHost });
export const simplefinSync = () => invoke<SyncResult>('simplefin_sync');
export const simplefinDisconnect = () => invoke<SimpleFinStatus>('simplefin_disconnect');

export function formatTime(unixSeconds: number): string {
	return new Date(unixSeconds * 1000).toLocaleString();
}
