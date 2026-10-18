import { redirect } from '@sveltejs/kit';
import { onboardingState } from './onboarding/api';
import { routeFor } from './onboarding/route';
import { unlockState } from './unlock/api';

// The app runs inside Tauri with no server: render on the client only. The static
// adapter's fallback page serves every route.
export const ssr = false;

// Every route except /unlock needs an open ledger (SPEC §7, T5.1); a new ledger goes
// through first run until it is done (SPEC decision 34, T5.5). The rules: onboarding/route.ts.
export async function load({ url }) {
	const { status } = await unlockState();
	const done =
		status === 'Unlocked'
			? await onboardingState()
					.then((o) => o.done)
					.catch(() => null)
			: null;
	const target = routeFor(url.pathname, status, done);
	if (target && target !== url.pathname) redirect(307, target);
}
