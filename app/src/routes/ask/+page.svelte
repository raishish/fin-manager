<script lang="ts">
	// Ask (T4.4): a question about your money. The app shows how it read the question
	// before anything else, then the answers, each with the transactions behind it, and
	// an explanation whose every number has been checked against the answers. Anything
	// the app added that you didn't ask for is marked as added.
	import TransactionList from '../dashboard/TransactionList.svelte';
	import { errorCode, errorText, money } from '../dashboard/ledger';
	import { ask, type AnswerView, type AskView } from './ask';

	let question = $state('');
	let result = $state<AskView | null>(null);
	let error = $state('');
	let notReady = $state(false);
	let busy = $state(false);
	let open = $state<string | null>(null);

	async function run() {
		if (!question.trim()) return;
		busy = true;
		error = '';
		notReady = false;
		result = null;
		open = null;
		try {
			result = await ask(question);
		} catch (e) {
			notReady = errorCode(e) === 'NotReady';
			error = errorText(e);
		} finally {
			busy = false;
		}
	}

	function value(a: AnswerView): string {
		switch (a.kind) {
			case 'money':
				return a.money ? money(a.money) : '';
			case 'count':
				return String(a.count ?? '');
			case 'percent':
				return a.percent ?? '';
			case 'date':
				return a.date ?? '';
			default:
				return a.texts.join(', ');
		}
	}
</script>

<main>
	<nav>
		<a href="/">Home</a> · <a href="/dashboard">Dashboard</a> · <a href="/review">Review</a>
	</nav>
	<h1>Ask about your money</h1>

	<form
		onsubmit={(e) => {
			e.preventDefault();
			run();
		}}
	>
		<label>
			Your question
			<input
				bind:value={question}
				maxlength="500"
				placeholder="How much did I spend on groceries last month?"
				disabled={busy}
			/>
		</label>
		<button type="submit" disabled={busy || !question.trim()}>Ask</button>
	</form>

	{#if notReady}
		<p class="calm">{error}</p>
	{:else if error}
		<p class="error" role="alert">{error}</p>
	{/if}

	{#if result}
		<section>
			<h2>How I read your question</h2>
			<ul class="read">
				{#each result.read as item (item.name)}
					<li class:added={item.added}>
						<strong>{item.label}</strong>: {item.line}
						{#if item.added}
							<span class="badge" title={item.addedRule ?? ''}>{item.added}</span>
						{/if}
					</li>
				{/each}
			</ul>
			<p class="muted">Answered as of {result.asOf}.</p>
		</section>

		{#if result.problem}
			<p class="error" role="alert">{result.problem}</p>
		{/if}

		{#if result.answers.length > 0}
			<section>
				<h2>Answer</h2>
				{#each result.answers as a (a.name)}
					<article class:added={a.added}>
						<div class="row">
							<span>{a.label}</span>
							<strong class="value">{value(a)}</strong>
						</div>
						{#if a.added}
							<p class="badge">{a.added}</p>
						{/if}
						{#if a.transactionIds.length > 0}
							<button
								type="button"
								class="link"
								onclick={() => (open = open === a.name ? null : a.name)}
							>
								{open === a.name ? 'Hide' : 'Show'} the {a.transactionIds.length}
								{a.transactionIds.length === 1 ? 'transaction' : 'transactions'}
							</button>
							{#if open === a.name}
								<TransactionList ids={a.transactionIds} title={a.label} />
							{/if}
						{/if}
					</article>
				{/each}
			</section>
		{/if}

		{#if result.explanation}
			<section>
				<h2>In words</h2>
				<p>{result.explanation.text}</p>
				<p class="muted">
					{result.explanation.source === 'model'
						? 'Written by the local model; every number in it was checked against the answer above.'
						: 'Written by the app from the answer above.'}
				</p>
			</section>
		{/if}
	{/if}
</main>

<style>
	main {
		font-family: system-ui, sans-serif;
		margin: 2rem;
		max-width: 48rem;
	}
	form {
		display: flex;
		gap: 0.5rem;
		align-items: end;
	}
	label {
		flex: 1;
	}
	input {
		display: block;
		width: 100%;
		padding: 0.4rem;
		font-size: 1rem;
	}
	.read li {
		margin: 0.25rem 0;
	}
	article {
		border: 1px solid #ccc;
		border-radius: 0.5rem;
		padding: 0.5rem 0.75rem;
		margin: 0.5rem 0;
	}
	.added {
		border-left: 4px solid #b26b00;
		padding-left: 0.5rem;
	}
	.badge {
		display: inline-block;
		background: #fff3d6;
		color: #6b4100;
		border: 1px solid #e0b252;
		border-radius: 0.25rem;
		padding: 0 0.4rem;
		margin: 0.25rem 0;
		font-size: 0.85rem;
	}
	.row {
		display: flex;
		justify-content: space-between;
	}
	.value {
		font-variant-numeric: tabular-nums;
	}
	.link {
		background: none;
		border: none;
		color: LinkText;
		cursor: pointer;
		padding: 0;
	}
	.muted {
		color: GrayText;
	}
	.calm {
		color: GrayText;
		font-style: italic;
	}
	.error {
		color: #b00020;
	}
</style>
