<script lang="ts">
	// The first insight (T5.5, SPEC decision 34), from fixed logic, before the model or any
	// category: money in and out by month, where the money went, subscriptions and bills,
	// transfers between your accounts. Every figure is fm-core's (views::first_look).
	// "Money out", not "spending": refunds aren't known before categorisation.
	import TransactionList from '../dashboard/TransactionList.svelte';
	import { errorText, money } from '../dashboard/ledger';
	import { firstInsight, type Insight } from './api';

	let insight = $state<Insight | null>(null);
	let error = $state('');
	let drill = $state<{ title: string; ids: number[] } | null>(null);

	$effect(() => {
		firstInsight()
			.then((i) => (insight = i))
			.catch((e) => (error = errorText(e)));
	});
</script>

{#if error}
	<p role="alert">{error}</p>
{:else if !insight}
	<p class="muted">Working it out…</p>
{:else if insight.empty}
	<p>No transactions yet. Go back a step and bring in a bank file.</p>
{:else}
	<p class="lead">
		{insight.firstMonth} to {insight.lastMonth}: {money(insight.moneyOut!.amount)} went out and
		{money(insight.moneyIn!.amount)} came in, not counting {insight.transferPairs} transfers between
		your own accounts.
	</p>

	<h3>Money in and out by month <span class="muted">(excluding transfers)</span></h3>
	<table>
		<thead>
			<tr><th>Month</th><th class="num">Money in</th><th class="num">Money out</th><th class="num">Pending</th></tr>
		</thead>
		<tbody>
			{#each insight.months as m (m.month)}
				<tr>
					<td>{m.month}{m.partial ? ' (so far)' : ''}</td>
					<td class="num">{money(m.moneyIn.amount)}</td>
					<td class="num">
						<button type="button" class="link" onclick={() => (drill = { title: `Money out, ${m.month}`, ids: m.moneyOut.transactionIds })}>
							{money(m.moneyOut.amount)}
						</button>
					</td>
					<td class="num muted">{m.pending.count > 0 ? money(m.pending.amount) : ''}</td>
				</tr>
			{/each}
		</tbody>
	</table>
	<p class="muted">Pending transactions aren't in any total until they post.</p>

	<h3>Where the money went</h3>
	<table>
		<tbody>
			{#each insight.merchants as m (m.label)}
				<tr>
					<td>{m.label}</td>
					<td class="num">
						<button type="button" class="link" onclick={() => (drill = { title: m.label, ids: m.transactionIds })}>
							{money(m.moneyOut)}
						</button>
					</td>
					<td class="muted">{m.count}×</td>
				</tr>
			{/each}
			{#if insight.merchantsOther}
				<tr>
					<td class="muted">Everyone else</td>
					<td class="num">{money(insight.merchantsOther.moneyOut)}</td>
					<td class="muted">{insight.merchantsOther.count}×</td>
				</tr>
			{/if}
		</tbody>
	</table>

	<h3>Subscriptions and bills</h3>
	{#if insight.recurring.length === 0}
		<p class="muted">None found yet; they show up from the third charge.</p>
	{:else}
		<ul class="recurring">
			{#each insight.recurring.filter((r) => !r.ended) as r (r.name + r.last)}
				<li>{r.name}: {r.variable ? 'about ' : ''}{money(r.amount)} {r.cadence}</li>
			{/each}
		</ul>
	{/if}

	{#if drill}
		<button type="button" onclick={() => (drill = null)}>Close</button>
		<TransactionList ids={drill.ids} title={drill.title} />
	{/if}
{/if}

<style>
	table {
		border-collapse: collapse;
		width: 100%;
		max-width: 40rem;
	}
	td,
	th {
		text-align: left;
		padding: 0.25rem 0.5rem;
		border-bottom: 1px solid color-mix(in srgb, currentColor 12%, transparent);
	}
	.num {
		text-align: right;
		white-space: nowrap;
	}
	.muted {
		color: GrayText;
	}
	.lead {
		font-size: 1.1em;
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
