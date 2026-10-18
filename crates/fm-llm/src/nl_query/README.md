# nl_query: question → structured query (T4.2)

The local model reads a question and writes a structured query (T4.1's language). The
app shows the user how the query was read, then runs it.

```text
question + today + the user's categories and accounts
  → prompt (language, worked examples)
  → model, decoding against the query schema's grammar
  → JSON query → fm_core::query::Query → describe() for the user, run() for the answers
```

```rust
let vocabulary = Vocabulary { categories, groups, accounts, currencies }; // from the ledger
let mut request = nl_query::Request::new(question, "2026-10-01", fm_core::query::SCHEMA, &vocabulary);
request.answer_lead = lead; // e.g. an empty <think></think> block for a thinking model
let reading = nl_query::interpret(&model, &request, &GenerateOptions { sampling: Sampling::Greedy, max_tokens: 768, threads: None })?;
let query: fm_core::query::Query = serde_json::from_str(&reading.json)?;
let shown = fm_core::query::describe(&ledger, &query, as_of); // one line per item
let answers = fm_core::query::run(&ledger, &query, &Context { as_of, labeler })?;
```

Public surface (for T4.5, the Ask screen):
- `interpret`: one question → `Interpretation { json, confidence_bp, tokens, model_id, prompt_version }`.
- `system_prompt`: the fixed part of the prompt, to keep in a prompt cache (T2.4's `Session`).
- `grammar(schema, vocabulary).render()`: the GBNF to decode against.
- `PREFIX`, and `Request::answer_lead` for a thinking model.
- `complete`: answer completion, with `Completion::added` naming each added item's rule and reason.
- `fm_core::query::describe`: the interpretation in plain English, one line per item.

## Constrained output

- **The grammar.** The model decodes against a grammar built from T4.1's `schema.json`, so what comes out is always a query of the right shape.
  - llama.cpp's JSON-schema converter lives in its `common` library, which fm-llm leaves out. So `gbnf.rs` converts the keyword subset the query schema uses, and refuses any other keyword: a schema change that needs more fails loudly.
  - It writes compact JSON, keys in a fixed order (`name`, `op`, `period` first, so the model settles what an item is before its details).
- **Narrowing** (`narrow.rs`). Before conversion, the schema is narrowed to the user's ledger:
  - Category and group keys, account ids and currencies become enums of what exists, so a model can't write a key that would fail at run time.
  - A query has at most 6 items (`MAX_ITEMS`).
  - Every scope must name its period, rather than silently defaulting to all time.
  - Optional fields can't be written as `null`.
- **A recogniser for tests.** The grammar is a small tree, rendered to GBNF for llama.cpp and also runnable directly (`Grammar::accepts`). Tests use it to prove the grammar's reach without a model:
  - all 98 TD.3 expected queries and every worked example are accepted;
  - unknown keys, a seventh item, a missing period and whitespace are not.

## Prompt (`prompt.rs`, `PROMPT_VERSION`)

- **The language:** ops, measures, scope and periods.
- **Rules:**
  - Transfers aren't spending.
  - Give everything a full answer shows: both sides plus the difference and percent change for comparisons, labels with amounts for rankings, names and a total for recurring costs.
  - Use relative periods for relative phrases.
- **Context:** today's date and weekday, from the app; the user's categories (by group, key and name) and accounts.
- **Worked examples** (`examples.rs`).

**Worked examples.** About 30 hand-written pairs, covering every op and the measures that are easy to get wrong.
- **None is taken from TD.3:** a test fails if any example question, query or item equals one of TD.3's.
- **Every example** must be accepted by the grammar and deserialise as a `Query`.

**Today comes from the app.** `as_of` goes into the prompt so "in 2025" and "last month" can be read against it. The executor resolves relative periods with the same `as_of`.

**Deterministic.** Greedy decoding: the same model, prompt version, question and ledger always give the same query.

**Fast decoding (T2.7).** The runtime samples each token first and checks only that token against the grammar, applying the grammar to the whole vocabulary only when the token is rejected (as llama.cpp's own sampler does). The query written is the same as checking every token. `confidence_bp` is the model's probability of the query over its whole vocabulary, which is at most the figure renormalised over what the grammar allows. `Model::generate_exact` gives that old figure, at the old speed.

**Where the time goes.** Measured with Qwen3.5-4B Q4_K_M on Metal, over 20 TD.3 questions (every fifth id), alternating old and new paths in a quiet window:

| | Prompt (4,610 tokens) | Generation | Total |
|---|---|---|---|
| Grammar over the whole vocabulary at every step (`generate_exact`) | 22.5 s | 4.23 s (82 ms a token) | 26.7 s |
| Sampled token checked first (`generate`, T2.7) | 22.7 s | 3.77 s (73 ms a token) | 26.5 s |

- The queries are byte-identical on all 20, and only 12 of 1,035 sampled tokens were rejected.
- The prompt (the language, the worked examples and the user's vocabulary) is about 85% of each question's time. The way to make a question fast is to evaluate that prompt once and keep it: a `Session` with the system prompt as its cached prefix (T4.5), after which a question costs only its own few tokens plus generation.
- On the CI test model on a CPU, the per-token grammar check was the cost. There, T2.7 is what makes the model tests fit in CI.

**fm-core isn't a dependency.** T2.5 makes fm-core call fm-llm, so this module takes the schema text and a `Vocabulary` as plain values and returns JSON. fm-core is only a dev-dependency, so tests can deserialise into `Query`.

## Answer completion (`complete`, optional)

A small model often writes the main value and stops, for example the month with the most spending but not the amount. `complete` applies fixed rules that add the companion values a full answer shows:
- Each rule only adds items, built from the model's own items (same measure, same scope).
- The added items are part of the query the user sees.
- The rules are listed in `COMPLETION_RULES` and printed in every eval report.

If the completed query fails to run, the app answers with the model's own query instead. An added item can fail where the model's own would not: for example, a price change for a series whose price never changed.

Whether completion counts toward T4.2's accuracy is the user's decision (ask 21): only if it also holds on the held-out set TD.7, which was written without seeing these rules.

## Accuracy

Measured on TD.3 with the local model by `testdata/questions`'s `nl-eval` (see its README). The report for each model is in `testdata/questions/nl-eval/`.

The model tests here (the small CI model, `FM_LLM_TEST_MODEL`, SmolLM2) are ignored in CI until T2.7 speeds up grammar checking. They check plumbing only, and the grammar tests without a model cover validity:
- llama.cpp takes the grammar;
- whatever the model writes, greedy or sampled, deserialises as a query;
- the same question gives the same query twice.
