// The column-mapping commands (app/src-tauri/src/csv_mapping.rs). Each sends the file's
// bytes as the body and its details as the `fm-mapping` header, like import_file.
import { invoke } from '@tauri-apps/api/core';
import type { Money } from '../../dashboard/ledger';

export type ColumnRole =
	| 'postedDate'
	| 'transactionDate'
	| 'description'
	| 'payee'
	| 'memo'
	| 'amount'
	| 'debit'
	| 'credit'
	| 'absAmount'
	| 'indicator'
	| 'balance'
	| 'checkNumber'
	| 'reference'
	| 'bankCategory'
	| 'ignore';

/** The choices, in the order the screen offers them. */
export const ROLES: [ColumnRole, string][] = [
	['ignore', 'Not needed'],
	['postedDate', 'Posted date'],
	['transactionDate', 'Purchase date'],
	['description', 'Description'],
	['payee', 'Payee or merchant'],
	['memo', 'Memo'],
	['amount', 'Amount'],
	['debit', 'Money out'],
	['credit', 'Money in'],
	['absAmount', 'Amount without a sign'],
	['indicator', 'Money in or out (indicator)'],
	['balance', 'Balance'],
	['checkNumber', 'Check number'],
	['reference', 'Reference'],
	['bankCategory', "Bank's category"]
];

export type Mapping = {
	columns: ColumnRole[];
	dateFormat: string;
	moneyOut: 'negative' | 'positive';
	indicator: { debit: string; credit: string } | null;
};

export type Inspection = {
	/** A layout already reads this file: import it instead. */
	known: string | null;
	headers: string[];
	sampleRows: string[][];
	suggestion: Mapping;
	dateFormats: string[];
	notice: string | null;
};

export type PreviewRow = {
	postedDate: string;
	transactionDate: string | null;
	description: string;
	payee: string | null;
	amount: Money;
	pending: boolean;
};

export type Preview = { rows: PreviewRow[]; rowCount: number };

export type Saved = { id: string; institution: string };

export type LayoutAccountKind = 'checking' | 'savings' | 'credit_card';

const headers = (details: object) => ({
	headers: { 'fm-mapping': encodeURIComponent(JSON.stringify(details)) }
});

export const inspectCsv = (bytes: Uint8Array) =>
	invoke<Inspection>('csv_mapping_inspect', bytes, headers({}));

export const previewCsv = (bytes: Uint8Array, mapping: Mapping, currency: string | null) =>
	invoke<Preview>('csv_mapping_preview', bytes, headers({ mapping, currency }));

export const saveCsvLayout = (
	bytes: Uint8Array,
	mapping: Mapping,
	bankName: string,
	accountKinds: LayoutAccountKind[]
) => invoke<Saved>('csv_mapping_save', bytes, headers({ mapping, bankName, accountKinds }));
