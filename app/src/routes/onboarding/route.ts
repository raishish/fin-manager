// Where the root layout sends a page: T5.1's lock guard plus first run (T5.5). Pure, so
// `node --test` checks it (route.test.ts).

import { worksWhileLocked } from '../unlock/locked.ts';

/**
 * The page to go to instead of `path`, or `null` to stay. `status` is the unlock state
 * (`unlock_state`); `onboardingDone` is known only once the ledger is open.
 */
export function routeFor(
	path: string,
	status: string,
	onboardingDone: boolean | null
): string | null {
	if (path === '/onboarding' || path.startsWith('/onboarding/')) {
		// Welcome comes before the ledger exists, the steps after it is open; a ledger
		// that is locked (or half set up) goes through /unlock first.
		return status === 'NoLedger' || status === 'Unlocked' ? null : '/unlock';
	}
	// /unlock (create, unlock, recovery) and backup restore work without a ledger.
	if (worksWhileLocked(path)) return null;
	if (status === 'NoLedger') return '/onboarding';
	if (status !== 'Unlocked') return '/unlock';
	if (onboardingDone === false) return '/onboarding';
	return null;
}
