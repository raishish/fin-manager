<script lang="ts">
	// Create, unlock and recover the ledger (SPEC §7). Every rule lives in the core;
	// this page only shows the screen that fits the session's status.
	import { goto } from '$app/navigation';
	import NewPassword from './NewPassword.svelte';
	import RecoveryKey from './RecoveryKey.svelte';
	import {
		createLedger,
		errorMessage,
		finishCreate,
		recoveryToConfirm,
		setPasswordAfterRecovery,
		unlockState,
		unlockWithPassword,
		unlockWithRecoveryKey,
		unlockWithTouchId,
		type RecoveryToConfirm,
		type UnlockState
	} from './api';

	let info = $state<UnlockState | null>(null);
	let recovery = $state<RecoveryToConfirm | null>(null);
	let mode = $state<'password' | 'recovery'>('password');
	let password = $state('');
	let newPassword = $state('');
	let newPasswordOk = $state(false);
	let recoveryText = $state('');
	let compromised = $state(false);
	let error = $state('');
	let busy = $state(false);

	async function refresh() {
		info = await unlockState();
		if (info.status === 'ConfirmRecoveryKey') recovery = await recoveryToConfirm();
		if (info.status === 'Unlocked') await goto('/');
	}

	async function run(action: () => Promise<RecoveryToConfirm | null | void>) {
		busy = true;
		error = '';
		try {
			const shown = await action();
			if (shown) recovery = shown;
			password = '';
			recoveryText = '';
			await refresh();
		} catch (e) {
			error = errorMessage(e);
		} finally {
			busy = false;
		}
	}

	function submit(event: SubmitEvent, action: () => Promise<RecoveryToConfirm | null | void>) {
		event.preventDefault();
		run(action);
	}

	$effect(() => {
		refresh().catch((e) => (error = errorMessage(e)));
	});
</script>

<main>
	<h1>fin-manager</h1>

	{#if !info}
		<p>Loading…</p>
	{:else if info.status === 'NoLedger'}
		<h2>Choose a master password</h2>
		<p>
			Your data is encrypted with this password and never leaves this computer. It is never
			stored, so it can't be reset by anyone; you'll also get a recovery key next.
		</p>
		<form onsubmit={(e) => submit(e, () => createLedger(newPassword))}>
			<NewPassword bind:password={newPassword} bind:ok={newPasswordOk} label="Master password" />
			<button type="submit" disabled={busy || !newPasswordOk}>Create</button>
		</form>
	{:else if info.status === 'Incomplete'}
		<h2>Finish setting up</h2>
		<p>Setup was interrupted. Enter the master password you chose to finish it.</p>
		<form onsubmit={(e) => submit(e, () => finishCreate(password))}>
			<input type="password" autocomplete="current-password" bind:value={password} />
			<button type="submit" disabled={busy || password === ''}>Continue</button>
		</form>
	{:else if info.status === 'Unrecoverable'}
		<h2>This data can't be opened</h2>
		<p>
			The key file for this data is missing, so it can never be decrypted. Restore it from a
			backup, or start again with new data.
		</p>
	{:else if info.status === 'Locked'}
		{#if mode === 'password'}
			<h2>Unlock</h2>
			<form onsubmit={(e) => submit(e, () => unlockWithPassword(password))}>
				<label>
					Master password
					<!-- svelte-ignore a11y_autofocus -->
					<input type="password" autocomplete="current-password" bind:value={password} autofocus />
				</label>
				<button type="submit" disabled={busy || password === ''}>Unlock</button>
			</form>
			{#if info.touchIdBlocked === null}
				<button class="secondary" disabled={busy} onclick={() => run(unlockWithTouchId)}>
					Use Touch ID
				</button>
			{/if}
			<button class="link" onclick={() => ((mode = 'recovery'), (error = ''))}>
				Forgot your password? Use your recovery key
			</button>
		{:else}
			<h2>Unlock with your recovery key</h2>
			<p>Enter the 56-character key you saved. Spaces and dashes don't matter.</p>
			<form onsubmit={(e) => submit(e, () => unlockWithRecoveryKey(recoveryText))}>
				<textarea
					rows="3"
					bind:value={recoveryText}
					autocomplete="off"
					autocapitalize="characters"
					spellcheck="false"
				></textarea>
				<button type="submit" disabled={busy || recoveryText.trim() === ''}>Unlock</button>
			</form>
			<button class="link" onclick={() => ((mode = 'password'), (error = ''))}>
				Use your password instead
			</button>
		{/if}
	{:else if info.status === 'NeedsNewPassword'}
		<h2>Set a new master password</h2>
		<p>You opened your data with the recovery key. Choose a new password to continue.</p>
		<form
			onsubmit={(e) => submit(e, () => setPasswordAfterRecovery(newPassword, compromised))}
		>
			<NewPassword bind:password={newPassword} bind:ok={newPasswordOk} />
			<label class="check">
				<input type="checkbox" bind:checked={compromised} />
				Someone else may have known my old password.
			</label>
			{#if compromised}
				<p class="note">
					Your data will be re-encrypted with a new key, so old copies of the key file (in
					backups or synced folders) stop working with it. This takes a moment.
				</p>
			{/if}
			<button type="submit" disabled={busy || !newPasswordOk}>Save password</button>
		</form>
		<p class="note">You'll get a new recovery key next; the one you just used stops working.</p>
	{:else if info.status === 'ConfirmRecoveryKey' && recovery}
		<RecoveryKey {recovery} ondone={() => refresh()} />
	{/if}

	{#if error}<p class="error" role="alert">{error}</p>{/if}
</main>

<style>
	main {
		font-family: system-ui, sans-serif;
		max-width: 34rem;
		margin: 3rem auto;
		padding: 0 1rem;
	}
	form {
		display: grid;
		gap: 0.75rem;
		margin: 1rem 0;
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
	textarea {
		font-family: ui-monospace, monospace;
		letter-spacing: 0.05em;
	}
	button.link {
		background: none;
		border: none;
		padding: 0;
		color: #0b57d0;
		cursor: pointer;
		display: block;
		margin-top: 0.75rem;
	}
	.note {
		font-size: 0.9rem;
		color: #555;
	}
	.error {
		color: #b00020;
	}
</style>
