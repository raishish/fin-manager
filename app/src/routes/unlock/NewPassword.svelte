<script lang="ts">
	// A new master password, typed twice, with a live strength check from the core.
	import { passwordStrength, type Strength } from './api';

	let {
		password = $bindable(''),
		ok = $bindable(false),
		label = 'New password'
	}: { password?: string; ok?: boolean; label?: string } = $props();

	let repeat = $state('');
	let strength = $state<Strength | null>(null);
	let request = 0;

	$effect(() => {
		const value = password;
		const mine = ++request;
		if (value === '') {
			strength = null;
			return;
		}
		const timer = setTimeout(async () => {
			const result = await passwordStrength(value);
			if (mine === request) strength = result;
		}, 150);
		return () => clearTimeout(timer);
	});

	const matches = $derived(repeat === password);
	$effect(() => {
		ok = !!strength?.acceptable && matches && password !== '';
	});

	const labels = ['Very weak', 'Weak', 'Fair', 'Good', 'Strong'];
</script>

<label>
	{label}
	<input type="password" autocomplete="new-password" bind:value={password} />
</label>

{#if strength}
	<div class="meter" aria-hidden="true">
		{#each [0, 1, 2, 3] as i (i)}
			<span class:on={strength.score > i} class:good={strength.acceptable}></span>
		{/each}
	</div>
	<p class="rating" aria-live="polite">
		{labels[strength.score]}{strength.acceptable ? '' : ': not strong enough yet'}
	</p>
	{#if !strength.acceptable}
		<div class="advice">
			{#if strength.warning}<p>{strength.warning}</p>{/if}
			{#each strength.suggestions as tip (tip)}<p>{tip}</p>{/each}
			<p>
				Try a passphrase: four or more unrelated words, such as
				<em>velvet anchor mosaic tundra</em>. Long and memorable beats short and complex.
			</p>
		</div>
	{/if}
{/if}

<label>
	Type it again
	<input type="password" autocomplete="new-password" bind:value={repeat} />
</label>
{#if repeat !== '' && !matches}
	<p class="error" role="alert">The two passwords don't match.</p>
{/if}

<style>
	label {
		display: grid;
		gap: 0.25rem;
		margin-top: 0.75rem;
	}
	.meter {
		display: grid;
		grid-template-columns: repeat(4, 1fr);
		gap: 4px;
		margin-top: 0.5rem;
	}
	.meter span {
		height: 6px;
		border-radius: 3px;
		background: var(--muted-bg, #ddd);
	}
	.meter span.on {
		background: #c77700;
	}
	.meter span.on.good {
		background: #2e7d32;
	}
	.rating {
		margin: 0.25rem 0 0;
		font-size: 0.9rem;
	}
	.advice {
		font-size: 0.9rem;
		color: var(--muted-fg, #555);
	}
	.advice p {
		margin: 0.25rem 0;
	}
	.error {
		color: #b00020;
	}
</style>
