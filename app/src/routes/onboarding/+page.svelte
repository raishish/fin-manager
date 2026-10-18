<script lang="ts">
	// First run (T5.5, SPEC decision 34):
	// 1. Welcome (before a ledger exists): the promise, and the model download starts.
	// 2. Password and recovery key: T5.1's create flow at /unlock.
	// 3. Bring in data. 4. First insight from fixed logic. 5. Review. 6. Backup folder,
	// suggested, not blocking. 7. A first question.
	// The step is kept in the ledger, so quitting part-way resumes where it left off.
	import { goto } from '$app/navigation';
	import { open } from '@tauri-apps/plugin-dialog';
	import { setBackupFolder } from '../backup/api';
	import { errorText, reviewQueue } from '../dashboard/ledger';
	import { unlockState } from '../unlock/api';
	import FirstInsight from './FirstInsight.svelte';
	import ImportQueue from './ImportQueue.svelte';
	import ModelProgress from './ModelProgress.svelte';
	import { onboardingState, setOnboardingStep, startModelDownload, type Step } from './api';

	let welcome = $state(false);
	let step = $state<Step | null>(null);
	let imported = $state(0);
	let waiting = $state<number | null>(null);
	let folder = $state('');
	let error = $state('');

	$effect(() => {
		unlockState()
			.then(async ({ status }) => {
				if (status === 'NoLedger') {
					welcome = true;
					return;
				}
				step = (await onboardingState()).step;
			})
			.catch((e) => (error = errorText(e)));
	});

	$effect(() => {
		if (step === 'review') {
			reviewQueue(0, 0)
				.then((q) => (waiting = q.total))
				.catch(() => (waiting = null));
		}
	});

	async function begin() {
		// The download needs no ledger; it runs in the background from here on.
		startModelDownload().catch(() => {});
		await goto('/unlock');
	}

	async function go(next: Step) {
		error = '';
		try {
			step = (await setOnboardingStep(next)).step;
			if (step === 'done') await goto('/dashboard');
		} catch (e) {
			error = errorText(e);
		}
	}

	/** The native folder picker; the typed path below stays as a fallback. */
	async function pickFolder() {
		error = '';
		try {
			const picked = await open({ directory: true, multiple: false, title: 'Backup folder' });
			if (typeof picked === 'string') folder = picked;
		} catch (e) {
			error = errorText(e);
		}
	}

	async function chooseFolder() {
		error = '';
		try {
			await setBackupFolder(folder.trim());
			await go('ask');
		} catch (e) {
			error = errorText(e);
		}
	}

	const order: Step[] = ['data', 'insight', 'review', 'backup', 'ask'];
</script>

<main>
	{#if welcome}
		<h1>Welcome to fin-manager</h1>
		<p class="lead">Your data never leaves this computer.</p>
		<p>
			Your bank files are read here, stored encrypted here, and the AI that sorts your spending
			runs here. The app connects out only to download that model, and to a bank connection if
			you set one up; every connection is listed in the <a href="/network">network log</a>.
		</p>
		<p class="muted">
			Next you'll choose a master password. The model starts downloading now, in the background.
		</p>
		<button type="button" class="primary" onclick={begin}>Get started</button>
	{:else if step}
		<ol class="steps" aria-label="Setup steps">
			{#each order as s, i (s)}
				<li class:current={s === step} class:past={order.indexOf(step) > i}>
					{['Bring in data', 'First look', 'Review', 'Backups', 'Ask'][i]}
				</li>
			{/each}
		</ol>
		<ModelProgress />
		{#if error}<p role="alert">{error}</p>{/if}

		{#if step === 'data'}
			<h1>Bring in your data</h1>
			<p>
				Download a CSV, OFX or QFX export from each bank's website. Known banks are recognised
				automatically.
			</p>
			<ImportQueue onimported={() => imported++} />
			<p class="muted">
				Or connect a bank through <a href="/connectors">SimpleFIN</a>: SimpleFIN and its data
				provider (MX) see your transactions when they fetch them for you.
			</p>
			<button type="button" class="primary" disabled={imported === 0} onclick={() => go('insight')}>
				Continue
			</button>
		{:else if step === 'insight'}
			<h1>A first look</h1>
			<p class="muted">From your files alone, before anything is categorised.</p>
			<FirstInsight />
			<button type="button" class="primary" onclick={() => go('review')}>Continue</button>
		{:else if step === 'review'}
			<h1>Review</h1>
			<p>
				{#if waiting === null}…{:else}{waiting} transactions wait for a category.{/if}
				Each one you file teaches a rule, so the same merchant's other transactions follow. When the
				model is ready it fills in most of them; you approve or correct its choices.
			</p>
			<p><a href="/review">Open the review queue</a> <span class="muted">(you can come back here)</span></p>
			<button type="button" class="primary" onclick={() => go('backup')}>Continue</button>
		{:else if step === 'backup'}
			<h1>Backups</h1>
			<p>
				Your ledger exists only on this Mac. Choose a folder (an external drive, or a synced folder:
				backups are encrypted) and the app backs up daily and after each import.
			</p>
			<form
				onsubmit={(e) => {
					e.preventDefault();
					chooseFolder();
				}}
			>
				<label>
					Backup folder
					<input bind:value={folder} placeholder="/Volumes/Backup/fin-manager" />
				</label>
				<button type="button" onclick={pickFolder}>Choose…</button>
				<button type="submit" disabled={folder.trim() === ''}>Use this folder</button>
			</form>
			<button type="button" onclick={() => go('ask')}>Skip for now</button>
			<p class="muted">Until a folder is set, the app reminds you (you can snooze it a day at a time).</p>
		{:else if step === 'ask'}
			<h1>Ask a question</h1>
			<p>Soon you'll be able to ask things like "Where did my money go last month?" right here.</p>
			<button type="button" disabled>Where did my money go last month?</button>
			<p class="muted">Questions arrive in a later version.</p>
			<button type="button" class="primary" onclick={() => go('done')}>Go to the dashboard</button>
		{/if}
	{:else if error}
		<p role="alert">{error}</p>
	{/if}
</main>

<style>
	main {
		font-family: system-ui, sans-serif;
		margin: 2rem auto;
		max-width: 46rem;
		padding: 0 1rem;
		display: grid;
		gap: 0.75rem;
	}
	.lead {
		font-size: 1.25em;
		font-weight: 600;
	}
	.muted {
		color: GrayText;
	}
	.steps {
		display: flex;
		gap: 1rem;
		list-style: none;
		padding: 0;
		font-size: 0.85em;
		color: GrayText;
	}
	.steps .current {
		color: inherit;
		font-weight: 600;
	}
	.steps .past::after {
		content: ' ✓';
	}
	button.primary {
		justify-self: start;
		font-weight: 600;
	}
	form {
		display: flex;
		gap: 0.5rem;
		align-items: end;
	}
	label {
		display: grid;
		gap: 0.25rem;
		flex: 1;
	}
</style>
