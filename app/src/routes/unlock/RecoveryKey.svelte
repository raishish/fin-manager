<script lang="ts">
	// Shows a new recovery key once, then asks for three of its groups back.
	import { confirmRecoveryKey, errorMessage, type RecoveryToConfirm } from './api';

	let { recovery, ondone }: { recovery: RecoveryToConfirm; ondone: () => void } = $props();

	let step = $state<'show' | 'confirm'>('show');
	let saved = $state(false);
	let answers = $state(['', '', '']);
	let error = $state('');
	let busy = $state(false);

	async function confirm(event: SubmitEvent) {
		event.preventDefault();
		busy = true;
		error = '';
		try {
			await confirmRecoveryKey(answers);
			ondone();
		} catch (e) {
			error = errorMessage(e);
		} finally {
			busy = false;
		}
	}
</script>

{#if step === 'show'}
	<h2>Your recovery key</h2>
	<p>
		This key opens your data if you forget your password. It is shown <strong>only now</strong>.
		Write it down or print it, and keep it somewhere safe and offline.
	</p>
	<p class="warning">
		If you lose both your password and this key, your data can't be recovered by anyone,
		including us.
	</p>
	<ol class="groups">
		{#each recovery.groups as group, i (i)}
			<li><span class="n">{i + 1}</span><code>{group}</code></li>
		{/each}
	</ol>
	<label class="check">
		<input type="checkbox" bind:checked={saved} />
		I have written down or printed my recovery key.
	</label>
	<button disabled={!saved} onclick={() => (step = 'confirm')}>Continue</button>
{:else}
	<h2>Check your recovery key</h2>
	<p>Type these groups from the key you saved. Case and the letters O and I don't matter.</p>
	<form onsubmit={confirm}>
		{#each recovery.challenge as group, i (group)}
			<label>
				Group {group + 1}
				<input
					bind:value={answers[i]}
					maxlength="8"
					autocomplete="off"
					autocapitalize="characters"
					spellcheck="false"
				/>
			</label>
		{/each}
		{#if error}<p class="error" role="alert">{error}</p>{/if}
		<div class="actions">
			<button type="button" class="secondary" onclick={() => (step = 'show')}>
				Show the key again
			</button>
			<button type="submit" disabled={busy || answers.some((a) => a.trim() === '')}>
				Confirm
			</button>
		</div>
	</form>
{/if}

<style>
	.groups {
		list-style: none;
		padding: 0;
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(7.5rem, 1fr));
		gap: 0.5rem;
		user-select: text;
	}
	.groups li {
		display: flex;
		gap: 0.5rem;
		align-items: baseline;
	}
	.n {
		font-size: 0.75rem;
		color: var(--muted-fg, #666);
		min-width: 1.25rem;
		text-align: right;
	}
	code {
		font-size: 1.15rem;
		letter-spacing: 0.08em;
	}
	.warning {
		color: #8a4b00;
	}
	.check {
		display: flex;
		gap: 0.5rem;
		margin: 1rem 0;
	}
	form label {
		display: grid;
		gap: 0.25rem;
		margin-top: 0.75rem;
		max-width: 12rem;
	}
	.actions {
		display: flex;
		gap: 0.5rem;
		margin-top: 1rem;
	}
	.error {
		color: #b00020;
	}
</style>
