<script lang="ts">
	import { invoke } from '@tauri-apps/api/core';

	type CoreInfo = { name: string; version: string };

	const info = invoke<CoreInfo>('core_info');
</script>

<main>
	<h1>fin-manager</h1>
	{#await info}
		<p>Connecting to the core…</p>
	{:then core}
		<p>Core: <code>{core.name}</code> {core.version}</p>
	{:catch error}
		<p role="alert">Could not reach the core: {String(error)}</p>
	{/await}
</main>

<style>
	main {
		font-family: system-ui, sans-serif;
		margin: 2rem;
	}
</style>
