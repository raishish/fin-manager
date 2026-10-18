<script lang="ts">
	// The model download's progress (T5.5). It runs in the background from the Welcome
	// screen on; nothing waits for it: the first insight comes from fixed logic.
	import { modelStatus, startModelDownload, type ModelStatus } from './api';

	let status = $state<ModelStatus | null>(null);

	$effect(() => {
		let stop = false;
		const tick = async () => {
			try {
				status = await modelStatus();
			} catch {
				// Not fatal: the next tick tries again.
			}
			if (!stop && status?.state !== 'ready') setTimeout(tick, 1000);
		};
		tick();
		return () => (stop = true);
	});

	const mb = (bytes: number) => `${Math.round(bytes / (1024 * 1024))} MB`;
	const pct = (s: { bytes: number; total: number }) =>
		s.total > 0 ? Math.min(100, Math.floor((s.bytes * 100) / s.total)) : 0;

	async function retry() {
		status = await startModelDownload();
	}
</script>

{#if status}
	<p class="model" role="status">
		{#if status.state === 'ready'}
			The local model is ready. Categories fill in from it.
		{:else if status.state === 'downloading'}
			Downloading the local model in the background: {mb(status.bytes)} of {mb(status.total)}
			<progress max="100" value={pct(status)}></progress>
			<span class="muted">You can carry on; categories fill in once it's ready.</span>
		{:else if status.state === 'failed'}
			{#if status.code === 'stopped'}
				The model download paused; it picks up where it left off.
			{:else if status.code === 'busy'}
				Another fin-manager window is downloading the model.
			{:else}
				The model download didn't finish ({status.message}).
			{/if}
			<button type="button" onclick={retry}>Try again</button>
		{:else}
			The local model hasn't been downloaded yet.
			<button type="button" onclick={retry}>Download it</button>
		{/if}
		<a href="/network">See every connection</a>
	</p>
{/if}

<style>
	.model {
		display: flex;
		gap: 0.6rem;
		align-items: center;
		flex-wrap: wrap;
		font-size: 0.9em;
		padding: 0.5rem 0.75rem;
		border: 1px solid color-mix(in srgb, currentColor 15%, transparent);
		border-radius: 6px;
	}
	.muted {
		color: GrayText;
	}
</style>
