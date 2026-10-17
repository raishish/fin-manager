<script lang="ts">
	// Security settings while unlocked: recovery key status, auto-lock, password change.
	import { goto } from '$app/navigation';
	import NewPassword from '../NewPassword.svelte';
	import RecoveryKey from '../RecoveryKey.svelte';
	import {
		changePassword,
		errorMessage,
		lockNow,
		setAutolockMinutes,
		unlockState,
		type RecoveryToConfirm,
		type UnlockState
	} from '../api';

	let info = $state<UnlockState | null>(null);
	let current = $state('');
	let newPassword = $state('');
	let newPasswordOk = $state(false);
	let compromised = $state(false);
	let recovery = $state<RecoveryToConfirm | null>(null);
	let message = $state('');
	let error = $state('');
	let busy = $state(false);

	async function refresh() {
		info = await unlockState();
		if (info.status !== 'Unlocked' && info.status !== 'ConfirmRecoveryKey') {
			await goto('/unlock');
		}
	}

	async function savePassword(event: SubmitEvent) {
		event.preventDefault();
		busy = true;
		error = '';
		message = '';
		try {
			recovery = await changePassword(current, newPassword, compromised);
			current = '';
			newPassword = '';
			message = compromised
				? 'Password changed and your data re-encrypted with a new key.'
				: 'Password changed.';
			compromised = false;
			await refresh();
		} catch (e) {
			error = errorMessage(e);
		} finally {
			busy = false;
		}
	}

	async function chooseAutolock(event: Event) {
		const minutes = Number((event.target as HTMLSelectElement).value);
		try {
			await setAutolockMinutes(minutes);
			await refresh();
		} catch (e) {
			error = errorMessage(e);
		}
	}

	$effect(() => {
		refresh().catch((e) => (error = errorMessage(e)));
	});
</script>

<main>
	<h1>Security</h1>

	{#if recovery}
		<RecoveryKey {recovery} ondone={() => ((recovery = null), refresh())} />
	{:else if info}
		<section>
			<h2>Recovery key</h2>
			{#if info.hasRecoveryKey}
				<p>✓ A recovery key is set up. It was shown once, when it was made.</p>
			{:else}
				<p class="error">No recovery key is set up. Lock and unlock with your password to make one.</p>
			{/if}
		</section>

		<section>
			<h2>Backups</h2>
			<p><a href="/backup">Backup folder and restore</a></p>
			<p><a href="/data">Delete data</a></p>
		</section>

		<section>
			<h2>Auto-lock</h2>
			<label>
				Lock after
				<select value={info.autolockMinutes} onchange={chooseAutolock}>
					{#each info.autolockChoices as m (m)}
						<option value={m}>{m === 60 ? '1 hour' : `${m} minute${m === 1 ? '' : 's'}`}</option>
					{/each}
				</select>
				without activity
			</label>
			<button onclick={async () => (await lockNow(), goto('/unlock'))}>Lock now</button>
		</section>

		<section>
			<h2>Change master password</h2>
			<form onsubmit={savePassword}>
				<label>
					Current password
					<input type="password" autocomplete="current-password" bind:value={current} />
				</label>
				<NewPassword bind:password={newPassword} bind:ok={newPasswordOk} />
				<label class="check">
					<input type="checkbox" bind:checked={compromised} />
					I think someone else knows my current password.
				</label>
				{#if compromised}
					<p class="note">
						Your data will also be re-encrypted with a new key, so copies of the old key file in
						backups or synced folders stop working with it, and you'll get a new recovery key.
					</p>
				{/if}
				<button type="submit" disabled={busy || current === '' || !newPasswordOk}>
					Change password
				</button>
			</form>
		</section>
	{/if}

	{#if message}<p role="status">{message}</p>{/if}
	{#if error}<p class="error" role="alert">{error}</p>{/if}
</main>

<style>
	main {
		font-family: system-ui, sans-serif;
		max-width: 38rem;
		margin: 2rem auto;
		padding: 0 1rem;
	}
	section {
		margin: 1.5rem 0;
	}
	form {
		display: grid;
		gap: 0.75rem;
	}
	label {
		display: grid;
		gap: 0.25rem;
	}
	.check {
		display: flex;
		gap: 0.5rem;
		align-items: baseline;
	}
	.note {
		font-size: 0.9rem;
		color: #555;
	}
	.error {
		color: #b00020;
	}
</style>
