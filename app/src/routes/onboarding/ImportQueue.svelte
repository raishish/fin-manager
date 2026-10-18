<script lang="ts">
	// Bring in data (T5.5, first run step 3): drop or choose several bank files; each is
	// imported in turn through the core (import, transfers, rules). A CSV that doesn't say
	// which account it is for asks here; an unknown CSV layout links to the column mapping.
	// An import the app couldn't finish (it locked or quit) is offered to finish, as in
	// ImportFile.svelte (T3.6).
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

	let { onimported }: { onimported?: () => void } = $props();

	/** Matches MAX_IMPORT_BYTES in app/src-tauri/src/importing.rs. */
	const MAX_BYTES = 50 * 1024 * 1024;

	type Item = {
		file: File;
		state: 'waiting' | 'importing' | 'done' | 'needsAccount' | 'noLayout' | 'failed';
		report?: ImportReport;
		message?: string;
		accountId?: number | null;
		newName?: string;
		newKind?: AccountKind;
	};

	let items = $state<Item[]>([]);
	let known = $state<Account[]>([]);
	let dragging = $state(false);
	let running = false;
	/** An import left unfinished earlier (the app locked or quit). */
	let leftOver = $state<{ importId: number | null } | null>(null);
	let finishing = $state(false);
	let finishError = $state('');

	$effect(() => {
		accounts()
			.then((a) => (known = a))
			.catch(() => {});
		unfinishedImport()
			.then((u) => (leftOver = u))
			.catch(() => {});
	});

	/** Runs what an import left unfinished (transfers, rules, backup). */
	async function finish(importId: number | null, item?: Item) {
		finishing = true;
		finishError = '';
		try {
			const report = await finishImport(importId);
			if (item) item.report = report;
			leftOver = null;
			onimported?.();
		} catch (e) {
			finishError = errorText(e);
		} finally {
			finishing = false;
		}
	}

	function add(files: FileList | File[] | null | undefined) {
		for (const file of files ?? []) items.push({ file, state: 'waiting' });
		run();
	}

	async function importOne(item: Item, target: ImportTarget) {
		if (item.file.size > MAX_BYTES) {
			item.state = 'failed';
			item.message = 'Over 50 MB: bank exports are much smaller. Check it is the right file.';
			return;
		}
		item.state = 'importing';
		try {
			const bytes = new Uint8Array(await item.file.arrayBuffer());
			item.report = await importFile(bytes, {
				fileName: item.file.name,
				target,
				csvCurrency: null
			});
			item.state = 'done';
			known = await accounts();
			onimported?.();
		} catch (e) {
			const code = errorCode(e);
			item.message = errorText(e);
			if (code === 'AccountNeeded') {
				item.state = 'needsAccount';
				item.accountId = known[0]?.id ?? null;
				item.newKind = 'checking';
			} else if (code === 'NoLayout') {
				item.state = 'noLayout';
			} else if (code === 'AlreadyImported') {
				item.state = 'done';
			} else {
				item.state = 'failed';
				if (code === 'Interrupted') {
					item.message =
						'Nothing was imported; the app was locked or closed during the import. Add the file again.';
				}
			}
		}
	}

	async function run() {
		if (running) return;
		running = true;
		try {
			for (const item of items) {
				if (item.state === 'waiting') await importOne(item, { kind: 'auto' });
			}
		} finally {
			running = false;
		}
	}

	function retryWith(item: Item) {
		const target: ImportTarget =
			item.accountId != null
				? { kind: 'account', id: item.accountId }
				: {
						kind: 'newAccount',
						name: item.newName ?? '',
						accountKind: item.newKind ?? 'checking',
						currency: 'USD'
					};
		importOne(item, target);
	}
</script>

<div
	class="drop"
	class:dragging
	role="region"
	aria-label="Drop bank files here"
	ondragover={(e) => {
		e.preventDefault();
		dragging = true;
	}}
	ondragleave={() => (dragging = false)}
	ondrop={(e) => {
		e.preventDefault();
		dragging = false;
		add(e.dataTransfer?.files);
	}}
>
	<p>Drop your bank's CSV, OFX or QFX exports here, 12 months if you can.</p>
	<label class="pick">
		or choose files
		<input
			type="file"
			multiple
			accept=".csv,.ofx,.qfx,text/csv"
			onchange={(e) => add((e.currentTarget as HTMLInputElement).files)}
		/>
	</label>
</div>

{#if leftOver}
	<p class="warn" role="status">
		An earlier import didn't finish (the app was locked or closed).
		<button type="button" disabled={finishing} onclick={() => finish(leftOver?.importId ?? null)}>
			Finish import
		</button>
	</p>
{/if}
{#if finishError}<p role="alert">{finishError}</p>{/if}

{#if items.length > 0}
	<ul class="queue">
		{#each items as item, i (i)}
			<li>
				<strong>{item.file.name}</strong>:
				{#if item.state === 'waiting'}
					waiting
				{:else if item.state === 'importing'}
					importing…
				{:else if item.state === 'done' && item.report}
					{#if item.report.resumed}
						finished
					{:else}
						{item.report.inserted} new, {item.report.matched.length} already in the ledger
					{/if}
					{#if item.report.transfersPaired > 0}· {item.report.transfersPaired} transfers paired{/if}
					{#if item.report.layoutsNotice}<br /><span class="warn">{item.report.layoutsNotice}</span>{/if}
					{#if item.report.incomplete}
						<br /><span class="warn">imported, but {item.report.incomplete} didn't finish.</span>
						<button
							type="button"
							disabled={finishing}
							onclick={() => finish(item.report?.importId ?? null, item)}
						>
							Finish import
						</button>
					{/if}
				{:else if item.state === 'done'}
					already imported
				{:else if item.state === 'noLayout'}
					this CSV layout isn't known yet. <a href="/import/mapping">Map its columns</a>
				{:else if item.state === 'needsAccount'}
					<span>which account is this file for?</span>
					<div class="account">
						<select bind:value={item.accountId} aria-label="Account">
							{#each known as a (a.id)}
								<option value={a.id}>{a.name}</option>
							{/each}
							<option value={null}>A new account…</option>
						</select>
						{#if item.accountId == null}
							<input bind:value={item.newName} placeholder="Account name" aria-label="New account name" />
							<select bind:value={item.newKind} aria-label="Kind">
								<option value="checking">Checking</option>
								<option value="savings">Savings</option>
								<option value="credit_card">Credit card</option>
							</select>
						{/if}
						<button type="button" onclick={() => retryWith(item)}>Import</button>
					</div>
				{:else}
					<span class="failed">{item.message}</span>
				{/if}
			</li>
		{/each}
	</ul>
{/if}

<style>
	.drop {
		border: 2px dashed color-mix(in srgb, currentColor 30%, transparent);
		border-radius: 8px;
		padding: 1.5rem;
		text-align: center;
	}
	.drop.dragging {
		border-color: LinkText;
		background: color-mix(in srgb, LinkText 6%, transparent);
	}
	.pick input {
		display: block;
		margin: 0.5rem auto 0;
	}
	.queue {
		padding-left: 1.2rem;
		display: grid;
		gap: 0.4rem;
	}
	.account {
		display: flex;
		gap: 0.4rem;
		flex-wrap: wrap;
		margin-top: 0.3rem;
	}
	.failed {
		color: #b42318;
	}
	.warn {
		color: #b25e00;
	}
</style>
