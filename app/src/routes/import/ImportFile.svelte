<script lang="ts">
	// Importing one bank file (T3.3). Reusable: T5.5's first run embeds it. The file's bytes
	// go straight to the core; nothing is parsed here. After the import the core pairs
	// transfers and runs your rules, and the report shows what happened, including the rows
	// that matched transactions already in the ledger (a wrong match would otherwise be
	// invisible).
	import TransactionList from '../dashboard/TransactionList.svelte';
	import {
		accounts,
		errorCode,
		errorText,
		finishImport,
		importFile,
		unfinishedImport,
		type Account,
		type AccountKind,
		type ImportReport,
		type ImportTarget
	} from '../dashboard/ledger';

	let { onimported }: { onimported?: (report: ImportReport) => void } = $props();

	const KINDS: [AccountKind, string][] = [
		['checking', 'Checking'],
		['savings', 'Savings'],
		['credit_card', 'Credit card'],
		['cash', 'Cash'],
		['loan', 'Loan'],
		['mortgage', 'Mortgage'],
		['brokerage', 'Brokerage'],
		['retirement', 'Retirement']
	];

	let file = $state<File | null>(null);
	let known = $state<Account[]>([]);
	let choice = $state<'auto' | 'account' | 'new'>('auto');
	let accountId = $state<number | null>(null);
	let newName = $state('');
	let newKind = $state<AccountKind>('checking');
	let newCurrency = $state('USD');
	let needsAccount = $state(false);
	let report = $state<ImportReport | null>(null);
	let showMatched = $state(false);
	let error = $state('');
	let noLayout = $state(false);
	let busy = $state(false);
	/** An import left unfinished earlier (the app locked or quit), offered on arrival. */
	let leftOver = $state<{ importId: number | null } | null>(null);

	$effect(() => {
		accounts()
			.then((a) => (known = a))
			.catch((e) => (error = errorText(e)));
		unfinishedImport()
			.then((u) => (leftOver = u))
			.catch(() => {});
	});

	async function finish(importId: number | null) {
		busy = true;
		error = '';
		try {
			report = await finishImport(importId);
			leftOver = null;
			onimported?.(report);
		} catch (e) {
			error = errorText(e);
		} finally {
			busy = false;
		}
	}

	function target(): ImportTarget {
		if (choice === 'account' && accountId !== null) return { kind: 'account', id: accountId };
		if (choice === 'new')
			return { kind: 'newAccount', name: newName, accountKind: newKind, currency: newCurrency };
		return { kind: 'auto' };
	}

	/** Matches MAX_IMPORT_BYTES in app/src-tauri/src/importing.rs. */
	const MAX_BYTES = 50 * 1024 * 1024;

	async function run() {
		if (!file) return;
		if (file.size > MAX_BYTES) {
			// Refused before the file is read into memory.
			error = 'This file is over 50 MB. Bank exports are much smaller: check it is the right file.';
			return;
		}
		busy = true;
		error = '';
		noLayout = false;
		report = null;
		showMatched = false;
		try {
			const bytes = new Uint8Array(await file.arrayBuffer());
			report = await importFile(bytes, { fileName: file.name, target: target(), csvCurrency: null });
			needsAccount = false;
			known = await accounts();
			onimported?.(report);
		} catch (e) {
			const code = errorCode(e);
			if (code === 'AccountNeeded') {
				needsAccount = true;
				choice = known.length ? 'account' : 'new';
			}
			noLayout = code === 'NoLayout';
			error =
				code === 'Interrupted'
					? 'Nothing was imported; the app was locked or closed during the import. Import the file again.'
					: errorText(e);
		} finally {
			busy = false;
		}
	}
</script>

<form
	onsubmit={(e) => {
		e.preventDefault();
		run();
	}}
>
	<label>
		Bank file (CSV, OFX or QFX)
		<input
			type="file"
			accept=".csv,.ofx,.qfx,text/csv"
			onchange={(e) => {
				file = (e.currentTarget as HTMLInputElement).files?.[0] ?? null;
				report = null;
				error = '';
			}}
		/>
	</label>

	<fieldset>
		<legend>Account</legend>
		<label class="check"><input type="radio" bind:group={choice} value="auto" /> Find it from the file</label>
		{#if known.length > 0}
			<label class="check"><input type="radio" bind:group={choice} value="account" /> One of my accounts</label>
		{/if}
		<label class="check"><input type="radio" bind:group={choice} value="new" /> A new account</label>
	</fieldset>
	{#if needsAccount}
		<p class="warn">This file doesn't say which account it is for. Choose one below.</p>
	{/if}
	{#if choice === 'account'}
		<select bind:value={accountId} aria-label="Account">
			<option value={null}>choose…</option>
			{#each known as a (a.id)}
				<option value={a.id}>{a.name}{a.institution ? ` · ${a.institution}` : ''} ({a.currency})</option>
			{/each}
		</select>
	{:else if choice === 'new'}
		<div class="row">
			<label>Name <input bind:value={newName} placeholder="e.g. Chase checking" /></label>
			<label>
				Kind
				<select bind:value={newKind}>
					{#each KINDS as [k, label] (k)}<option value={k}>{label}</option>{/each}
				</select>
			</label>
			<label>Currency <input bind:value={newCurrency} size="4" maxlength="3" /></label>
		</div>
	{/if}

	<button type="submit" disabled={!file || busy || (choice === 'account' && accountId === null)}>
		{busy ? 'Importing…' : 'Import'}
	</button>
</form>

{#if leftOver && !report}
	<p class="warn" role="status">
		An earlier import didn't finish (the app was locked or closed).
		<button type="button" disabled={busy} onclick={() => finish(leftOver?.importId ?? null)}>
			Finish import
		</button>
	</p>
{/if}

{#if error}
	<p role="alert">
		{error}
		{#if noLayout}<a href="/import/mapping">Map columns</a>{/if}
	</p>
{/if}

{#if report}
	<section class="report" role="status">
		<h3>{report.resumed ? 'Import finished' : 'Imported'}</h3>
		{#if report.layoutsNotice}<p class="warn">{report.layoutsNotice}</p>{/if}
		{#if report.incomplete}
			<p class="warn">
				Imported, but {report.incomplete} didn't finish.
				<button type="button" disabled={busy} onclick={() => finish(report?.importId ?? null)}>
					Finish import
				</button>
			</p>
		{/if}
		{#if report.overlapWarning}
			<p class="warn">{report.overlapWarning}. Everything in the file was imported.</p>
		{/if}
		<ul>
			{#each report.statements as s (s.accountId)}
				<li>
					<strong>{s.account}</strong>{s.accountCreated ? ' (new account)' : ''}: {s.inserted} new
					{s.inserted === 1 ? 'transaction' : 'transactions'}
					{#if s.postedFromPending > 0}, {s.postedFromPending} pending ones settled{/if}
				</li>
			{/each}
			{#if !report.resumed}<li>
				{report.matched.length}
				{report.matched.length === 1 ? 'row' : 'rows'} matched existing transactions
				{#if report.matched.length > 0}
					<button type="button" class="link" onclick={() => (showMatched = !showMatched)}>
						{showMatched ? 'hide' : 'check them'}
					</button>
				{/if}
			</li>{/if}
			<li>{report.transfersPaired} transfers between your accounts paired{report.transfersSuggested > 0 ? `, ${report.transfersSuggested} suggested` : ''}</li>
			<li>
				Your rules filed {report.ruleBuckets}; {report.cleanedMerchants} merchant names cleaned up;
				<a href="/review">{report.needsReview} to review</a>
			</li>
			{#if report.ruleDidNotFit > 0}
				<li class="warn">A rule didn't fit {report.ruleDidNotFit}; they were left as they are.</li>
			{/if}
			{#if report.suggestions > 0}
				<li>{report.suggestions} suggestions for transactions you've already confirmed or corrected</li>
			{/if}
			<li>
				{#if report.backup === null}
					No backup folder is set: <a href="/backup">choose one</a>.
				{:else if report.backup.ok}
					Backed up.{report.backup.message ? ` ${report.backup.message}` : ''}
				{:else}
					<span class="warn">The backup failed: {report.backup.message}</span>
				{/if}
			</li>
		</ul>
		{#if showMatched}
			<p class="muted">
				These rows of the file were not added because they are already in your ledger (or settled a
				pending one). If one looks wrong, tell us: nothing else shows it.
			</p>
			<TransactionList ids={report.matched.map((m) => m.transactionId)} title="Matched to existing transactions" />
		{/if}
	</section>
{/if}

<style>
	form {
		display: grid;
		gap: 0.75rem;
		max-width: 36rem;
	}
	label {
		display: grid;
		gap: 0.25rem;
	}
	label.check {
		display: flex;
		gap: 0.5rem;
		align-items: baseline;
	}
	fieldset {
		border: none;
		padding: 0;
		display: flex;
		gap: 1rem;
		flex-wrap: wrap;
	}
	.row {
		display: flex;
		gap: 0.75rem;
		align-items: end;
	}
	.report {
		border: 1px solid color-mix(in srgb, currentColor 20%, transparent);
		border-radius: 6px;
		padding: 0.5rem 1rem;
		margin-top: 1rem;
	}
	.warn {
		color: #b25e00;
	}
	.muted {
		color: GrayText;
	}
	button.link {
		background: none;
		border: none;
		padding: 0;
		color: LinkText;
		cursor: pointer;
		font: inherit;
	}
</style>
