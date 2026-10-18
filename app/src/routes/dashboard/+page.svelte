<script lang="ts">
	// The dashboard (T3.3). Every figure is an fm-core result: this page formats and lays
	// out, it never adds anything up. Pending transactions are their own line, never in a
	// total; Needs review is its own line and feeds the review queue (SPEC decision 42).
	import TransactionList from './TransactionList.svelte';
	import BackupReminder from '../onboarding/BackupReminder.svelte';
	import { dashboard, errorText, money, type Dashboard, type Line } from './ledger';

	let month = $state<string | null>(null);
	let currency = $state<string | null>(null);
	let view = $state<Dashboard | null>(null);
	let error = $state('');
	let open = $state<Set<number>>(new Set());
	let drill = $state<{ title: string; ids: number[] } | null>(null);

	$effect(() => {
		const m = month;
		const c = currency;
		error = '';
		dashboard(m, c)
			.then((v) => {
				view = v;
				if (month === null) month = v.month;
				if (currency === null) currency = v.currency;
			})
			.catch((e) => (error = errorText(e)));
	});

	function show(title: string, ids: number[]) {
		drill = { title, ids };
	}

	function toggle(id: number | null) {
		if (id === null) return;
		const next = new Set(open);
		if (next.has(id)) next.delete(id);
		else next.add(id);
		open = next;
	}

	const lines: [string, keyof Omit<NonNullable<Dashboard['totals']>, 'netFlow'>][] = [
		['Spending', 'spending'],
		['Refunds (already netted in spending)', 'refunds'],
		['Income', 'income'],
		['Transfers out', 'transfersOut'],
		['Transfers in', 'transfersIn']
	];
</script>

<main>
	<nav>
		<a href="/">Home</a> · <a href="/review">Review</a> · <a href="/categories">Categories</a> ·
		<a href="/import">Import</a>
	</nav>
	<h1>Dashboard</h1>
	<BackupReminder />

	{#if error}
		<p role="alert">{error}</p>
	{/if}

	{#if view && view.currency === null}
		<p>No transactions yet. <a href="/import">Import a bank file</a> to start.</p>
	{:else if view && view.totals}
		<div class="controls">
			<label>
				Month
				<select bind:value={month}>
					{#each view.months as m (m)}
						<option value={m}>{m}</option>
					{/each}
				</select>
			</label>
			{#if view.currencies.length > 1}
				<label>
					Currency
					<select bind:value={currency}>
						{#each view.currencies as c (c)}
							<option value={c}>{c}</option>
						{/each}
					</select>
				</label>
			{/if}
			{#if view.partial}
				<span class="badge">month not over yet</span>
			{/if}
			<span class="muted">as of {view.asOf}</span>
		</div>

		<section>
			<h2>{view.month}</h2>
			<table class="totals">
				<tbody>
					{#each lines as [label, key] (key)}
						{@const line: Line = view.totals[key]}
						<tr>
							<th>{label}</th>
							<td class="num">
								<button type="button" class="link" onclick={() => show(label, line.transactionIds)}>
									{money(line.amount)}
								</button>
							</td>
							<td class="muted">{line.count} transactions</td>
							<td class="muted">
								{#if key === 'spending' && view.spendingChange}
									{money(view.spendingChange.change, { signed: true })}
									{#if view.spendingChange.percent}({view.spendingChange.percent}){/if}
									vs the month before
								{/if}
							</td>
						</tr>
					{/each}
					<tr class="strong">
						<th>Net flow</th>
						<td class="num">{money(view.totals.netFlow)}</td>
						<td></td>
						<td class="muted">every posted amount in and out</td>
					</tr>
					<tr class="review">
						<th>Needs review</th>
						<td class="num">
							<button
								type="button"
								class="link"
								onclick={() => show('Needs review', view!.totals!.needsReview.transactionIds)}
							>
								{money(view.totals.needsReview.amount)}
							</button>
						</td>
						<td class="muted">{view.totals.needsReview.count} transactions</td>
						<td><a href="/review">Review them</a></td>
					</tr>
				</tbody>
			</table>
			<p class="pending">
				Pending (not in any total):
				<button
					type="button"
					class="link"
					onclick={() => show('Pending', view!.totals!.pending.transactionIds)}
				>
					{money(view.totals.pending.amount)}
				</button>
				<span class="muted">· {view.totals.pending.count} transactions</span>
			</p>
		</section>

		{#if drill}
			<div class="drill">
				<button type="button" class="close" onclick={() => (drill = null)}>Close</button>
				<TransactionList ids={drill.ids} title={drill.title} />
			</div>
		{/if}

		<section>
			<h2>Spending by group</h2>
			<table>
				<thead>
					<tr><th>Group</th><th class="num">Spent</th><th class="num">Refunds</th><th>vs month before</th></tr>
				</thead>
				<tbody>
					{#each view.groups as g (g.groupId ?? g.row.label)}
						<tr>
							<td>
								<button type="button" class="link" onclick={() => toggle(g.groupId)}>
									{open.has(g.groupId ?? -1) ? '▾' : '▸'} {g.row.label}
								</button>
							</td>
							<td class="num">
								<button type="button" class="link" onclick={() => show(g.row.label, g.row.transactionIds)}>
									{money(g.row.total)}
								</button>
							</td>
							<td class="num muted">{money(g.row.refunds)}</td>
							<td class="muted">
								{#if g.change}
									{money(g.change.change, { signed: true })}
									{#if g.change.percent}({g.change.percent}){/if}
								{/if}
							</td>
						</tr>
						{#if g.groupId !== null && open.has(g.groupId)}
							{#each g.categories as c (c.categoryId)}
								<tr class="sub">
									<td>{c.row.label}</td>
									<td class="num">
										<button type="button" class="link" onclick={() => show(c.row.label, c.row.transactionIds)}>
											{money(c.row.total)}
										</button>
									</td>
									<td class="num muted">{money(c.row.refunds)}</td>
									<td class="muted">{c.row.count} transactions</td>
								</tr>
							{/each}
						{/if}
					{:else}
						<tr><td colspan="4" class="muted">No spending this month.</td></tr>
					{/each}
				</tbody>
			</table>
		</section>

		<section>
			<h2>Top merchants</h2>
			<table>
				<tbody>
					{#each view.merchants as m (m.label)}
						<tr>
							<td>
								{m.label}
								{#if m.fromDescription}<span class="muted">(bank description)</span>{/if}
							</td>
							<td class="num">
								<button type="button" class="link" onclick={() => show(m.label, m.transactionIds)}>
									{money(m.total)}
								</button>
							</td>
							<td class="muted">{m.count} transactions</td>
						</tr>
					{/each}
					{#if view.merchantsOther}
						<tr>
							<td class="muted">Everything else</td>
							<td class="num">{money(view.merchantsOther.total)}</td>
							<td class="muted">{view.merchantsOther.count} transactions</td>
						</tr>
					{/if}
				</tbody>
			</table>
		</section>

		{#if view.trend}
			<section>
				<h2>12 months</h2>
				<div class="scroll">
					<table class="trend">
						<thead>
							<tr>
								<th>Group</th>
								{#each view.trend.months as m (m.month)}
									<th class="num">{m.month.slice(2)}{m.partial ? '*' : ''}</th>
								{/each}
								<th class="num">Average</th>
								<th class="num">Median</th>
							</tr>
						</thead>
						<tbody>
							{#each [...view.trend.rows, view.trend.total] as r (r.label)}
								<tr class:strong={r === view.trend.total}>
									<td>{r.label}</td>
									{#each r.values as v, i (i)}
										<td class="num">{money(v)}</td>
									{/each}
									<td class="num">{r.average ? money(r.average) : '–'}</td>
									<td class="num">{r.median ? money(r.median) : '–'}</td>
								</tr>
							{/each}
						</tbody>
					</table>
				</div>
				<p class="muted">* month not over yet; averages and medians use complete months only.</p>
			</section>
		{/if}

		<section>
			<h2>Subscriptions and bills</h2>
			<table>
				<thead>
					<tr><th>Name</th><th>How often</th><th class="num">Latest</th><th>Next</th><th>Account</th></tr>
				</thead>
				<tbody>
					{#each view.recurring as r (r.name + r.last + r.cadence)}
						<tr class:ended={r.ended}>
							<td>
								<button type="button" class="link" onclick={() => show(r.name, r.transactionIds)}>{r.name}</button>
							</td>
							<td>{r.cadence}</td>
							<td class="num">{r.variable ? 'about ' : ''}{money(r.amount)}</td>
							<td>{r.ended ? `ended (last ${r.last})` : (r.nextExpected ?? '')}</td>
							<td class="muted">{r.accounts.join(', ')}</td>
						</tr>
					{:else}
						<tr><td colspan="5" class="muted">None found yet.</td></tr>
					{/each}
				</tbody>
			</table>
		</section>
	{:else if !error}
		<p class="muted">Loading…</p>
	{/if}
</main>

<style>
	main {
		font-family: system-ui, sans-serif;
		margin: 2rem;
		max-width: 64rem;
	}
	section {
		margin: 1.5rem 0;
	}
	.controls {
		display: flex;
		gap: 1rem;
		align-items: end;
		flex-wrap: wrap;
	}
	label {
		display: grid;
		gap: 0.25rem;
	}
	table {
		border-collapse: collapse;
		width: 100%;
	}
	th,
	td {
		text-align: left;
		padding: 0.3rem 0.5rem;
		border-bottom: 1px solid color-mix(in srgb, currentColor 12%, transparent);
	}
	.num {
		text-align: right;
		white-space: nowrap;
	}
	.muted {
		color: GrayText;
	}
	.strong th,
	.strong td {
		font-weight: 600;
	}
	.review th {
		color: #b25e00;
	}
	.sub td:first-child {
		padding-left: 2rem;
	}
	.pending {
		border: 1px dashed color-mix(in srgb, currentColor 30%, transparent);
		padding: 0.5rem 0.75rem;
		border-radius: 6px;
	}
	.badge {
		font-size: 0.8em;
		padding: 0.1rem 0.4rem;
		border: 1px solid currentColor;
		border-radius: 3px;
	}
	button.link {
		background: none;
		border: none;
		padding: 0;
		color: LinkText;
		cursor: pointer;
		font: inherit;
		text-align: inherit;
	}
	.drill {
		border: 1px solid color-mix(in srgb, currentColor 20%, transparent);
		border-radius: 6px;
		padding: 0.5rem 1rem;
	}
	.close {
		float: right;
	}
	.scroll {
		overflow-x: auto;
	}
	.trend {
		font-size: 0.85em;
	}
	tr.ended {
		opacity: 0.6;
	}
</style>
