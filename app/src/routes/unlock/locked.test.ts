// Run with `npm test` (node --test; Node runs TypeScript directly).
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { guardInvoke, isLockedError, worksWhileLocked } from './locked.ts';

test('a Locked answer is recognised in both shapes', () => {
	assert.equal(isLockedError('locked'), true);
	assert.equal(isLockedError({ code: 'Locked', message: 'the ledger is locked' }), true);
	assert.equal(isLockedError({ code: 'NotFound', message: 'x' }), false);
	assert.equal(isLockedError('some other error'), false);
	assert.equal(isLockedError(null), false);
});

test('a Locked answer redirects, and the caller still gets the error', async () => {
	let redirects = 0;
	const failing = async (_cmd: string) => {
		throw 'locked';
	};
	const call = guardInvoke(failing, () => redirects++, () => '/dashboard');
	await assert.rejects(call('dashboard'), (e) => e === 'locked');
	assert.equal(redirects, 1);
});

test('other errors and successes leave the page alone', async () => {
	let redirects = 0;
	const onLocked = () => redirects++;
	const ok = guardInvoke(async (_cmd: string) => 42, onLocked, () => '/dashboard');
	assert.equal(await ok('x'), 42);
	const other = guardInvoke(
		async (_cmd: string) => {
			throw { code: 'NotFound', message: 'no' };
		},
		onLocked,
		() => '/dashboard'
	);
	await assert.rejects(other('x'));
	assert.equal(redirects, 0);
});

test('pages that work while locked are not redirected', async () => {
	let redirects = 0;
	for (const path of ['/unlock', '/unlock/', '/backup/restore']) {
		const call = guardInvoke(
			async (_cmd: string) => {
				throw 'locked';
			},
			() => redirects++,
			() => path
		);
		await assert.rejects(call('x'));
	}
	assert.equal(redirects, 0);
	assert.equal(worksWhileLocked('/unlock/settings'), false, 'the security settings need a ledger');
	assert.equal(worksWhileLocked('/review'), false);
});
