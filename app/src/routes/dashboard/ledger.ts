// Typed wrappers for the ledger commands (T3.3): app/src-tauri/src/{ledger_view,dashboard,
// review,categories,importing}.rs. Every number comes from fm-core; this file only formats.

import { invoke } from '@tauri-apps/api/core';

/** An amount: fm-core's minor units and its exact decimal string, e.g. "-1234.50". */
export type Money = { minor: number; currency: string; decimal: string };

export type UiError = { code: string; message: string };

export function errorText(e: unknown): string {
	if (typeof e === 'object' && e !== null && 'message' in e) return String(e.message);
	return String(e);
}

export function errorCode(e: unknown): string | null {
	return typeof e === 'object' && e !== null && 'code' in e ? String(e.code) : null;
}

const formats = new Map<string, Intl.NumberFormat>();

/**
 * Formats fm-core's decimal string as currency. Intl formats the string exactly, so no
 * floating point or arithmetic touches the amount.
 */
export function money(m: Money, opts: { signed?: boolean } = {}): string {
	const key = `${m.currency}:${opts.signed ? 's' : ''}`;
	let f = formats.get(key);
	if (!f) {
		f = new Intl.NumberFormat(undefined, {
			style: 'currency',
			currency: m.currency,
			signDisplay: opts.signed ? 'exceptZero' : 'auto'
		});
		formats.set(key, f);
	}
	return f.format(m.decimal as Intl.StringNumericLiteral);
}

export const isNegative = (m: Money) => m.minor < 0;

export type Bucket = {
	kind: 'category' | 'refund' | 'income' | 'transfer' | 'needsReview';
	label: string;
	categoryId: number | null;
	paired: boolean;
};

export type Suggestion = {
	decisionId: number;
	source: string;
	label: string;
	bucket: Bucket | null;
	merchant: string | null;
};

export type Transaction = {
	id: number;
	date: string;
	postedDate: string;
	pending: boolean;
	account: string;
	description: string;
	merchant: string | null;
	amount: Money;
	bucket: Bucket;
	bucketSource: string | null;
	/** The bucket decision on screen: what Approve and Reject act on. */
	bucketDecisionId: number | null;
	/** The model's confidence, e.g. "87%", for model decisions. */
	bucketConfidence: string | null;
	/** The bucket waits for Approve or Reject (decision 46). */
	awaitsConfirmation: boolean;
	/** For a paired transfer: the other side, which Approve, Reject and corrections change too. */
	pairedWith: { id: number; account: string; decisionId: number | null } | null;
	suggestions: Suggestion[];
};

// --- Dashboard

export type Line = { amount: Money; count: number; transactionIds: number[] };
export type Totals = {
	spending: Line;
	refunds: Line;
	income: Line;
	transfersOut: Line;
	transfersIn: Line;
	needsReview: Line;
	pending: Line;
	netFlow: Money;
};
export type Row = {
	label: string;
	total: Money;
	gross: Money;
	refunds: Money;
	count: number;
	transactionIds: number[];
	fromDescription: boolean;
};
export type Change = { base: Money; change: Money; percent: string | null };
export type GroupRow = {
	groupId: number | null;
	row: Row;
	change: Change | null;
	categories: { categoryId: number; row: Row }[];
};
export type TrendRow = {
	label: string;
	values: Money[];
	average: Money | null;
	median: Money | null;
	latest: Money | null;
};
export type Trend = {
	months: { month: string; partial: boolean; total: Money }[];
	rows: TrendRow[];
	total: TrendRow;
};
export type Recurring = {
	name: string;
	cadence: string;
	amount: Money;
	variable: boolean;
	last: string;
	nextExpected: string | null;
	ended: boolean;
	accounts: string[];
	transactionIds: number[];
};
export type Dashboard = {
	asOf: string;
	currencies: string[];
	currency: string | null;
	months: string[];
	month: string | null;
	partial: boolean;
	totals: Totals | null;
	spendingChange: Change | null;
	groups: GroupRow[];
	merchants: Row[];
	merchantsOther: Row | null;
	trend: Trend | null;
	recurring: Recurring[];
};

/**
 * Today's local calendar date, YYYY-MM-DD. Not `toISOString()`, which is the UTC date: in
 * the evening west of UTC that is already tomorrow.
 */
export function localToday(now: Date = new Date()): string {
	const pad = (n: number) => String(n).padStart(2, '0');
	return `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}`;
}

export const dashboard = (month: string | null, currency: string | null) =>
	invoke<Dashboard>('dashboard', { month, currency, today: localToday() });

export const transactions = (ids: number[]) => invoke<Transaction[]>('transactions', { ids });

// --- Review

export type Queue = { total: number; offset: number; transactions: Transaction[] };
export type PickerGroup = { groupId: number; name: string; categories: { id: number; name: string }[] };
export type BucketChoice =
	| { kind: 'category'; categoryId: number }
	| { kind: 'refund'; categoryId: number }
	| { kind: 'income' }
	| { kind: 'transfer' };
export type RulePreview = { pattern: string | null; matches: number };
export type CorrectionResult = {
	ruleId: number | null;
	noPattern: boolean;
	unpaired: number[];
	reapplied: number;
	ruleBuckets: number;
	ruleMerchants: number;
	ruleDidNotFit: number;
};

export const reviewQueue = (offset: number, limit: number) =>
	invoke<Queue>('review_queue', { offset, limit });
export const categoryPicker = () => invoke<PickerGroup[]>('category_picker');
export const previewRule = (transactionId: number, pattern: string | null) =>
	invoke<RulePreview>('preview_rule', { transactionId, pattern });
export const correctTransaction = ({
	transactionId,
	bucket,
	merchant,
	learn,
	pattern
}: {
	transactionId: number;
	bucket: BucketChoice | null;
	merchant: string | null;
	learn: boolean;
	pattern: string | null;
}) =>
	invoke<CorrectionResult>('correct_transaction', {
		transactionId,
		bucket,
		merchant,
		learn,
		pattern
	});
export const suggestions = () => invoke<Transaction[]>('suggestions');
/**
 * Approve the decisions on screen (decision 46): pass the ids that were displayed, never
 * "whatever is waiting now". Fails with code "Changed" if one changed since loading.
 */
export const approveDecisions = (decisionIds: number[]) =>
	invoke<number>('approve_decisions', { decisionIds });
/** Reject the decisions on screen: back to Needs review. Returns how many changed. */
export const rejectDecisions = (decisionIds: number[]) =>
	invoke<number>('reject_decisions', { decisionIds });
export const acceptSuggestion = (decisionId: number) =>
	invoke<number[]>('accept_suggestion', { decisionId });
export const dismissSuggestion = (decisionId: number) =>
	invoke<number[]>('dismiss_suggestion', { decisionId });

// --- Categories and rules

export type Category = {
	id: number;
	key: string;
	name: string;
	hidden: boolean;
	mergedInto: number | null;
	mergedIntoName: string | null;
	transactions: number;
};
export type Group = { id: number; key: string; name: string; hidden: boolean; categories: Category[] };
export type Taxonomy = { version: string; groups: Group[] };
/** fm_core::categories::Edit. */
export type Edit =
	| { kind: 'rename_category'; id: number; name: string }
	| { kind: 'rename_group'; id: number; name: string }
	| { kind: 'set_category_hidden'; id: number; hidden: boolean }
	| { kind: 'set_group_hidden'; id: number; hidden: boolean }
	| { kind: 'merge'; from: number; into: number };
export type Rule = {
	id: number;
	matchKind: 'contains' | 'startsWith' | 'equals';
	pattern: string;
	account: string | null;
	amount:
		| { kind: 'exact'; amount: Money }
		| { kind: 'between'; min: Money; max: Money }
		| null;
	bucket: string | null;
	merchant: string | null;
	origin: 'you' | 'correction';
	enabled: boolean;
	priority: number;
	decisions: number;
};
export type Rules = { rules: Rule[]; ruleDidNotFit: number };

export const taxonomy = () => invoke<Taxonomy>('taxonomy');
export const editTaxonomy = (edit: Edit) => invoke<Taxonomy>('edit_taxonomy', { edit });
export const rules = () => invoke<Rules>('rules');
export const setRuleEnabled = (ruleId: number, enabled: boolean, undo: boolean) =>
	invoke<Rules>('set_rule_enabled', { ruleId, enabled, undo });
export const deleteRule = (ruleId: number, undo: boolean) =>
	invoke<Rules>('delete_rule', { ruleId, undo });

// --- Import

export type AccountKind =
	| 'checking'
	| 'savings'
	| 'credit_card'
	| 'cash'
	| 'loan'
	| 'mortgage'
	| 'brokerage'
	| 'retirement';
export type Account = {
	id: number;
	name: string;
	kind: AccountKind;
	currency: string;
	institution: string | null;
};
export type ImportTarget =
	| { kind: 'auto' }
	| { kind: 'account'; id: number }
	| { kind: 'newAccount'; name: string; accountKind: AccountKind; currency: string };
export type MatchedRow = {
	sourceIndex: number;
	outcome: 'duplicate' | 'settledPending' | 'alreadyPosted';
	transactionId: number;
};
export type ImportReport = {
	importId: number;
	statements: {
		accountId: number;
		account: string;
		accountCreated: boolean;
		inserted: number;
		duplicates: number;
		postedFromPending: number;
		pendingAlreadyPosted: number;
	}[];
	inserted: number;
	matched: MatchedRow[];
	transfersPaired: number;
	transfersSuggested: number;
	ruleBuckets: number;
	ruleMerchants: number;
	cleanedMerchants: number;
	suggestions: number;
	ruleDidNotFit: number;
	needsReview: number;
	backup: { ok: boolean; message: string | null } | null;
	overlapWarning: string | null;
	/** The import was kept, but this step didn't finish; `finishImport` runs it. */
	incomplete: string | null;
	/** This completes an earlier import; no rows were read now. */
	resumed: boolean;
	/** Set when saved CSV layouts couldn't be read, so only built-in ones were tried. */
	layoutsNotice: string | null;
};

export const accounts = () => invoke<Account[]>('accounts');

/** The file's bytes go as the raw body; the details as a percent-encoded JSON header. */
export const importFile = (
	bytes: Uint8Array,
	details: { fileName: string | null; target: ImportTarget; csvCurrency: string | null }
) =>
	invoke<ImportReport>('import_file', bytes, {
		headers: { 'fm-import': encodeURIComponent(JSON.stringify(details)) }
	});

/** Runs what an import left unfinished (transfers, rules, backup); harmless if nothing is. */
export const finishImport = (importId: number | null) =>
	invoke<ImportReport>('finish_import', { importId });

/** The import left unfinished, if any (`importId` is null if the app stopped mid-read). */
export const unfinishedImport = () =>
	invoke<{ importId: number | null } | null>('unfinished_import');
