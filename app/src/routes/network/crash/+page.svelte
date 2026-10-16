<script lang="ts">
	import { onMount } from 'svelte';
	import {
		crashReport,
		crashReports,
		crashSettings,
		deleteCrashReport,
		setCrashReportsEnabled,
		type CrashSettings,
		type SavedReport
	} from '../api';

	let settings = $state<CrashSettings | null>(null);
	let reports = $state<SavedReport[]>([]);
	let selected = $state<SavedReport | null>(null);
	// Exactly the file's contents: what the user would send.
	let preview = $state('');
	let status = $state<string | null>(null);
	let error = $state<string | null>(null);

	async function load() {
		try {
			settings = await crashSettings();
			reports = await crashReports();
			error = null;
		} catch (e) {
			error = String(e);
		}
	}

	async function toggle(event: Event) {
		const enabled = (event.currentTarget as HTMLInputElement).checked;
		try {
			settings = await setCrashReportsEnabled(enabled);
		} catch (e) {
			error = String(e);
		}
	}

	async function open(report: SavedReport) {
		status = null;
		try {
			preview = await crashReport(report.id);
			selected = report;
		} catch (e) {
			error = String(e);
		}
	}

	async function copy() {
		try {
			await navigator.clipboard.writeText(preview);
			status = 'Copied. Paste it into an email or a GitHub issue.';
		} catch {
			status = 'Copy failed. Select the text below and copy it yourself.';
		}
	}

	async function remove(report: SavedReport) {
		try {
			await deleteCrashReport(report.id);
			if (selected?.id === report.id) {
				selected = null;
				preview = '';
			}
			status = null;
			await load();
		} catch (e) {
			error = String(e);
		}
	}

	onMount(load);
</script>

<main>
	<nav><a href="/">← Home</a> · <a href="/network">Network log</a></nav>
	<h1>Crash reports</h1>
	<p class="lead">
		Off unless you turn it on. When the app crashes, it saves a report on this computer with
		transaction details, amounts, merchant names and file paths removed. The app never sends
		reports. You can read one here and, if you want to, copy it into an email or a GitHub issue.
	</p>

	{#if error}
		<p role="alert">{error}</p>
	{/if}

	{#if settings}
		<label class="toggle">
			<input type="checkbox" checked={settings.enabled} onchange={toggle} />
			Save a crash report when the app crashes
		</label>
		<p class="note">Saved in <code>{settings.dir}</code></p>
	{/if}

	<h2>Saved reports</h2>
	{#if reports.length === 0}
		<p class="empty">None.</p>
	{:else}
		<ul class="reports">
			{#each reports as report (report.id)}
				<li class:current={selected?.id === report.id}>
					<button type="button" onclick={() => open(report)}>{report.id}</button>
					<button type="button" class="danger" onclick={() => remove(report)}>Delete</button>
				</li>
			{/each}
		</ul>
	{/if}

	{#if selected}
		<section>
			<h2>Exactly what would be sent</h2>
			<p class="note">File: <code>{selected.path}</code></p>
			<div class="actions">
				<button type="button" onclick={copy}>Copy report</button>
				<button type="button" class="danger" onclick={() => selected && remove(selected)}>
					Delete
				</button>
			</div>
			{#if status}
				<p role="status">{status}</p>
			{/if}
			<textarea readonly rows="24" aria-label="Crash report contents" value={preview}></textarea>
		</section>
	{/if}
</main>

<style>
	main {
		font-family: system-ui, sans-serif;
		margin: 2rem;
		max-width: 60rem;
	}
	.lead,
	.note,
	.empty {
		color: GrayText;
	}
	.note {
		font-size: 0.85em;
	}
	.toggle {
		display: flex;
		gap: 0.5rem;
		align-items: center;
		font-weight: 600;
	}
	.reports {
		list-style: none;
		padding: 0;
	}
	.reports li {
		display: flex;
		gap: 0.5rem;
		margin-bottom: 0.4rem;
	}
	.reports li.current button:first-child {
		font-weight: 600;
	}
	.actions {
		display: flex;
		gap: 0.5rem;
		margin-bottom: 0.5rem;
	}
	.danger {
		color: #b42318;
	}
	textarea {
		width: 100%;
		font-family: ui-monospace, monospace;
		font-size: 0.85em;
		box-sizing: border-box;
	}
</style>
