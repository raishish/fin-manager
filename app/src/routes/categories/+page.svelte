<script lang="ts">
	// Categories and rules (T3.3). Renaming or merging applies to past transactions at once:
	// they refer to the category, not its name. Hiding only removes a category from pickers;
	// its money still counts.
	import {
		deleteRule,
		editTaxonomy,
		errorText,
		money,
		rules,
		setRuleEnabled,
		taxonomy,
		type Category,
		type Edit,
		type Rule,
		type Rules,
		type Taxonomy
	} from '../dashboard/ledger';

	let list = $state<Taxonomy | null>(null);
	let ruleList = $state<Rules | null>(null);
	let error = $state('');
	let message = $state('');
	let renaming = $state<{ kind: 'category' | 'group'; id: number; name: string } | null>(null);
	let merging = $state<{ from: Category; into: number | null } | null>(null);
	let undo = $state<Record<number, boolean>>({});

	async function load() {
		try {
			[list, ruleList] = await Promise.all([taxonomy(), rules()]);
		} catch (e) {
			error = errorText(e);
		}
	}

	$effect(() => {
		load();
	});

	async function edit(e: Edit, done: string) {
		error = '';
		message = '';
		try {
			list = await editTaxonomy(e);
			ruleList = await rules();
			message = done;
			renaming = null;
			merging = null;
		} catch (err) {
			error = errorText(err);
		}
	}

	async function ruleAction(action: () => Promise<Rules>) {
		error = '';
		message = '';
		try {
			ruleList = await action();
		} catch (err) {
			error = errorText(err);
		}
	}

	const live = $derived(
		list?.groups.flatMap((g) =>
			// Merge targets: not merged away, not hidden, not in a hidden group (the core
			// refuses those too).
			g.hidden
				? []
				: g.categories
						.filter((c) => c.mergedInto === null && !c.hidden)
						.map((c) => ({ ...c, group: g.name }))
		) ?? []
	);

	function amount(r: Rule): string {
		if (!r.amount) return '';
		return r.amount.kind === 'exact'
			? `amount ${money(r.amount.amount)}`
			: `amount ${money(r.amount.min)} to ${money(r.amount.max)}`;
	}
</script>

<main>
	<nav>
		<a href="/">Home</a> · <a href="/dashboard">Dashboard</a> · <a href="/review">Review</a> ·
		<a href="/import">Import</a>
	</nav>
	<h1>Categories</h1>
	{#if error}<p role="alert">{error}</p>{/if}
	{#if message}<p role="status" class="done">{message}</p>{/if}

	{#if list}
		<p class="muted">List version {list.version}.</p>
		{#each list.groups as g (g.id)}
			<section class:hidden={g.hidden}>
				<h2>
					{#if renaming?.kind === 'group' && renaming.id === g.id}
						<form
							class="inline"
							onsubmit={(e) => {
								e.preventDefault();
								edit({ kind: 'rename_group', id: g.id, name: renaming!.name }, 'Group renamed.');
							}}
						>
							<input bind:value={renaming.name} aria-label="Group name" />
							<button type="submit">Save</button>
							<button type="button" onclick={() => (renaming = null)}>Cancel</button>
						</form>
					{:else}
						{g.name}
						<button type="button" class="small" onclick={() => (renaming = { kind: 'group', id: g.id, name: g.name })}>
							Rename
						</button>
						<button
							type="button"
							class="small"
							onclick={() =>
								edit(
									{ kind: 'set_group_hidden', id: g.id, hidden: !g.hidden },
									g.hidden ? 'Group shown again.' : 'Group hidden from pickers. Its money still counts.'
								)}
						>
							{g.hidden ? 'Show' : 'Hide'}
						</button>
					{/if}
				</h2>
				<table>
					<tbody>
						{#each g.categories as c (c.id)}
							<tr class:hidden={c.hidden} class:merged={c.mergedInto !== null}>
								<td>
									{#if renaming?.kind === 'category' && renaming.id === c.id}
										<form
											class="inline"
											onsubmit={(e) => {
												e.preventDefault();
												edit(
													{ kind: 'rename_category', id: c.id, name: renaming!.name },
													'Renamed. Past transactions show the new name.'
												);
											}}
										>
											<input bind:value={renaming.name} aria-label="Category name" />
											<button type="submit">Save</button>
											<button type="button" onclick={() => (renaming = null)}>Cancel</button>
										</form>
									{:else}
										{c.name}
										{#if c.hidden}<span class="muted">(hidden)</span>{/if}
										{#if c.mergedInto !== null}<span class="muted">→ merged into {c.mergedIntoName}</span>{/if}
									{/if}
								</td>
								<td class="muted num">
									{#if c.mergedInto === null}{c.transactions} transactions{/if}
								</td>
								<td class="nowrap">
									{#if c.mergedInto === null}
										<button type="button" class="small" onclick={() => (renaming = { kind: 'category', id: c.id, name: c.name })}>
											Rename
										</button>
										<button
											type="button"
											class="small"
											onclick={() =>
												edit(
													{ kind: 'set_category_hidden', id: c.id, hidden: !c.hidden },
													c.hidden ? 'Shown again.' : 'Hidden from pickers. Its money still counts.'
												)}
										>
											{c.hidden ? 'Show' : 'Hide'}
										</button>
										<button type="button" class="small" onclick={() => (merging = { from: c, into: null })}>
											Merge…
										</button>
									{/if}
								</td>
							</tr>
							{#if merging?.from.id === c.id}
								<tr class="editor">
									<td colspan="3">
										<form
											class="inline"
											onsubmit={(e) => {
												e.preventDefault();
												if (merging?.into != null)
													edit(
														{ kind: 'merge', from: c.id, into: merging.into },
														'Merged. Past transactions now count in the category you chose.'
													);
											}}
										>
											Merge <strong>{c.name}</strong> into
											<select bind:value={merging.into}>
												<option value={null}>choose…</option>
												{#each live.filter((o) => o.id !== c.id) as o (o.id)}
													<option value={o.id}>{o.group} · {o.name}</option>
												{/each}
											</select>
											<button type="submit" disabled={merging.into === null}>
												Move {c.transactions} {c.transactions === 1 ? 'transaction' : 'transactions'}
											</button>
											<button type="button" onclick={() => (merging = null)}>Cancel</button>
										</form>
										<p class="muted">Merging can't be undone yet.</p>
									</td>
								</tr>
							{/if}
						{/each}
					</tbody>
				</table>
			</section>
		{/each}
	{/if}

	{#if ruleList}
		<section>
			<h2>Rules</h2>
			{#if ruleList.ruleDidNotFit > 0}
				<p class="warn">
					A rule didn't fit {ruleList.ruleDidNotFit}
					{ruleList.ruleDidNotFit === 1 ? 'transaction' : 'transactions'} (for example an income rule
					on money going out). They were left as they are; check them in
					<a href="/review">Review</a>.
				</p>
			{/if}
			{#if ruleList.rules.length === 0}
				<p class="muted">No rules yet. Filing a transaction in Review learns one.</p>
			{:else}
				<table>
					<thead>
						<tr><th>When the description</th><th>Then</th><th>From</th><th class="num">Decisions</th><th></th></tr>
					</thead>
					<tbody>
						{#each ruleList.rules as r (r.id)}
							<tr class:hidden={!r.enabled}>
								<td>
									{r.matchKind === 'contains' ? 'contains' : r.matchKind === 'startsWith' ? 'starts with' : 'is'}
									<code>{r.pattern}</code>
									{#if r.account}<br /><span class="muted">on {r.account}</span>{/if}
									{#if r.amount}<br /><span class="muted">{amount(r)}</span>{/if}
								</td>
								<td>
									{#if r.bucket}{r.bucket}{/if}
									{#if r.merchant}<br /><span class="muted">merchant “{r.merchant}”</span>{/if}
								</td>
								<td class="muted">{r.origin === 'you' ? 'you wrote it' : 'learned from a correction'}</td>
								<td class="num">{r.decisions}</td>
								<td class="nowrap">
									<label class="check">
										<input type="checkbox" bind:checked={undo[r.id]} />
										also undo its {r.decisions} decisions
									</label>
									<button
										type="button"
										class="small"
										onclick={() => ruleAction(() => setRuleEnabled(r.id, !r.enabled, !!undo[r.id]))}
									>
										{r.enabled ? 'Disable' : 'Enable'}
									</button>
									<button type="button" class="small" onclick={() => ruleAction(() => deleteRule(r.id, !!undo[r.id]))}>
										Delete
									</button>
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
				<p class="muted">
					A disabled or deleted rule stops matching. What it already decided stays unless you also
					undo its decisions.
				</p>
			{/if}
		</section>
	{/if}
</main>

<style>
	main {
		font-family: system-ui, sans-serif;
		margin: 2rem;
		max-width: 60rem;
	}
	section {
		margin: 1.25rem 0;
	}
	h2 {
		font-size: 1.1rem;
		display: flex;
		gap: 0.5rem;
		align-items: center;
	}
	table {
		border-collapse: collapse;
		width: 100%;
		font-size: 0.92em;
	}
	td,
	th {
		text-align: left;
		padding: 0.3rem 0.5rem;
		border-bottom: 1px solid color-mix(in srgb, currentColor 12%, transparent);
		vertical-align: top;
	}
	.num {
		text-align: right;
	}
	.nowrap {
		white-space: nowrap;
	}
	.muted {
		color: GrayText;
	}
	.hidden {
		opacity: 0.6;
	}
	tr.merged {
		opacity: 0.5;
	}
	.small {
		font-size: 0.8rem;
	}
	form.inline {
		display: inline-flex;
		gap: 0.4rem;
		align-items: center;
		flex-wrap: wrap;
	}
	.check {
		display: inline-flex;
		gap: 0.3rem;
		font-size: 0.8rem;
	}
	tr.editor td {
		background: color-mix(in srgb, currentColor 4%, transparent);
	}
	.done {
		border-left: 3px solid #2e7d32;
		padding-left: 0.75rem;
	}
	.warn {
		border-left: 3px solid #b25e00;
		padding-left: 0.75rem;
	}
</style>
