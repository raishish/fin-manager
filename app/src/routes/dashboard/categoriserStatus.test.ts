// Run with `npm test` (node --test).
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { changesLedger, statusText, type Progress } from './categoriserStatus.ts';

const p = (over: Partial<Progress>): Progress => ({
	running: false,
	total: 0,
	answered: 0,
	applied: 0,
	leftInReview: 0,
	stillWaiting: 0,
	...over
});

test('while running: how many are still being categorised', () => {
	assert.equal(
		statusText(p({ running: true, total: 120, answered: 40, stillWaiting: 80 })),
		'Categorising 80 transactions with the local model…'
	);
	assert.equal(
		statusText(p({ running: true, total: 1, stillWaiting: 1 })),
		'Categorising 1 transaction with the local model…'
	);
});

test('when done: filed and left for review', () => {
	assert.equal(
		statusText(p({ total: 120, answered: 120, applied: 97, leftInReview: 23 })),
		'Filed 97, 23 left for you to review'
	);
	assert.equal(statusText(p({})), null, 'a run with nothing to do says nothing');
});

test('lists reload when a batch is written or a run ends', () => {
	const start = p({ running: true, total: 50, stillWaiting: 50 });
	assert.equal(changesLedger(start, null), false, 'starting changes nothing yet');
	const batch = p({ running: true, total: 50, answered: 25, applied: 20, leftInReview: 5, stillWaiting: 25 });
	assert.equal(changesLedger(batch, start), true);
	assert.equal(changesLedger(batch, batch), false, 'the same counts again: nothing new');
	assert.equal(changesLedger(p({ answered: 50 }), batch), true, 'the end');
});

test('the status line never names a merchant or an amount', () => {
	const line = statusText(p({ total: 3, answered: 3, applied: 2, leftInReview: 1 })) ?? '';
	assert.match(line, /^Filed \d+, \d+ left for you to review$/);
});
