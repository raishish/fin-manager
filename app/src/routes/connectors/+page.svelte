<script lang="ts">
	// Bank connection through SimpleFIN (T5.4, SPEC decision 12).
	import {
		connectorErrorMessage,
		formatTime,
		simplefinConnect,
		simplefinDisconnect,
		simplefinInspectToken,
		simplefinStatus,
		simplefinSync,
		type SimpleFinStatus,
		type SyncResult,
		type TokenInfo
	} from './api';

	let status = $state<SimpleFinStatus | null>(null);
	let result = $state<SyncResult | null>(null);
	let token = $state('');
	/** Where the pasted token points, shown for the user to confirm. */
	let pending = $state<TokenInfo | null>(null);
	let message = $state('');
	let error = $state('');
	let busy = $state(false);

	async function refresh() {
		status = await simplefinStatus();
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
			error = connectorErrorMessage(e);
			await refresh().catch(() => {});
		} finally {
			busy = false;
		}
	}

	async function check(event: SubmitEvent) {
		event.preventDefault();
		error = '';
		message = '';
		try {
			pending = await simplefinInspectToken(token);
		} catch (e) {
			error = connectorErrorMessage(e);
		}
	}

	function confirm() {
		if (!pending) return;
		const setupToken = token;
		const host = pending.host;
		token = '';
		pending = null;
		run(
			() => simplefinConnect(setupToken, host),
			'Connected. Update now to fetch your transactions.'
		);
	}

	function cancel() {
		token = '';
		pending = null;
	}

	function update() {
		result = null;
		run(async () => {
			result = await simplefinSync();
		}, 'Updated.');
	}

	function disconnect() {
		result = null;
		run(simplefinDisconnect, 'Disconnected. Your transactions stay in the app.');
	}

	$effect(() => {
		refresh().catch((e) => (error = connectorErrorMessage(e)));
	});
</script>

<main>
	<h1>Bank connection (SimpleFIN)</h1>

	<section class="who-sees">
		<h2>Who sees your data</h2>
		<p>
			<strong>SimpleFIN and MX see your bank data. We don't.</strong> SimpleFIN is a paid service you
			sign up for yourself; it uses MX to log in to your bank and collect your accounts and
			transactions. This app then fetches them from SimpleFIN directly to this computer. Nothing
			passes through us, and we have no copy.
		</p>
		<p class="note">
			If you'd rather nobody else saw your data, skip this and import the files your bank lets you
			download instead.
		</p>
	</section>

	{#if status && !status.connected}
		<section>
			<h2>Connect</h2>
			<ol>
				<li>Sign up at SimpleFIN Bridge and connect your banks there.</li>
				<li>In SimpleFIN, create a <em>setup token</em> for this app and copy it.</li>
				<li>Paste it below. It works once; this app swaps it for a private access key.</li>
			</ol>
			{#if !pending}
				<form onsubmit={check}>
					<label>
						Setup token
						<textarea
							bind:value={token}
							rows="3"
							autocomplete="off"
							spellcheck="false"
							placeholder="aHR0cHM6Ly9…"
						></textarea>
					</label>
					<button type="submit" disabled={busy || token.trim() === ''}>Continue</button>
				</form>
			{:else}
				<div class="confirm">
					<p>This token connects to <strong><code>{pending.host}</code></strong>.</p>
					{#if pending.simplefin}
						<p>That is SimpleFIN's own server.</p>
					{:else}
						<p class="warning">
							That is <strong>not</strong> a SimpleFIN server (simplefin.org). Only continue if you
							run this server yourself or trust whoever gave you the token: it will receive your
							requests and send you bank data.
						</p>
					{/if}
					<button disabled={busy} onclick={confirm}>Connect to {pending.host}</button>
					<button disabled={busy} onclick={cancel}>Cancel</button>
				</div>
			{/if}
			<p class="note">
				The access key is kept only inside your encrypted data, and the app contacts only your
				SimpleFIN server, which you can check in the network log.
			</p>
		</section>
	{/if}

	{#if status?.connected}
		<section>
			<h2>Connected</h2>
			<p>Server: <code>{status.bridge}</code></p>
			<p>
				Last update: {status.lastSync ? formatTime(status.lastSync) : 'not yet'}
				{#if status.syncedTo}· transactions fetched up to {formatTime(status.syncedTo)}{/if}
			</p>
			<p>
				Updates left today: {status.requestsLeft} of {status.requestsPerDay}
				{#if status.nextAllowed}(the next one is allowed after {formatTime(status.nextAllowed)}){/if}
			</p>
			<p class="note">
				SimpleFIN allows {status.requestsPerDay} requests a day and disables connections that use more,
				so the app stops before that. A first update fetches 90 days in two requests.
			</p>
			<button disabled={busy || status.requestsLeft === 0} onclick={update}>Update now</button>
		</section>

		{#if status.messages.length > 0}
			<section>
				<h2>Messages from SimpleFIN</h2>
				<ul>
					{#each status.messages as m, i (i)}
						<li class="warning">{m}</li>
					{/each}
				</ul>
			</section>
		{/if}

		{#if result}
			<section role="status">
				<h2>Last update</h2>
				<p>
					{result.inserted} new transactions{result.accountsCreated
						? `, ${result.accountsCreated} new accounts`
						: ''}; {result.postedFromPending} pending transactions posted; {result.duplicates} already
					here.
				</p>
				{#if !result.complete}
					<p class="warning">
						Today's requests ran out before everything was fetched. The next update carries on from
						where this one stopped.
					</p>
				{/if}
				{#if result.backupError}<p class="warning">Backup after the update failed: {result.backupError}</p>{/if}
			</section>
		{/if}

		<section>
			<h2>Disconnect</h2>
			<p>
				Forgets the access key and stops contacting SimpleFIN. Transactions already fetched stay
				(delete them under Data if you want). To stop SimpleFIN sharing your data, also disable
				this app's token in SimpleFIN.
			</p>
			<button disabled={busy} onclick={disconnect}>Disconnect</button>
		</section>
	{/if}

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
	.confirm {
		display: grid;
		gap: 0.5rem;
		justify-items: start;
	}
	.who-sees {
		border-left: 4px solid #8a4b00;
		padding-left: 1rem;
	}
	form {
		display: grid;
		gap: 0.75rem;
	}
	label {
		display: grid;
		gap: 0.25rem;
	}
	textarea {
		font-family: ui-monospace, monospace;
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
