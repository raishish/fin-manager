<script lang="ts">
	// App-wide unlock behaviour (T5.1): activity resets the auto-lock timer, and an
	// auto-lock returns to /unlock.
	import { onMount } from 'svelte';
	import { installUnlockGuard } from './unlock/guard';

	let { children } = $props();

	onMount(() => installUnlockGuard());
</script>

{@render children()}
