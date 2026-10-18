<script lang="ts">
	// The home page: where to go, and how much is waiting in Review.
	import { errorText, reviewQueue } from './dashboard/ledger';
	import BackupReminder from './onboarding/BackupReminder.svelte';

	let waiting = $state<number | null>(null);
	let error = $state('');

	$effect(() => {
		// An empty page: just the count.
		reviewQueue(0, 0)
			.then((q) => (waiting = q.total))
			.catch((e) => (error = errorText(e)));
	});
</script>

<main>
	<h1>fin-manager</h1>
	<BackupReminder />
	{#if error}
		<p role="alert">{error}</p>
	{:else if waiting !== null && waiting > 0}
		<p><a href="/review">{waiting} {waiting === 1 ? 'transaction needs' : 'transactions need'} review</a></p>
	{/if}
	<nav>
		<ul>
			<li><a href="/dashboard">Dashboard</a></li>
			<li><a href="/review">Review</a></li>
			<li><a href="/categories">Categories and rules</a></li>
			<li><a href="/import">Import a bank file</a></li>
			<li><a href="/connectors">Bank connections</a></li>
		</ul>
		<ul class="secondary">
			<li><a href="/backup">Backups</a></li>
			<li><a href="/data">Delete data</a></li>
			<li><a href="/unlock/settings">Password and auto-lock</a></li>
			<li><a href="/network">Network log</a></li>
			<li><a href="/network/crash">Crash reports</a></li>
		</ul>
	</nav>
</main>

<style>
	main {
		font-family: system-ui, sans-serif;
		margin: 2rem;
	}
	ul {
		list-style: none;
		padding: 0;
		display: grid;
		gap: 0.4rem;
	}
	.secondary {
		margin-top: 1.5rem;
		font-size: 0.9em;
	}
</style>
