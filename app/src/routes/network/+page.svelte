<script lang="ts">
	import { onMount } from 'svelte';
	import {
		describeOutcome,
		networkAllowlist,
		networkLog,
		wasSent,
		type NetworkLogEntry
	} from './api';

	// The guard keeps this many entries; the page keeps the same.
	const MAX_ENTRIES = 10_000;
	const POLL_MS = 2000;

	type Filter = 'all' | 'sent' | 'stopped';

	let entries = $state<NetworkLogEntry[]>([]);
	let origins = $state<string[] | null>(null);
	let error = $state<string | null>(null);
	let filter = $state<Filter>('all');
	let busy = false;

	const sentCount = $derived(entries.filter((e) => wasSent(e.outcome)).length);
	const shown = $derived(
		entries
			.filter((e) => filter === 'all' || wasSent(e.outcome) === (filter === 'sent'))
			.toReversed()
	);

	async function refresh() {
		if (busy) return;
		busy = true;
		try {
			const newer = await networkLog(entries.at(-1)?.id);
			if (newer.length > 0) entries = [...entries, ...newer].slice(-MAX_ENTRIES);
			error = null;
		} catch (e) {
			error = String(e);
		} finally {
			busy = false;
		}
	}

	onMount(() => {
		networkAllowlist()
			.then((list) => (origins = list))
			.catch((e) => (error = String(e)));
		refresh();
		const timer = setInterval(refresh, POLL_MS);
		return () => clearInterval(timer);
	});

	const time = (ms: number) => new Date(ms).toLocaleString();
</script>

<main>
	<nav><a href="/">← Home</a> · <a href="/network/crash">Crash reports</a></nav>
	<h1>Network log</h1>
	<p class="lead">
		Every connection the app attempts, when and where. Requests to anywhere not allowed below are
		blocked before anything is sent.
	</p>

	<section>
		<h2>Allowed destinations</h2>
		{#if origins === null}
			<p>Loading…</p>
		{:else if origins.length === 0}
			<p>None. The app can't contact anything.</p>
		{:else}
			<ul class="origins">
				{#each origins as origin (origin)}
					<li><code>{origin}</code></li>
				{/each}
			</ul>
		{/if}
	</section>

	<section>
		<h2>Requests</h2>
		{#if error}
			<p role="alert">Could not read the network log: {error}</p>
		{/if}
		<p>
			{entries.length}
			{entries.length === 1 ? 'request' : 'requests'}: {sentCount} sent, {entries.length -
				sentCount} stopped before sending.
		</p>
		<div class="filters" role="radiogroup" aria-label="Show">
			{#each [['all', 'All'], ['sent', 'Sent'], ['stopped', 'Stopped']] as [value, label] (value)}
				<label>
					<input type="radio" name="filter" {value} bind:group={filter} />
					{label}
				</label>
			{/each}
		</div>

		{#if shown.length === 0}
			<p class="empty">No requests yet.</p>
		{:else}
			<table>
				<thead>
					<tr>
						<th>#</th>
						<th>Time</th>
						<th>Method</th>
						<th>Destination</th>
						<th>Purpose</th>
						<th>Result</th>
					</tr>
				</thead>
				<tbody>
					{#each shown as entry (entry.id)}
						<tr class:stopped={!wasSent(entry.outcome)}>
							<td>{entry.id}</td>
							<td class="nowrap">{time(entry.atUnixMs)}</td>
							<td>{entry.method}</td>
							<td class="destination">
								<code>{entry.origin}{entry.path}{entry.queryOmitted ? '?…' : ''}</code>
								{#if entry.redirectOf !== null}
									<span class="note">redirect from #{entry.redirectOf}</span>
								{/if}
							</td>
							<td>{entry.purpose}</td>
							<td>{describeOutcome(entry.outcome)}</td>
						</tr>
					{/each}
				</tbody>
			</table>
			<p class="note">
				Query strings (<code>?…</code>) are never shown or stored: they can hold tokens.
			</p>
		{/if}
	</section>
</main>

<style>
	main {
		font-family: system-ui, sans-serif;
		margin: 2rem;
		max-width: 72rem;
	}
	.lead,
	.note,
	.empty {
		color: GrayText;
	}
	.note {
		font-size: 0.85em;
	}
	.destination .note {
		display: block;
	}
	.origins {
		padding-left: 1.2rem;
	}
	.filters {
		display: flex;
		gap: 1rem;
		margin-bottom: 0.75rem;
	}
	table {
		border-collapse: collapse;
		width: 100%;
		font-size: 0.9em;
	}
	th,
	td {
		text-align: left;
		padding: 0.35rem 0.5rem;
		border-bottom: 1px solid color-mix(in srgb, currentColor 15%, transparent);
		vertical-align: top;
	}
	td.destination {
		word-break: break-all;
	}
	.nowrap {
		white-space: nowrap;
	}
	tr.stopped td:last-child {
		font-weight: 600;
	}
</style>
