// The Ask screen's command (app/src-tauri/src/ask.rs).
import { invoke } from '@tauri-apps/api/core';
import { localToday, type Money } from '../dashboard/ledger';

export type ReadItem = {
	name: string;
	label: string;
	line: string;
	/** The badge's text when the app added this item: you didn't ask for it. */
	added: string | null;
	addedRule: string | null;
};

export type AnswerKind = 'money' | 'count' | 'text' | 'texts' | 'date' | 'percent';

export type AnswerView = {
	name: string;
	label: string;
	added: string | null;
	kind: AnswerKind;
	money: Money | null;
	count: number | null;
	texts: string[];
	date: string | null;
	basisPoints: number | null;
	percent: string | null;
	transactionIds: number[];
};

export type AskView = {
	read: ReadItem[];
	answers: AnswerView[];
	explanation: { text: string; source: 'model' | 'template'; numbers: number } | null;
	problem: string | null;
	asOf: string;
};

/** `today` is the local date, as on the dashboard; the model never gives it. */
export const ask = (question: string) =>
	invoke<AskView>('ask', { question, today: localToday() });
