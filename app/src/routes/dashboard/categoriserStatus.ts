// The model categoriser's progress (T4.8): what `categoriser://progress` carries and the
// status line it becomes. Counts only, never names or amounts. Pure, so `node --test`
// checks it (categoriserStatus.test.ts).

/** The event the app's categoriser emits (app/src-tauri/src/categoriser.rs). */
export const PROGRESS_EVENT = 'categoriser://progress';

export type Progress = {
	/** A run is going on. */
	running: boolean;
	/** Transactions waiting for the model when the run started. */
	total: number;
	/** Answers written so far in this run. */
	answered: number;
	/** Of those, filed to a category, refund or income. */
	applied: number;
	/** Of those (plus any the model couldn't answer), left in Needs review. */
	leftInReview: number;
	/** Not answered yet. */
	stillWaiting: number;
};

const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

/** The status line for a progress event, or `null` when there's nothing to say. */
export function statusText(p: Progress): string | null {
	if (p.running) {
		return `Categorising ${plural(p.stillWaiting, 'transaction', 'transactions')} with the local model…`;
	}
	if (p.answered === 0 && p.leftInReview === 0) return null;
	return `Filed ${p.applied}, ${p.leftInReview} left for you to review`;
}

/** Whether the ledger changed, so lists and totals should reload: a batch was written,
 * or a run ended. */
export function changesLedger(p: Progress, previous: Progress | null): boolean {
	return !p.running || p.answered !== (previous?.answered ?? 0);
}
