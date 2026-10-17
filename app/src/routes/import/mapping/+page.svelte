<script lang="ts">
	// The column-mapping screen (T3.5): for a CSV whose layout no saved or built-in one
	// knows. You say what each column holds, check the rows it reads, and save the layout
	// under the bank's name; the next file from that bank is recognised by itself. The
	// file is only read, never stored, until you import it.
	import ImportFile from '../ImportFile.svelte';
	import { errorText, money } from '../../dashboard/ledger';
	import {
		ROLES,
		inspectCsv,
		previewCsv,
		saveCsvLayout,
		type Inspection,
		type LayoutAccountKind,
		type Mapping,
		type Preview,
		type Saved
	} from './mapping';

	/** Matches MAX_IMPORT_BYTES in app/src-tauri/src/importing.rs. */
	const MAX_BYTES = 50 * 1024 * 1024;

	const KINDS: [LayoutAccountKind, string][] = [
		['checking', 'Checking'],
		['savings', 'Savings'],
		['credit_card', 'Credit card']
	];

	let bytes = $state<Uint8Array | null>(null);
	let shown = $state<Inspection | null>(null);
	let mapping = $state<Mapping | null>(null);
	let preview = $state<Preview | null>(null);
	let bankName = $state('');
	let kinds = $state<LayoutAccountKind[]>([]);
	let saved = $state<Saved | null>(null);
	let error = $state('');
	let busy = $state(false);

	const usesAmount = $derived(mapping?.columns.includes('amount') ?? false);
	const usesIndicator = $derived(mapping?.columns.includes('indicator') ?? false);

	async function choose(file: File | null) {
		bytes = null;
		shown = null;
		mapping = null;
		preview = null;
		saved = null;
		error = '';
		if (!file) return;
		if (file.size > MAX_BYTES) {
			error = 'This file is over 50 MB. Bank exports are much smaller: check it is the right file.';
			return;
		}
		busy = true;
		try {
			bytes = new Uint8Array(await file.arrayBuffer());
			shown = await inspectCsv(bytes);
			mapping = $state.snapshot(shown.suggestion);
		} catch (e) {
			error = errorText(e);
		} finally {
			busy = false;
		}
	}

	async function check() {
		if (!bytes || !mapping) return;
		busy = true;
		error = '';
		preview = null;
		try {
			preview = await previewCsv(bytes, $state.snapshot(mapping), null);
		} catch (e) {
			error = errorText(e);
		} finally {
			busy = false;
		}
	}

	async function save() {
		if (!bytes || !mapping) return;
		busy = true;
		error = '';
		try {
			saved = await saveCsvLayout(bytes, $state.snapshot(mapping), bankName, $state.snapshot(kinds));
		} catch (e) {
			error = errorText(e);
		} finally {
			busy = false;
		}
	}

	function changed() {
		// A new mapping needs a new preview before it can be saved.
		preview = null;
		if (mapping && mapping.columns.includes('indicator') && !mapping.indicator) {
			mapping.indicator = { debit: '', credit: '' };
		}
	}
</script>

<main>
	<nav>
		<a href="/">Home</a> · <a href="/import">Import</a> · <a href="/dashboard">Dashboard</a>
	</nav>
	<h1>Map a CSV file's columns</h1>
	<p class="muted">
		For a bank whose CSV layout isn't known yet. Say what each column holds, check the rows,
		and save the layout under the bank's name. Next time, a file from that bank imports by
		itself.
	</p>

	<label>
		CSV file
		<input
			type="file"
			accept=".csv,text/csv"
			disabled={busy}
			onchange={(e) => choose((e.currentTarget as HTMLInputElement).files?.[0] ?? null)}
		/>
	</label>

	{#if shown?.notice}
		<p class="warn">{shown.notice}</p>
	{/if}

	{#if shown?.known}
		<p>
			This file already reads with the <strong>{shown.known}</strong> layout: import it on the
			<a href="/import">Import</a> page.
		</p>
	{:else if shown && mapping}
		<section>
			<h2>What each column holds</h2>
			<div class="scroll">
				<table>
					<thead>
						<tr>
							{#each shown.headers as header, i (i)}
								<th>
									<div>{header || `Column ${i + 1}`}</div>
									<select bind:value={mapping.columns[i]} onchange={changed}>
										{#each ROLES as [role, label] (role)}
											<option value={role}>{label}</option>
										{/each}
									</select>
								</th>
							{/each}
						</tr>
					</thead>
					<tbody>
						{#each shown.sampleRows as row, r (r)}
							<tr>
								{#each row as cell, c (c)}
									<td>{cell}</td>
								{/each}
							</tr>
						{/each}
					</tbody>
				</table>
			</div>

			<label>
				Dates are written as
				<select bind:value={mapping.dateFormat} onchange={changed}>
					{#each shown.dateFormats as format (format)}
						<option value={format}>{format}</option>
					{/each}
				</select>
			</label>
			{#if usesAmount}
				<fieldset onchange={changed}>
					<legend>In the amount column, a purchase is</legend>
					<label class="check">
						<input type="radio" bind:group={mapping.moneyOut} value="negative" /> negative (−12.50)
					</label>
					<label class="check">
						<input type="radio" bind:group={mapping.moneyOut} value="positive" /> positive (12.50)
					</label>
				</fieldset>
			{/if}
			{#if usesIndicator}
				{#if mapping.indicator}
					<fieldset onchange={changed}>
						<legend>The indicator column says</legend>
						<label>Money out: <input bind:value={mapping.indicator.debit} /></label>
						<label>Money in: <input bind:value={mapping.indicator.credit} /></label>
					</fieldset>
				{/if}
			{/if}
			<button type="button" disabled={busy} onclick={check}>Preview the rows</button>
		</section>
	{/if}

	{#if error}
		<p class="error" role="alert">{error}</p>
	{/if}

	{#if preview && mapping}
		<section>
			<h2>The rows it reads</h2>
			<p>
				{preview.rowCount} rows. The first {preview.rows.length}:
			</p>
			<div class="scroll">
				<table>
					<thead>
						<tr><th>Posted</th><th>Purchased</th><th>Description</th><th class="num">Amount</th></tr>
					</thead>
					<tbody>
						{#each preview.rows as row, i (i)}
							<tr>
								<td>{row.postedDate}</td>
								<td>{row.transactionDate ?? ''}</td>
								<td>{row.description}{row.pending ? ' (pending)' : ''}</td>
								<td class="num">{money(row.amount, { signed: true })}</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>

			{#if !saved}
				<form
					onsubmit={(e) => {
						e.preventDefault();
						save();
					}}
				>
					<label>
						Bank name
						<input bind:value={bankName} maxlength="60" required />
					</label>
					<fieldset>
						<legend>Used for (optional)</legend>
						{#each KINDS as [kind, label] (kind)}
							<label class="check">
								<input type="checkbox" bind:group={kinds} value={kind} />
								{label}
							</label>
						{/each}
					</fieldset>
					<button type="submit" disabled={busy || !bankName.trim()}>Save this layout</button>
				</form>
			{/if}
		</section>
	{/if}

	{#if saved}
		<section>
			<p>
				Saved as the <strong>{saved.institution}</strong> layout. Files with these columns are now
				recognised. Import this one:
			</p>
			<ImportFile />
		</section>
	{/if}
</main>

<style>
	main {
		font-family: system-ui, sans-serif;
		margin: 2rem;
		max-width: 64rem;
	}
	.muted {
		color: GrayText;
	}
	.scroll {
		overflow-x: auto;
	}
	table {
		border-collapse: collapse;
		margin: 0.5rem 0 1rem;
	}
	th,
	td {
		border: 1px solid #ccc;
		padding: 0.25rem 0.5rem;
		text-align: left;
		vertical-align: top;
		white-space: nowrap;
	}
	.num {
		text-align: right;
	}
	label {
		display: block;
		margin: 0.5rem 0;
	}
	label.check {
		display: inline-block;
		margin-right: 1rem;
	}
	.warn {
		color: #8a5a00;
	}
	.error {
		color: #b00020;
	}
</style>
