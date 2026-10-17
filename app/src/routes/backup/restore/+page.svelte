<script lang="ts">
	// Restore a backup on a fresh install (T5.2). Reachable while locked; the app
	// refuses unless there is no data here yet, so nothing can be overwritten.
	import { goto } from '$app/navigation';
	import { errorMessage } from '../../unlock/api';
	import { restoreBackup } from '../api';

	let file = $state('');
	let error = $state('');
	let busy = $state(false);

	async function restore(event: SubmitEvent) {
		event.preventDefault();
		busy = true;
		error = '';
		try {
			await restoreBackup(file);
			await goto('/unlock');
		} catch (e) {
			error = errorMessage(e);
		} finally {
			busy = false;
		}
	}
</script>

<main>
	<h1>Restore from a backup</h1>
	<p>
		Give the full path of a backup file (named like
		<code>fin-manager-20261016T120000Z.fmbackup</code>). Choose the newest one you have.
	</p>
	<form onsubmit={restore}>
		<label>
			Backup file
			<input bind:value={file} autocomplete="off" spellcheck="false" />
		</label>
		<button type="submit" disabled={busy || file.trim() === ''}>Restore</button>
	</form>
	<p class="note">
		Afterwards, unlock with the password that was current <em>when the backup was made</em>, or
		with your recovery key. Restoring only works when this computer has no data in the app yet.
	</p>
	{#if error}<p class="error" role="alert">{error}</p>{/if}
	<p><a href="/unlock">Back</a></p>
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
	.note {
		font-size: 0.9rem;
		color: #555;
	}
	.error {
		color: #b00020;
	}
</style>
