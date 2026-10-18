<script lang="ts">
	// The backup reminder (T5.5, SPEC decision 34): shown until a backup folder is set and
	// a backup has succeeded, or when the last one failed or is over a week old. The rule
	// is in app/src-tauri/src/onboarding.rs (`reminder`).
	import { errorText } from '../dashboard/ledger';
	import { backupReminder, snoozeBackupReminder, type Reminder } from './api';

	let reminder = $state<Reminder | null>(null);
	let error = $state('');

	$effect(() => {
		backupReminder()
			.then((r) => (reminder = r))
			.catch(() => (reminder = null));
	});

	const messages: Record<string, string> = {
		noFolder: 'No backup folder yet: your data exists only on this Mac.',
		noBackupYet: 'A backup folder is set, but no backup has been made yet.',
		failed: 'The last backup failed.',
		stale: 'The last backup is more than a week old.'
	};

	async function snooze() {
		try {
			reminder = await snoozeBackupReminder();
		} catch (e) {
			error = errorText(e);
		}
	}
</script>

{#if reminder?.reason}
	<div class="reminder" role="status">
		<span>{messages[reminder.reason]}</span>
		<a href="/backup">Choose a backup folder</a>
		<button type="button" onclick={snooze}>Remind me tomorrow</button>
		{#if error}<span role="alert">{error}</span>{/if}
	</div>
{/if}

<style>
	.reminder {
		display: flex;
		gap: 0.75rem;
		align-items: center;
		flex-wrap: wrap;
		padding: 0.5rem 0.75rem;
		border-left: 3px solid #b25e00;
		background: color-mix(in srgb, #b25e00 8%, transparent);
		margin: 0.5rem 0 1rem;
	}
</style>
