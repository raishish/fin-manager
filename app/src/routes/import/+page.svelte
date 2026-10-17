<script lang="ts">
	import ImportFile from './ImportFile.svelte';
</script>

<main>
	<nav>
		<a href="/">Home</a> · <a href="/dashboard">Dashboard</a> · <a href="/review">Review</a> ·
		<a href="/categories">Categories</a>
	</nav>
	<h1>Import a bank file</h1>
	<p class="muted">
		Download a CSV, OFX or QFX export from your bank's website, then choose it here. The file
		stays on this computer; it is stored, encrypted, with your ledger.
	</p>
	<ImportFile />
</main>

<style>
	main {
		font-family: system-ui, sans-serif;
		margin: 2rem;
		max-width: 48rem;
	}
	.muted {
		color: GrayText;
	}
</style>
