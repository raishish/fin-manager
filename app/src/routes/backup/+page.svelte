<script lang="ts">
	// Backup settings (T5.2, SPEC §8): where backups go, the last one, and how to restore.
	import { errorMessage } from '../unlock/api';
	import {
		backupNow,
		backupStatus,
		formatBytes,
		formatTime,
		setBackupFolder,
		type BackupStatus
	} from './api';

	let info = $state<BackupStatus | null>(null);
	let folder = $state('');
	let message = $state('');
	let error = $state('');
	let busy = $state(false);

	async function refresh() {
		info = await backupStatus();
		if (folder === '' && info.folder) folder = info.folder;
	}

	async function run(action: () => Promise<unknown>, done: string) {
		busy = true;
		error = '';
		message = '';
		try {
			await action();
			message = done;
			await refresh();
		} catch (e) {
			error = errorMessage(e);
		} finally {
			busy = false;
		}
	}

	function saveFolder(event: SubmitEvent) {
		event.preventDefault();
		run(() => setBackupFolder(folder), 'Backup folder saved.');
	}

	$effect(() => {
		refresh().catch((e) => (error = errorMessage(e)));
	});
</script>

<main>
	<h1>Backups</h1>
	<p>
		Encrypted copies of your data go to a folder you choose: an external drive, or a folder
		synced by iCloud Drive, Dropbox or Google Drive. They can't be read without your password or
		recovery key, and they never pass through us. A backup is made once a day and after each
		import; the last 7 days, 4 weeks and 12 months are kept.
	</p>

	<section>
		<h2>Backup folder</h2>
		<form onsubmit={saveFolder}>
			<label>
				Full path of the folder
				<input
					bind:value={folder}
					placeholder="/Volumes/Backup Drive/fin-manager"
					autocomplete="off"
					spellcheck="false"
				/>
			</label>
			<button type="submit" disabled={busy || folder.trim() === ''}>Save folder</button>
		</form>
		<p class="note">The folder must already exist. Only files named fin-manager-….fmbackup are ever changed in it.</p>
	</section>

	{#if info}
		<section>
			<h2>Status</h2>
			{#if !info.folder}
				<p class="warning">No backup folder is set, so nothing is being backed up.</p>
			{:else}
				<p>
					Last backup:
					{info.lastAt ? formatTime(info.lastAt) : 'none yet'}
				</p>
				{#if info.lastError}<p class="error">Last attempt failed: {info.lastError}</p>{/if}
				{#if info.folderError}<p class="error">{info.folderError}</p>{/if}
				<button disabled={busy} onclick={() => run(backupNow, 'Backup made and checked.')}>
					Back up now
				</button>
			{/if}
		</section>

		{#if info.backups.length > 0}
			<section>
				<h2>Backups in the folder</h2>
				<ul class="list">
					{#each info.backups as b (b.fileName)}
						<li><span>{formatTime(b.createdAt)}</span><span>{formatBytes(b.bytes)}</span></li>
					{/each}
				</ul>
			</section>
		{/if}
	{/if}

	<section>
		<h2>Restoring</h2>
		<p>
			To restore, install the app on a computer with no data yet, choose
			<strong>Restore from a backup</strong> on the first screen, and give the path of a backup file.
		</p>
		<p class="note">
			A backup opens with the password (or recovery key) that was current <em>when it was made</em>.
			If you changed your password since, older backups still need the old one.
		</p>
	</section>

	{#if message}<p role="status">{message}</p>{/if}
	{#if error}<p class="error" role="alert">{error}</p>{/if}
</main>

<style>
	main {
		font-family: system-ui, sans-serif;
		max-width: 40rem;
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
	.list {
		list-style: none;
		padding: 0;
	}
	.list li {
		display: flex;
		justify-content: space-between;
		padding: 0.25rem 0;
		border-bottom: 1px solid #eee;
	}
	.note {
		font-size: 0.9rem;
		color: #555;
	}
	.warning {
		color: #8a4b00;
	}
	.error {
		color: #b00020;
	}
</style>
