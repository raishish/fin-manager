// Listening for the model categoriser's progress (T4.8). The event carries counts only
// (categoriserStatus.ts); this module only subscribes.

import { listen } from '@tauri-apps/api/event';
import { PROGRESS_EVENT, changesLedger, type Progress } from './categoriserStatus';

/** Calls `f` with every progress event; returns a function that stops listening. */
export function onProgress(f: (p: Progress) => void): () => void {
	const stop = listen<Progress>(PROGRESS_EVENT, (e) => f(e.payload));
	return () => {
		stop.then((unlisten) => unlisten()).catch(() => {});
	};
}

/**
 * Calls `reload` whenever the categoriser wrote to the ledger (a batch, or the end of a
 * run), so a list or total shows the model's work without a manual refresh.
 */
export function reloadOnProgress(reload: () => void): () => void {
	let previous: Progress | null = null;
	return onProgress((p) => {
		const changed = changesLedger(p, previous);
		previous = p.running ? p : null;
		if (changed) reload();
	});
}
