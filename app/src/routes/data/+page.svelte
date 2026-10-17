<script lang="ts">
	// Deleting data (T5.3, SPEC §8, decisions 28 and 43). Every rule lives in the core;
	// this page shows what will go before anything is deleted.
	import { goto } from '$app/navigation';
	import { errorMessage, isUnlockError } from '../unlock/api';
	import {
		CONFIRMATION,
		dataChoices,
		deleteData,
		deleteEverything,
		deleteOldBackups,
		pendingBackupOffer,
		previewDeletion,
		type BackupOffer,
		type Counts,
		type DataChoices,
		type Preview,
		type Target
	} from './api';

	let choices = $state<DataChoices | null>(null);
	let kind = $state<'import' | 'account' | 'range'>('import');
	let fileId = $state<number | null>(null);
	let accountId = $state<number | null>(null);
	let rangeAccount = $state<number | null>(null);
	let from = $state('');
	let to = $state('');
	let deletePartialFiles = $state(false);
	let preview = $state<Preview | null>(null);
	let done = $state<Counts | null>(null);
	let offer = $state<BackupOffer | null>(null);
	let everythingGone = $state(false);
	let phrase = $state('');
	/** This build can't check the Keychain; set once the user has been told. */
	let keychainWarning = $state('');
	let message = $state('');
	let error = $state('');
	let busy = $state(false);

	const target = $derived<Target | null>(
		kind === 'import' && fileId !== null
			? { kind, id: fileId }
			: kind === 'account' && accountId !== null
				? { kind, id: accountId }
				: kind === 'range' && from !== '' && to !== ''
					? { kind, account: rangeAccount, from, to }
					: null
	);

	// A new choice needs a new preview, and starts with partly-covered files kept.
	$effect(() => {
		void target;
		preview = null;
		deletePartialFiles = false;
	});

	async function guard<T>(action: () => Promise<T>): Promise<T | undefined> {
		busy = true;
		error = '';
		message = '';
		try {
			return await action();
		} catch (e) {
			error = errorMessage(e);
		} finally {
			busy = false;
		}
	}

	async function showPreview() {
		if (!target) return;
		preview = (await guard(() => previewDeletion(target, deletePartialFiles))) ?? null;
	}

	async function confirmDelete() {
		if (!target) return;
		const result = await guard(() => deleteData(target, deletePartialFiles));
		if (result) {
			done = result.counts;
			offer = result.backupOffer;
			preview = null;
			choices = await dataChoices();
		}
	}

	async function runDeleteEverything(allowUnverifiedKeychain: boolean) {
		busy = true;
		error = '';
		try {
			offer = await deleteEverything(phrase, allowUnverifiedKeychain);
			everythingGone = true;
			keychainWarning = '';
		} catch (e) {
			if (isUnlockError(e) && (e.code as string) === 'KeychainUnverifiable') {
				keychainWarning = e.message;
			} else {
				error = errorMessage(e);
			}
		} finally {
			busy = false;
		}
	}

	function confirmEverything(event: SubmitEvent) {
		event.preventDefault();
		runDeleteEverything(false);
	}

	async function acceptOffer() {
		const n = await guard(deleteOldBackups);
		if (n !== undefined) {
			message = `Deleted ${n} old backup${n === 1 ? '' : 's'}.`;
			offer = null;
		}
	}

	function describe(c: Counts): string {
		const parts = [
			`${c.transactions} transaction${c.transactions === 1 ? '' : 's'}`,
			`${c.decisions} categorisation${c.decisions === 1 ? '' : 's'}`,
			`${c.rules} rule${c.rules === 1 ? '' : 's'}`
		];
		if (c.imports) parts.push(`${c.imports} original file${c.imports === 1 ? '' : 's'}`);
		if (c.accounts) parts.push(`${c.accounts} account${c.accounts === 1 ? '' : 's'}`);
		return parts.join(', ');
	}

	$effect(() => {
		dataChoices()
			.then((c) => (choices = c))
			.catch(async (e) => {
				// After "delete everything" in this run, only the backup offer remains.
				offer = await pendingBackupOffer().catch(() => null);
				if (!offer) error = errorMessage(e);
			});
	});
</script>

<main>
	<h1>Delete data</h1>

	{#if offer}
		<section class="offer" role="alert">
			<p>{offer.message}</p>
			<div class="actions">
				<button class="danger" disabled={busy} onclick={acceptOffer}>
					Delete {offer.count} backup{offer.count === 1 ? '' : 's'}
				</button>
				<button disabled={busy} onclick={() => (offer = null)}>Keep them</button>
			</div>
		</section>
	{/if}

	{#if everythingGone}
		<p>Everything was deleted. The keys were destroyed first, so no copy left behind can be read.</p>
		{#if !offer}<button onclick={() => goto('/unlock')}>Start again</button>{/if}
	{:else if choices}
		<section>
			<h2>Delete part of your data</h2>
			<p>
				Everything that came from what you delete goes with it: categorisations, rules learned
				from it, and transfer matches. You see the counts before anything is deleted.
			</p>
			<fieldset>
				<label><input type="radio" bind:group={kind} value="import" /> An original file</label>
				<label><input type="radio" bind:group={kind} value="account" /> An account</label>
				<label><input type="radio" bind:group={kind} value="range" /> A date range</label>
			</fieldset>

			{#if kind === 'import'}
				<select bind:value={fileId}>
					<option value={null}>Choose a file…</option>
					{#each choices.files as f (f.id)}
						<option value={f.id}>
							{f.fileName ?? `Import ${f.id}`} ({f.transactions} transactions,
							{new Date(f.importedAt * 1000).toLocaleDateString()})
						</option>
					{/each}
				</select>
			{:else if kind === 'account'}
				<select bind:value={accountId}>
					<option value={null}>Choose an account…</option>
					{#each choices.accounts as a (a.id)}
						<option value={a.id}>
							{a.name}{a.institution ? ` (${a.institution})` : ''}, {a.transactions} transactions
						</option>
					{/each}
				</select>
			{:else}
				<div class="range">
					<select bind:value={rangeAccount}>
						<option value={null}>All accounts</option>
						{#each choices.accounts as a (a.id)}<option value={a.id}>{a.name}</option>{/each}
					</select>
					<label>From <input type="date" bind:value={from} /></label>
					<label>To <input type="date" bind:value={to} /></label>
				</div>
				<p class="note">Dates are purchase dates when the bank gives them, otherwise posted dates.</p>
			{/if}

			<button disabled={busy || !target} onclick={showPreview}>Show what will be deleted</button>

			{#if preview}
				<div class="preview">
					<p><strong>This will delete:</strong> {describe(preview.counts)}.</p>
					{#if preview.partialFiles.length > 0}
						<p>
							{preview.partialFiles.length} original file{preview.partialFiles.length === 1
								? ''
								: 's'} also contain{preview.partialFiles.length === 1 ? 's' : ''} transactions
							outside what you chose:
						</p>
						<ul>
							{#each preview.partialFiles as p (p.id)}
								<li>
									{p.fileName ?? `Import ${p.id}`}: {p.inScope} inside, {p.outOfScope} outside{p.forced
										? ' (belongs to this account, so it is deleted whole)'
										: ''}
								</li>
							{/each}
						</ul>
						<label class="check">
							<input type="checkbox" bind:checked={deletePartialFiles} onchange={showPreview} />
							Also delete these files whole, with their other transactions
						</label>
						{#if !deletePartialFiles}
							<p class="note">
								The original file still contains these transactions, encrypted in your ledger;
								they won't come back from it.
							</p>
						{/if}
					{/if}
					<button class="danger" disabled={busy} onclick={confirmDelete}>Delete</button>
				</div>
			{/if}
			{#if done}<p role="status">Deleted: {describe(done)}.</p>{/if}
		</section>

		<section class="danger-zone">
			<h2>Delete everything</h2>
			<p>
				Deletes all your data from this computer. The keys are destroyed first (the Touch ID copy,
				the key in memory, and the key file holding your password and recovery key), so no copy
				left behind in snapshots or synced folders can ever be read. Your recovery key will no
				longer unlock anything. This can't be undone.
			</p>
			<form onsubmit={confirmEverything}>
				<label>
					Type <code>{CONFIRMATION}</code> to confirm
					<input bind:value={phrase} autocomplete="off" spellcheck="false" />
				</label>
				<button
					type="submit"
					class="danger"
					disabled={busy || phrase.trim().toLowerCase() !== CONFIRMATION}
				>
					Delete everything
				</button>
			</form>
			{#if keychainWarning}
				<div class="offer" role="alert">
					<p>{keychainWarning}</p>
					<p>Nothing has been deleted yet.</p>
					<button class="danger" disabled={busy} onclick={() => runDeleteEverything(true)}>
						Delete everything anyway
					</button>
				</div>
			{/if}
		</section>
	{/if}

	{#if message}<p role="status">{message}</p>{/if}
	{#if error}<p class="error" role="alert">{error}</p>{/if}
</main>

<style>
	main {
		font-family: system-ui, sans-serif;
		max-width: 42rem;
		margin: 2rem auto;
		padding: 0 1rem;
	}
	section {
		margin: 1.5rem 0;
	}
	fieldset {
		border: none;
		padding: 0;
		display: flex;
		gap: 1rem;
		margin: 0.5rem 0;
	}
	.range {
		display: flex;
		gap: 0.75rem;
		align-items: end;
	}
	label {
		display: grid;
		gap: 0.25rem;
	}
	.check {
		display: flex;
		gap: 0.5rem;
		align-items: baseline;
	}
	.preview,
	.offer {
		border: 1px solid #ccc;
		border-radius: 6px;
		padding: 0.75rem 1rem;
		margin: 1rem 0;
	}
	.offer {
		border-color: #c77700;
	}
	.actions {
		display: flex;
		gap: 0.5rem;
	}
	.danger-zone {
		border-top: 1px solid #eee;
		padding-top: 1rem;
	}
	button.danger {
		color: #fff;
		background: #b00020;
		border: none;
		padding: 0.4rem 0.9rem;
		border-radius: 4px;
	}
	button.danger:disabled {
		opacity: 0.5;
	}
	form {
		display: grid;
		gap: 0.75rem;
		max-width: 22rem;
	}
	.note {
		font-size: 0.9rem;
		color: #555;
	}
	.error {
		color: #b00020;
	}
</style>
