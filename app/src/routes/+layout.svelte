<script lang="ts">
	// App-wide unlock behaviour (T5.1): activity resets the auto-lock timer, and an
	// auto-lock returns to /unlock.
	// File drops (T5.5): a file dropped anywhere but a drop zone would otherwise be opened
	// by the web view, navigating away from the app. Drop zones handle their own drops
	// first; this only cancels the browser's default. The Rust side refuses any such
	// navigation too (lib.rs, navigation_allowed).
	import { onMount } from 'svelte';
	import { installUnlockGuard } from './unlock/guard';

	let { children } = $props();

	onMount(() => {
		const stopGuard = installUnlockGuard();
		const cancel = (e: DragEvent) => e.preventDefault();
		window.addEventListener('dragover', cancel);
		window.addEventListener('drop', cancel);
		return () => {
			stopGuard();
			window.removeEventListener('dragover', cancel);
			window.removeEventListener('drop', cancel);
		};
	});
</script>

{@render children()}
