<script lang="ts">
	// The review queue (T3.3): Needs-review transactions to file, each correction learning a
	// rule that re-applies to the merchant's past and future transactions; the model's
	// decisions you haven't acted on yet, to Approve or Reject one by one or several at once;
	// and suggestions (changes to decisions you've already acted on) to accept or dismiss.
	// Viewing marks nothing seen; only acting does (SPEC decision 46).
	import {
		acceptSuggestion,
		approveDecisions,
		errorCode,
		rejectDecisions,
		categoryPicker,
		correctTransaction,
		dismissSuggestion,
		errorText,
		money,
		previewRule,
		reviewQueue,
		suggestions,
		type BucketChoice,
		type CorrectionResult,
		type PickerGroup,
		type Queue,
		type RulePreview,
		type Transaction
	} from '../dashboard/ledger';
	import { reloadOnProgress } from '../dashboard/categoriser';

	const PAGE = 50;

	let queue = $state<Queue | null>(null);
	let offset = $state(0);
	let picker = $state<PickerGroup[]>([]);
	let pending = $state<Transaction[]>([]);
	let selected = $state<Transaction | null>(null);
	let choice = $state('');
	let merchant = $state('');
	let learn = $state(true);
	let pattern = $state('');
	/** The pattern fm-core would learn by default; an edited one is sent explicitly. */
	let defaultPattern = $state<string | null>(null);
	let preview = $state<RulePreview | null>(null);
	let result = $state<CorrectionResult | null>(null);
	let error = $state('');
	let busy = $state(false);
	let picked = $state<Set<number>>(new Set());
	let bulkMessage = $state('');

	const pickable = $derived(queue?.transactions.filter((t) => t.awaitsConfirmation) ?? []);
	// Money going out is never income or a refund (the core refuses it too).
	const moneyOut = $derived(selected !== null && selected.amount.minor < 0);
	const pickedPairs = $derived(
		pickable.filter((t) => picked.has(t.id) && t.pairedWith !== null).length
	);
	const allPicked = $derived(pickable.length > 0 && pickable.every((t) => picked.has(t.id)));

	function pick(id: number, on: boolean) {
		const next = new Set(picked);
		if (on) next.add(id);
		else next.delete(id);
		picked = next;
	}

	async function bulk(approve: boolean) {
		// The decisions on screen, and for Approve the other side of each transfer pair.
		const ids = pickable
			.filter((t) => picked.has(t.id))
			.flatMap((t) => [t.bucketDecisionId, approve ? (t.pairedWith?.decisionId ?? null) : null])
			.filter((id): id is number => id !== null);
		if (ids.length === 0) return;
		busy = true;
		error = '';
		bulkMessage = '';
		try {
			const n = approve ? await approveDecisions(ids) : await rejectDecisions(ids);
			bulkMessage = approve
				? `Approved ${n}: they keep the suggested category or transfer.`
				: `Rejected ${n}: they're back in Needs review, and the same answer won't be suggested again.`;
			picked = new Set();
			await load();
		} catch (e) {
			if (errorCode(e) === 'Changed') {
				// Show what is current now, then say why nothing was done.
				picked = new Set();
				await load();
			}
			error = errorText(e);
		} finally {
			busy = false;
		}
	}

	async function load() {
		error = '';
		try {
			[queue, picker, pending] = await Promise.all([
				reviewQueue(offset, PAGE),
				picker.length ? Promise.resolve(picker) : categoryPicker(),
				suggestions()
			]);
		} catch (e) {
			error = errorText(e);
		}
	}

	$effect(() => {
		void offset;
		load();
	});

	// The model's batches land in the queue as they are written (T4.8).
	$effect(() => reloadOnProgress(() => load()));

	async function select(t: Transaction) {
		selected = t;
		result = null;
		choice = t.bucket.categoryId !== null ? `category:${t.bucket.categoryId}` : '';
		merchant = t.merchant ?? '';
		learn = true;
		try {
			preview = await previewRule(t.id, null);
			defaultPattern = preview.pattern;
			pattern = preview.pattern ?? '';
		} catch (e) {
			error = errorText(e);
		}
	}

	// Re-count as the pattern is edited.
	$effect(() => {
		const t = selected;
		const p = pattern;
		if (!t || !learn || p.trim() === '') return;
		const timer = setTimeout(() => {
			previewRule(t.id, p)
				.then((r) => (preview = r))
				.catch(() => (preview = { pattern: null, matches: 0 }));
		}, 250);
		return () => clearTimeout(timer);
	});

	function bucket(): BucketChoice | null {
		if (choice === 'income') return { kind: 'income' };
		if (choice === 'transfer') return { kind: 'transfer' };
		const [kind, id] = choice.split(':');
		if (kind === 'category' || kind === 'refund') return { kind, categoryId: Number(id) };
		return null;
	}

	async function save() {
		if (!selected) return;
		const b = bucket();
		const name = merchant.trim();
		const renamed = name !== '' && name !== (selected.merchant ?? '') ? name : null;
		if (!b && !renamed) {
			error = 'Choose where it counts, or a new merchant name.';
			return;
		}
		busy = true;
		error = '';
		try {
			result = await correctTransaction({
				transactionId: selected.id,
				bucket: b,
				merchant: renamed,
				learn,
				pattern: learn && pattern.trim() !== '' && pattern !== defaultPattern ? pattern : null
			});
			selected = null;
			await load();
		} catch (e) {
			error = errorText(e);
		} finally {
			busy = false;
		}
	}

	async function act(action: (id: number) => Promise<number[]>, id: number) {
		error = '';
		try {
			await action(id);
			await load();
		} catch (e) {
			error = errorText(e);
		}
	}
</script>

<main>
	<nav>
		<a href="/">Home</a> · <a href="/dashboard">Dashboard</a> · <a href="/categories">Categories</a> ·
		<a href="/import">Import</a>
	</nav>
	<h1>Review</h1>

	{#if error}
		<p role="alert">{error}</p>
	{/if}
	{#if bulkMessage}
		<p role="status" class="done">{bulkMessage}</p>
	{/if}
	{#if result}
		<p role="status" class="done">
			Saved.
			{#if result.ruleId !== null}
				The new rule re-applied to {result.reapplied}
				{result.reapplied === 1 ? 'transaction' : 'transactions'}, past and future ones included.
			{:else if result.noPattern}
				This description names no merchant, so no rule was learned.
			{/if}
			{#if result.ruleDidNotFit > 0}
				{result.ruleDidNotFit} matching
				{result.ruleDidNotFit === 1 ? 'transaction was' : 'transactions were'} left as they were:
				the rule didn't fit them.
			{/if}
			{#if result.unpaired.length > 0}
				{result.unpaired.length} transfer {result.unpaired.length === 1 ? 'side was' : 'sides were'}
				unpaired and moved to Needs review.
			{/if}
		</p>
	{/if}

	{#if pending.length > 0}
		<section>
			<h2>Suggestions <span class="muted">({pending.length})</span></h2>
			<p class="muted">
				Changes to transactions you've already confirmed or corrected. Nothing changes until you accept one.
			</p>
			<table>
				<tbody>
					{#each pending as t (t.id)}
						{#each t.suggestions as s (s.decisionId)}
							<tr>
								<td class="nowrap">{t.date}</td>
								<td>{t.merchant ?? t.description}</td>
								<td class="num">{money(t.amount)}</td>
								<td>{t.bucket.label} → <strong>{s.label}</strong> <span class="muted">({s.source})</span></td>
								<td class="nowrap">
									<button type="button" onclick={() => act(acceptSuggestion, s.decisionId)}>Accept</button>
									<button type="button" onclick={() => act(dismissSuggestion, s.decisionId)}>Dismiss</button>
								</td>
							</tr>
						{/each}
					{/each}
				</tbody>
			</table>
		</section>
	{/if}

	<section>
		<h2>Needs review {#if queue}<span class="muted">({queue.total})</span>{/if}</h2>
		{#if queue && queue.total === 0}
			<p>Nothing to review.</p>
		{:else if queue}
			{#if pickable.length > 0}
				<div class="bulk">
					<label class="check">
						<input
							type="checkbox"
							checked={allPicked}
							onchange={(e) => {
								const on = (e.currentTarget as HTMLInputElement).checked;
								picked = new Set(on ? pickable.map((t) => t.id) : []);
							}}
						/>
						Select everything waiting for approval on this page
					</label>
					<button type="button" disabled={busy || picked.size === 0} onclick={() => bulk(true)}>
						Approve {picked.size || ''}
					</button>
					<button type="button" disabled={busy || picked.size === 0} onclick={() => bulk(false)}>
						Reject {picked.size || ''}
					</button>
					<span class="muted">
						Approve keeps the suggested category or transfer; Reject sends it back to Needs review,
						and that answer won't come back (the model may still suggest a different one).
						{#if pickedPairs > 0}
							{pickedPairs} of the selected {pickedPairs === 1 ? 'is a transfer' : 'are transfers'}: the
							matching side in the other account changes too.
						{/if}
					</span>
				</div>
			{/if}
			<table>
				<thead>
					<tr><th></th><th>Date</th><th>Description</th><th>Account</th><th>Counts as</th><th class="num">Amount</th><th></th></tr>
				</thead>
				<tbody>
					{#each queue.transactions as t (t.id)}
						<tr class:current={selected?.id === t.id}>
							<td>
								{#if t.awaitsConfirmation}
									<input
										type="checkbox"
										aria-label="Select"
										checked={picked.has(t.id)}
										onchange={(e) => pick(t.id, (e.currentTarget as HTMLInputElement).checked)}
									/>
								{/if}
							</td>
							<td class="nowrap">{t.date}</td>
							<td>
								{#if t.merchant}<strong>{t.merchant}</strong><br />{/if}
								<span class="muted">{t.description}</span>
								{#if t.pending}<span class="badge">pending</span>{/if}
							</td>
							<td>{t.account}</td>
							<td>
								{#if t.awaitsConfirmation}
									{t.bucket.label}
									<span class="muted">
										· {t.bucketSource ?? 'model'}{t.bucketConfidence ? `, ${t.bucketConfidence} sure` : ''}
									</span>
									{#if t.pairedWith}
										<br /><span class="muted">also applies to the matching transfer in {t.pairedWith.account}</span>
									{/if}
								{:else}
									<span class="muted">Needs review</span>
								{/if}
							</td>
							<td class="num">{money(t.amount)}</td>
							<td><button type="button" onclick={() => select(t)}>{t.awaitsConfirmation ? 'Change…' : 'File…'}</button></td>
						</tr>
						{#if selected?.id === t.id}
							<tr class="editor">
								<td colspan="7">
									<form
										onsubmit={(e) => {
											e.preventDefault();
											save();
										}}
									>
										<label>
											Counts as
											<select bind:value={choice}>
												<option value="">(leave as it is)</option>
												{#if !moneyOut}<option value="income">Income</option>{/if}
												<option value="transfer">Transfer between my accounts</option>
												{#each picker as g (g.groupId)}
													<optgroup label={g.name}>
														{#each g.categories as c (c.id)}
															<option value={`category:${c.id}`}>{c.name}</option>
														{/each}
													</optgroup>
												{/each}
												{#if !moneyOut}
													{#each picker as g (g.groupId)}
														<optgroup label={`Refund · ${g.name}`}>
															{#each g.categories as c (c.id)}
																<option value={`refund:${c.id}`}>Refund · {c.name}</option>
															{/each}
														</optgroup>
													{/each}
												{/if}
											</select>
										</label>
										<label>
											Merchant name
											<input bind:value={merchant} placeholder="e.g. Blue Bottle Coffee" />
										</label>
										<label class="check">
											<input type="checkbox" bind:checked={learn} />
											Do the same for similar transactions (learn a rule)
										</label>
										{#if learn}
											<label>
												Rule matches descriptions containing
												<input bind:value={pattern} />
											</label>
											<p class="muted">
												{#if preview?.pattern === null}
													This description names no merchant, so no rule will be learned.
												{:else if preview}
													This rule matches {preview.matches}
													{preview.matches === 1 ? 'transaction' : 'transactions'}.
												{/if}
											</p>
										{/if}
										<div class="actions">
											<button type="submit" disabled={busy}>Save</button>
											<button type="button" onclick={() => (selected = null)}>Cancel</button>
										</div>
									</form>
								</td>
							</tr>
						{/if}
					{/each}
				</tbody>
			</table>
			<div class="pager">
				<button type="button" disabled={offset === 0} onclick={() => (offset = Math.max(0, offset - PAGE))}>
					Newer
				</button>
				<span class="muted">{offset + 1}–{offset + queue.transactions.length} of {queue.total}</span>
				<button
					type="button"
					disabled={offset + PAGE >= queue.total}
					onclick={() => (offset = offset + PAGE)}
				>
					Older
				</button>
			</div>
		{:else if !error}
			<p class="muted">Loading…</p>
		{/if}
	</section>
</main>

<style>
	main {
		font-family: system-ui, sans-serif;
		margin: 2rem;
		max-width: 60rem;
	}
	section {
		margin: 1.5rem 0;
	}
	table {
		border-collapse: collapse;
		width: 100%;
		font-size: 0.92em;
	}
	th,
	td {
		text-align: left;
		padding: 0.3rem 0.5rem;
		border-bottom: 1px solid color-mix(in srgb, currentColor 12%, transparent);
		vertical-align: top;
	}
	.num {
		text-align: right;
		white-space: nowrap;
	}
	.nowrap {
		white-space: nowrap;
	}
	.muted {
		color: GrayText;
	}
	.badge {
		font-size: 0.8em;
		margin-left: 0.4rem;
		padding: 0 0.3rem;
		border: 1px solid currentColor;
		border-radius: 3px;
	}
	tr.current td {
		font-weight: 600;
	}
	tr.editor td {
		background: color-mix(in srgb, currentColor 4%, transparent);
	}
	form {
		display: grid;
		gap: 0.6rem;
		max-width: 32rem;
		padding: 0.5rem 0;
	}
	label {
		display: grid;
		gap: 0.25rem;
	}
	.bulk {
		flex-wrap: wrap;
		margin-bottom: 0.5rem;
	}
	label.check {
		display: flex;
		gap: 0.5rem;
		align-items: baseline;
	}
	.actions,
	.pager,
	.bulk {
		display: flex;
		gap: 0.5rem;
		align-items: center;
	}
	.done {
		border-left: 3px solid #2e7d32;
		padding-left: 0.75rem;
	}
</style>
