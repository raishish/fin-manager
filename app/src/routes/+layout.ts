import { redirect } from '@sveltejs/kit';
import { needsUnlock } from './unlock/guard';

// The app runs inside Tauri with no server: render on the client only. The static
// adapter's fallback page serves every route.
export const ssr = false;

// Every route except /unlock needs an open ledger (SPEC §7, T5.1).
export async function load({ url }) {
	if (await needsUnlock(url.pathname)) redirect(307, '/unlock');
}
