// App-wide unlock behaviour, installed once by the root layout:
// - every route except /unlock needs an unlocked ledger;
// - user activity restarts the auto-lock timer (reported at most every 15 s);
// - when the core auto-locks, the app returns to /unlock.

import { goto } from '$app/navigation';
import { listen } from '@tauri-apps/api/event';
import { LOCKED_EVENT, reportActivity, unlockState } from './api';

const ACTIVITY_EVENTS = ['pointerdown', 'keydown', 'wheel', 'touchstart'] as const;
const ACTIVITY_INTERVAL_MS = 15_000;

/** True if `path` may not be shown now: every route except /unlock needs an open ledger. */
export async function needsUnlock(path: string): Promise<boolean> {
	if (path === '/unlock' || path.startsWith('/unlock/')) {
		// The security settings page needs an open ledger too.
		if (!path.startsWith('/unlock/settings')) return false;
	}
	const { status } = await unlockState();
	return status !== 'Unlocked';
}

/** Starts activity reporting and the auto-lock redirect; returns a cleanup function. */
export function installUnlockGuard(): () => void {
	let last = 0;
	const onActivity = () => {
		const now = Date.now();
		if (now - last < ACTIVITY_INTERVAL_MS) return;
		last = now;
		reportActivity().catch(() => {});
	};
	for (const name of ACTIVITY_EVENTS) window.addEventListener(name, onActivity, { passive: true });
	const unlisten = listen(LOCKED_EVENT, () => goto('/unlock'));
	return () => {
		for (const name of ACTIVITY_EVENTS) window.removeEventListener(name, onActivity);
		unlisten.then((stop) => stop());
	};
}
