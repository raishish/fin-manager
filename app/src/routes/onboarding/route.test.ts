// Run with `npm test` (node --test).
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { routeFor } from './route.ts';

test('first launch: Welcome first, then the create flow', () => {
	assert.equal(routeFor('/', 'NoLedger', null), '/onboarding');
	assert.equal(routeFor('/dashboard', 'NoLedger', null), '/onboarding');
	assert.equal(routeFor('/onboarding', 'NoLedger', null), null);
	assert.equal(routeFor('/unlock', 'NoLedger', null), null, "Welcome's Continue goes here");
	assert.equal(routeFor('/backup/restore', 'NoLedger', null), null, 'restore on a fresh install');
});

test("an open ledger whose onboarding isn't done goes back to it", () => {
	assert.equal(routeFor('/', 'Unlocked', false), '/onboarding');
	assert.equal(routeFor('/review', 'Unlocked', false), '/onboarding');
	assert.equal(routeFor('/onboarding', 'Unlocked', false), null);
	assert.equal(routeFor('/', 'Unlocked', true), null);
	assert.equal(routeFor('/review', 'Unlocked', true), null);
});

test("T5.1's lock guard is unchanged", () => {
	for (const status of ['Locked', 'Incomplete', 'NeedsNewPassword', 'ConfirmRecoveryKey']) {
		assert.equal(routeFor('/dashboard', status, null), '/unlock', status);
		assert.equal(routeFor('/onboarding', status, null), '/unlock', `${status}: unlock first`);
		assert.equal(routeFor('/unlock', status, null), null);
		assert.equal(routeFor('/unlock/settings', status, null), '/unlock', 'settings need a ledger');
	}
	assert.equal(routeFor('/unlock/settings', 'Unlocked', true), null);
});
