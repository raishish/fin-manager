// App-wide unlock behaviour, installed once by the root layout:
// - every route except /unlock needs an unlocked ledger;
// - user activity restarts the auto-lock timer (reported at most every 15 s);
// - when the core auto-locks, the app returns to /unlock: on the core's lock event, and
//   (T5.12) whenever any command answers "locked", so the screen clears even if the event
//   is ever missed.

import { goto } from '$app/navigation';
import { listen } from '@tauri-apps/api/event';
import { LOCKED_EVENT, reportActivity, unlockState } from './api';
import { guardInvoke, worksWhileLocked } from './locked';

/** Tauri's IPC hook: every command call from @tauri-apps/api goes through it. */
type Internals = { invoke: (...args: never[]) => Promise<unknown> };

const ACTIVITY_EVENTS = ['pointerdown', 'keydown', 'wheel', 'touchstart'] as const;
const ACTIVITY_INTERVAL_MS = 15_000;

/** True if `path` may not be shown now: every route except /unlock needs an open ledger. */
export async function needsUnlock(path: string): Promise<boolean> {
	// Restoring a backup happens on a fresh install, before there is anything to unlock.
	// Only this exact page; /backup itself (the settings) needs an open ledger.
	// /unlock (but not its security settings) and backup restore work without a ledger.
	if (worksWhileLocked(path)) return false;
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
	// Defence in depth: a "locked" answer to any command clears the screen too.
	const internals = (window as unknown as { __TAURI_INTERNALS__?: Internals }).__TAURI_INTERNALS__;
	const original = internals?.invoke;
	if (internals && original) {
		internals.invoke = guardInvoke(
			original,
			() => void goto('/unlock'),
			() => window.location.pathname
		);
	}
	return () => {
		for (const name of ACTIVITY_EVENTS) window.removeEventListener(name, onActivity);
		unlisten.then((stop) => stop());
		if (internals && original) internals.invoke = original;
	};
}
