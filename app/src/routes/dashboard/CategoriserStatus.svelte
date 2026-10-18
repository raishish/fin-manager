<script lang="ts">
	// A quiet status line while the local model categorises in the background (T4.8):
	// "Categorising N transactions…", then "Filed N, M left for you to review" for a
	// while. Counts only.
	import { onProgress } from './categoriser';
	import { statusText } from './categoriserStatus';

	/** How long the summary stays after a run ends. */
	const SHOW_SUMMARY_MS = 15_000;

	let line = $state<string | null>(null);
	let running = $state(false);

	$effect(() => {
		let hide: ReturnType<typeof setTimeout> | undefined;
		const stop = onProgress((p) => {
			clearTimeout(hide);
			running = p.running;
			line = statusText(p);
			if (!p.running) hide = setTimeout(() => (line = null), SHOW_SUMMARY_MS);
		});
		return () => {
			clearTimeout(hide);
			stop();
		};
	});
</script>

{#if line}
	<p class="status" class:running role="status" aria-live="polite">
		{line}
		{#if !running}<a href="/review">Review</a>{/if}
	</p>
{/if}

<style>
	.status {
		position: fixed;
		bottom: 0.75rem;
		right: 0.75rem;
		margin: 0;
		padding: 0.35rem 0.75rem;
		font: 0.85em system-ui, sans-serif;
		border-radius: 999px;
		background: color-mix(in srgb, Canvas 92%, CanvasText 8%);
		border: 1px solid color-mix(in srgb, CanvasText 15%, transparent);
		display: flex;
		gap: 0.5rem;
		align-items: center;
	}
	.status.running::before {
		content: '';
		width: 0.5rem;
		height: 0.5rem;
		border-radius: 50%;
		background: LinkText;
		opacity: 0.7;
	}
</style>
