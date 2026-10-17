<script lang="ts">
	// A list of transactions by id (a drill-down). Viewing changes nothing: decisions count
	// as seen only once you act on them (SPEC decision 46).
	import { errorText, money, transactions, type Transaction } from './ledger';

	let { ids, title = 'Transactions' }: { ids: number[]; title?: string } = $props();

	let rows = $state<Transaction[] | null>(null);
	let error = $state('');

	$effect(() => {
		const wanted = ids;
		rows = null;
		error = '';
		transactions(wanted)
			.then((r) => (rows = r))
			.catch((e) => (error = errorText(e)));
	});
</script>

<section class="drill">
	<h3>{title} <span class="muted">({ids.length})</span></h3>
	{#if error}
		<p role="alert">{error}</p>
	{:else if rows === null}
		<p class="muted">Loading…</p>
	{:else if rows.length === 0}
		<p class="muted">None.</p>
	{:else}
		<table>
			<thead>
				<tr>
					<th>Date</th>
					<th>Description</th>
					<th>Account</th>
					<th>Counts as</th>
					<th class="num">Amount</th>
				</tr>
			</thead>
			<tbody>
				{#each rows as t (t.id)}
					<tr class:pending={t.pending}>
						<td class="nowrap">{t.date}</td>
						<td>
							{#if t.merchant}<strong>{t.merchant}</strong><br />{/if}
							<span class="muted">{t.description}</span>
							{#if t.pending}<span class="badge">pending · not counted</span>{/if}
						</td>
						<td>{t.account}</td>
						<td>
							{t.bucket.label}
							{#if t.bucketSource}<br /><span class="muted">by {t.bucketSource}</span>{/if}
						</td>
						<td class="num nowrap">{money(t.amount)}</td>
					</tr>
				{/each}
			</tbody>
		</table>
	{/if}
</section>

<style>
	.drill {
		margin: 1rem 0;
	}
	table {
		border-collapse: collapse;
		width: 100%;
		font-size: 0.9em;
	}
	th,
	td {
		text-align: left;
		padding: 0.3rem 0.5rem;
		border-bottom: 1px solid color-mix(in srgb, currentColor 15%, transparent);
		vertical-align: top;
	}
	.num {
		text-align: right;
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
	tr.pending {
		opacity: 0.75;
	}
</style>
