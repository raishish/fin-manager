//! `nl-eval`: T4.2's accuracy on TD.3. Each question goes through the local model
//! (fm-llm's `nl_query`), the query it writes runs on the freelancer's labelled ledger
//! (fm-core's `query::run`), and the answers are compared with TD.3's.
//!
//! ```text
//! cargo run --release --features nl-eval --bin nl-eval -- \
//!   --model <file.gguf> --sha256 <hex> [--id <model id>] [--lead '<think>\n\n</think>\n\n'] \
//!   [--only q001,q002] [--questions <questions.json>] [--out nl-eval/<model id>.md] \\
//!   [--rows <results.jsonl>]
//! ```
//!
//! `--questions` runs another set in TD.3's format (the held-out TD.7); the default is
//! TD.3. The set's `persona` picks the TD.1 ledger. `--rows` keeps each question's
//! result as it finishes, and a rerun with the same file skips those questions: use a
//! fresh file whenever the model, the prompt or the rules change.
//!
//! ```text
//! ```
//!
//! **Correct** means:
//! - every expected answer is matched, one to one, by an answer of the model's query
//!   with the same value (money in the same currency, counts, percents, dates, texts);
//!   names and order don't matter, and extra answers are allowed but counted;
//! - no item of the model's query covers a period disjoint from every period the
//!   expected query covers (a stray item about the wrong time).
//!
//! The two questions with no expected query count as wrong unless the model somehow
//! matches them.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use fm_core::model::{Date, Ledger};
use fm_core::query::{self, Ask, Context, PeriodSpec, Query, Value};
use fm_llm::nl_query::{self, Account, Category, Group, Request, Vocabulary};
use fm_llm::{GenerateOptions, Model, ModelSpec, Sampling, sha256_hex};
use fm_questions::{Answer, Question, QuestionSet, build, labelled};

/// A value to compare, from either side.
#[derive(Debug, Clone, PartialEq, Eq)]
enum V {
    Money(i64, String),
    Count(i64),
    Percent(i64),
    Date(String),
    Text(String),
    Texts(Vec<String>),
}

fn expected(a: &Answer) -> (V, Option<Vec<i64>>) {
    let ids = |e: &fm_questions::Evidence| {
        (e.rule != fm_questions::Rule::Derived).then(|| {
            let mut v = e.transaction_ids.clone();
            v.sort_unstable();
            v
        })
    };
    match a {
        Answer::Money {
            amount, evidence, ..
        } => (
            V::Money(amount.minor(), amount.currency().code().to_owned()),
            ids(evidence),
        ),
        Answer::Count {
            value, evidence, ..
        } => (V::Count(*value), ids(evidence)),
        Answer::Percent { basis_points, .. } => (V::Percent(*basis_points), None),
        Answer::Date { value, .. } => (V::Date(value.to_string()), None),
        Answer::Text { value, .. } => (V::Text(value.clone()), None),
        Answer::Texts { values, .. } => (V::Texts(values.clone()), None),
    }
}

fn got(v: &Value) -> (V, Vec<i64>) {
    let ids = |t: &[fm_core::model::TransactionId]| t.iter().map(|i| i.0).collect();
    match v {
        Value::Money {
            amount,
            transaction_ids,
        } => (
            V::Money(amount.minor(), amount.currency().code().to_owned()),
            ids(transaction_ids),
        ),
        Value::Count {
            value,
            transaction_ids,
        } => (V::Count(*value), ids(transaction_ids)),
        Value::Percent { basis_points } => (V::Percent(*basis_points), vec![]),
        Value::Date { value } => (V::Date(value.to_string()), vec![]),
        Value::Text { value } => (V::Text(value.clone()), vec![]),
        Value::Texts { values } => (V::Texts(values.clone()), vec![]),
    }
}

/// Every period a query's items cover, resolved.
fn periods(ledger: &Ledger, q: &Query, as_of: Date) -> Vec<Option<(Date, Date)>> {
    q.items
        .iter()
        .filter_map(|i| match &i.ask {
            Ask::Sum { scope, .. }
            | Ask::Count { scope, .. }
            | Ask::Average { scope, .. }
            | Ask::Median { scope, .. }
            | Ask::Rank { scope, .. }
            | Ask::Keys { scope, .. }
            | Ask::Distinct { scope, .. }
            | Ask::Pick { scope, .. } => Some(&scope.period),
            Ask::Recurring { select, .. } => select.period.as_ref(),
            Ask::Derive { .. } => None,
        })
        .map(|p: &PeriodSpec| query::resolve_period(ledger, p, as_of).map(|p| (p.from, p.to)))
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum Outcome {
    Correct,
    /// The model failed (token cap, runtime).
    NoAnswer,
    /// The JSON didn't deserialise as a query (shouldn't happen under the grammar).
    NoParse,
    /// The query didn't run (an unknown merchant series, a zero base).
    RunError,
    WrongValue,
    /// Right values, plus an item about a period the question isn't about.
    StrayPeriod,
}

/// One question's result. Kept as a JSON line in `--rows`, so an interrupted run can
/// carry on where it stopped.
#[derive(serde::Serialize, serde::Deserialize)]
struct Row {
    id: String,
    kind: String,
    proxy: bool,
    outcome: Outcome,
    /// The same query after `nl_query::complete`.
    completed: Outcome,
    extra_items: bool,
    completed_extra: bool,
    ids_agree: Option<bool>,
    confidence_bp: u16,
    millis: u64,
    query: String,
    reading: Vec<String>,
    note: String,
}

fn vocabulary(ledger: &Ledger) -> Vocabulary {
    let t = &ledger.taxonomy;
    let group_key = |id| {
        t.groups
            .iter()
            .find(|g| g.id == id)
            .map(|g| g.key.clone())
            .unwrap_or_default()
    };
    // Currencies recorded abroad, never an account's own.
    let own: Vec<&str> = ledger.accounts.iter().map(|a| a.currency.code()).collect();
    let mut currencies: Vec<String> = ledger
        .transactions
        .iter()
        .filter_map(|t| t.original_amount.map(|o| o.currency().code().to_owned()))
        .filter(|c| !own.contains(&c.as_str()))
        .collect();
    currencies.sort();
    currencies.dedup();
    // The merchant names the ledger shows: applied merchant decisions.
    let mut merchants: Vec<String> = ledger
        .decisions
        .iter()
        .filter(|d| d.state == fm_core::model::DecisionState::Applied)
        .filter_map(|d| match &d.assignment {
            fm_core::model::Assignment::Merchant(m) => Some(m.clone()),
            fm_core::model::Assignment::Bucket(_) => None,
        })
        .collect();
    merchants.sort();
    merchants.dedup();
    Vocabulary {
        categories: t
            .categories
            .iter()
            .filter(|c| !c.hidden && c.merged_into.is_none())
            .map(|c| Category {
                key: c.key.clone(),
                name: c.name.clone(),
                group: group_key(c.group_id),
            })
            .collect(),
        groups: t
            .groups
            .iter()
            .map(|g| Group {
                key: g.key.clone(),
                name: g.name.clone(),
            })
            .collect(),
        accounts: ledger
            .accounts
            .iter()
            .map(|a| Account {
                id: a.id.0,
                name: a.name.clone(),
                kind: serde_json::to_value(a.kind)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_default(),
            })
            .collect(),
        currencies,
        merchants,
    }
}

struct Args {
    model: PathBuf,
    sha256: String,
    id: String,
    lead: String,
    only: Option<Vec<String>>,
    out: Option<PathBuf>,
    threads: Option<u32>,
    questions: Option<PathBuf>,
    rows: Option<PathBuf>,
}

fn args() -> Result<Args, String> {
    let mut a = Args {
        model: PathBuf::new(),
        sha256: String::new(),
        id: String::new(),
        lead: String::new(),
        only: None,
        out: None,
        threads: None,
        questions: None,
        rows: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = || it.next().ok_or(format!("{flag} needs a value"));
        match flag.as_str() {
            "--model" => a.model = PathBuf::from(value()?),
            "--sha256" => a.sha256 = value()?,
            "--id" => a.id = value()?,
            "--lead" => a.lead = value()?.replace("\\n", "\n"),
            "--only" => a.only = Some(value()?.split(',').map(str::to_owned).collect()),
            "--out" => a.out = Some(PathBuf::from(value()?)),
            "--questions" => a.questions = Some(PathBuf::from(value()?)),
            "--rows" => a.rows = Some(PathBuf::from(value()?)),
            "--threads" => a.threads = Some(value()?.parse().map_err(|_| "bad --threads")?),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    if a.model.as_os_str().is_empty() || a.sha256.len() != 64 {
        return Err("--model <file.gguf> and --sha256 <64 hex> are required".into());
    }
    if a.id.is_empty() {
        a.id = a
            .model
            .file_stem()
            .map(|s| s.to_string_lossy().to_lowercase())
            .unwrap_or_default();
    }
    Ok(a)
}

fn score(
    q: &Question,
    ledger: &Ledger,
    reference: &[(V, Option<Vec<i64>>)],
    expected_periods: &[Option<(Date, Date)>],
    query: &Query,
    ctx: &Context<'_>,
) -> (Outcome, bool, Option<bool>, String) {
    let answers = match query::run(ledger, query, ctx) {
        Ok(a) => a,
        Err(e) => return (Outcome::RunError, false, None, e.to_string()),
    };
    let extra = answers.len() > reference.len();
    let mut used = vec![false; answers.len()];
    let mut ids_agree = true;
    let values: Vec<(V, Vec<i64>)> = answers.iter().map(|a| got(&a.value)).collect();
    for (want, want_ids) in reference {
        let hit = values
            .iter()
            .enumerate()
            .find(|(i, (v, _))| !used[*i] && v == want);
        let Some((i, (_, ids))) = hit else {
            return (
                Outcome::WrongValue,
                extra,
                None,
                format!("{want:?} not found"),
            );
        };
        used[i] = true;
        if let Some(w) = want_ids {
            ids_agree &= w == ids;
        }
    }
    let stray = !expected_periods.is_empty()
        && periods(ledger, query, ctx.as_of).iter().any(|p| {
            p.is_some_and(|(from, to)| {
                expected_periods
                    .iter()
                    .flatten()
                    .all(|(ef, et)| to < *ef || from > *et)
            })
        });
    if stray {
        return (Outcome::StrayPeriod, extra, Some(ids_agree), String::new());
    }
    let _ = q;
    (Outcome::Correct, extra, Some(ids_agree), String::new())
}

fn main() -> ExitCode {
    let a = match args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let size = match std::fs::metadata(&a.model) {
        Ok(m) => m.len(),
        Err(e) => {
            eprintln!("can't read the model file: {e}");
            return ExitCode::FAILURE;
        }
    };
    let Ok(sha) = std::panic::catch_unwind(|| sha256_hex(&a.sha256)) else {
        eprintln!("--sha256 isn't 64 hex digits");
        return ExitCode::from(2);
    };
    let spec = ModelSpec {
        id: a.id.clone().into(),
        url: "local".into(),
        file_name: a
            .model
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
            .into(),
        size,
        sha256: sha,
    };
    // Load verifies the file's size and SHA-256 against the pin.
    let model = match Model::load(&a.model, &spec) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("can't load the model: {e}");
            return ExitCode::FAILURE;
        }
    };

    let set: QuestionSet = match &a.questions {
        None => build(),
        Some(path) => match std::fs::read(path)
            .map_err(|e| e.to_string())
            .and_then(|b| serde_json::from_slice(&b).map_err(|e| e.to_string()))
        {
            Ok(s) => s,
            Err(e) => {
                eprintln!("can't read the question set: {e}");
                return ExitCode::FAILURE;
            }
        },
    };
    let (ledger, truth) = labelled::ledger_for(&set.persona);
    let labeler = labelled::TruthLabeler::new(&ledger, &truth);
    let ctx = Context {
        as_of: set.today,
        labeler: Some(&labeler),
    };
    let vocabulary = vocabulary(&ledger);
    let as_of = set.today.to_string();
    let options = GenerateOptions {
        sampling: Sampling::Greedy,
        max_tokens: 768,
        threads: a.threads,
    };

    // Results of an earlier, interrupted run of the same model, prompt and set.
    let mut done: Vec<Row> = a
        .rows
        .as_ref()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|text| {
            text.lines()
                .filter_map(|l| serde_json::from_str(l).ok())
                .collect()
        })
        .unwrap_or_default();
    let mut rows = Vec::new();
    for q in &set.questions {
        if let Some(i) = done.iter().position(|r| r.id == q.id) {
            rows.push(done.swap_remove(i));
            continue;
        }
        if a.only.as_ref().is_some_and(|o| !o.contains(&q.id)) {
            continue;
        }
        let reference: Vec<(V, Option<Vec<i64>>)> = q.answers.iter().map(expected).collect();
        let expected_periods = q
            .expected_query
            .as_ref()
            .map(|eq| periods(&ledger, eq, set.today))
            .unwrap_or_default();

        let mut request = Request::new(&q.question, &as_of, query::SCHEMA, &vocabulary);
        request.answer_lead = &a.lead;
        let start = Instant::now();
        let result = nl_query::interpret(&model, &request, &options);
        let millis = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
        let row = match result {
            Err(e) => Row {
                id: q.id.clone(),
                kind: q.kind.clone(),
                proxy: q.proxy.is_some(),
                outcome: Outcome::NoAnswer,
                completed: Outcome::NoAnswer,
                extra_items: false,
                completed_extra: false,
                ids_agree: None,
                confidence_bp: 0,
                millis,
                query: String::new(),
                reading: vec![],
                note: e.to_string(),
            },
            Ok(i) => match serde_json::from_str::<Query>(&i.json) {
                Err(e) => Row {
                    id: q.id.clone(),
                    kind: q.kind.clone(),
                    proxy: q.proxy.is_some(),
                    outcome: Outcome::NoParse,
                    completed: Outcome::NoParse,
                    extra_items: false,
                    completed_extra: false,
                    ids_agree: None,
                    confidence_bp: i.confidence_bp,
                    millis,
                    query: i.json,
                    reading: vec![],
                    note: e.to_string(),
                },
                Ok(parsed) => {
                    let (outcome, extra_items, ids_agree, note) =
                        score(q, &ledger, &reference, &expected_periods, &parsed, &ctx);
                    let completed_query: Option<Query> =
                        serde_json::from_str::<serde_json::Value>(&i.json)
                            .ok()
                            .and_then(|v| {
                                serde_json::from_value(nl_query::complete(&v).query).ok()
                            });
                    // As the app uses completion: if the completed query doesn't run,
                    // the model's own query is answered instead.
                    let (completed, completed_extra) = match &completed_query {
                        Some(cq) => {
                            match score(q, &ledger, &reference, &expected_periods, cq, &ctx) {
                                (Outcome::RunError, _, _, _) => (outcome, extra_items),
                                (o, e, _, _) => (o, e),
                            }
                        }
                        None => (outcome, extra_items),
                    };
                    Row {
                        id: q.id.clone(),
                        kind: q.kind.clone(),
                        proxy: q.proxy.is_some(),
                        outcome,
                        completed,
                        extra_items,
                        completed_extra,
                        ids_agree,
                        confidence_bp: i.confidence_bp,
                        millis,
                        reading: query::describe(&ledger, &parsed, set.today),
                        query: i.json,
                        note,
                    }
                }
            },
        };
        eprintln!(
            "{} {:?} completed {:?} {} ms {} bp",
            row.id, row.outcome, row.completed, row.millis, row.confidence_bp
        );
        if row.outcome != Outcome::Correct || row.completed != row.outcome {
            eprintln!("  {}\n  {}", row.note, row.query);
        }
        if let Some(p) = &a.rows {
            use std::io::Write as _;
            let line = serde_json::to_string(&row).unwrap_or_default();
            if let Err(e) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)
                .and_then(|mut f| writeln!(f, "{line}"))
            {
                eprintln!("can't record the result: {e}");
            }
        }
        rows.push(row);
    }

    let report = report(&a, &spec, &rows, &as_of);
    print!("{report}");
    if let Some(out) = &a.out
        && let Err(e) = std::fs::write(out, &report)
    {
        eprintln!("can't write the report: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn is_correct(r: &Row) -> bool {
    r.outcome == Outcome::Correct
}

fn is_completed_correct(r: &Row) -> bool {
    r.completed == Outcome::Correct
}

fn pct(n: usize, d: usize) -> String {
    if d == 0 {
        "–".into()
    } else {
        format!("{:.1}%", 100.0 * n as f64 / d as f64)
    }
}

fn report(a: &Args, spec: &ModelSpec, rows: &[Row], as_of: &str) -> String {
    let n = rows.len();
    let ok = rows.iter().filter(|r| is_correct(r)).count();
    let plain: Vec<&Row> = rows.iter().filter(|r| !r.proxy).collect();
    let plain_ok = plain.iter().filter(|r| is_correct(r)).count();
    let mut out = String::new();
    let _ = writeln!(out, "# nl-eval: {}\n", spec.id);
    let _ = writeln!(
        out,
        "- Model file: `{}` ({} bytes), SHA-256 `{}`",
        spec.file_name, spec.size, a.sha256
    );
    let _ = writeln!(
        out,
        "- Prompt `{}`, greedy, as of {as_of}, answer lead {:?}",
        nl_query::PROMPT_VERSION,
        a.lead
    );
    let _ = writeln!(
        out,
        "- Questions: {}",
        a.questions
            .as_ref()
            .and_then(|p| p.file_name())
            .map_or("TD.3".to_owned(), |f| f.to_string_lossy().into_owned())
    );
    let _ = writeln!(out, "\n## Result\n");
    let _ = writeln!(out, "| | Correct | |\n|---|---|---|");
    let _ = writeln!(out, "| All questions | {ok}/{n} | {} |", pct(ok, n));
    let _ = writeln!(
        out,
        "| Without the {} proxies | {plain_ok}/{} | {} |",
        n - plain.len(),
        plain.len(),
        pct(plain_ok, plain.len())
    );
    let mut kinds: Vec<&str> = rows.iter().map(|r| r.kind.as_str()).collect();
    kinds.sort();
    kinds.dedup();
    for k in kinds {
        let of: Vec<&Row> = rows.iter().filter(|r| r.kind == k).collect();
        let c = of.iter().filter(|r| is_correct(r)).count();
        let _ = writeln!(out, "| {k} | {c}/{} | {} |", of.len(), pct(c, of.len()));
    }
    let count = |o: Outcome| rows.iter().filter(|r| r.outcome == o).count();
    let extra = rows.iter().filter(|r| r.extra_items).count();
    let agree = rows.iter().filter(|r| r.ids_agree == Some(true)).count();
    let judged = rows.iter().filter(|r| r.ids_agree.is_some()).count();
    let mean = |v: Vec<u64>| {
        if v.is_empty() {
            0
        } else {
            v.iter().sum::<u64>() / v.len() as u64
        }
    };
    let conf = |c: bool| {
        mean(
            rows.iter()
                .filter(|r| (r.outcome == Outcome::Correct) == c && r.outcome != Outcome::NoAnswer)
                .map(|r| u64::from(r.confidence_bp))
                .collect(),
        )
    };
    let cok = rows.iter().filter(|r| is_completed_correct(r)).count();
    let cplain = plain.iter().filter(|r| is_completed_correct(r)).count();
    let cextra = rows.iter().filter(|r| r.completed_extra).count();
    let _ = writeln!(out, "\n## With answer completion (`nl_query::complete`)\n");
    let _ = writeln!(out, "| | Correct | |\n|---|---|---|");
    let _ = writeln!(out, "| All questions | {cok}/{n} | {} |", pct(cok, n));
    let _ = writeln!(
        out,
        "| Without the proxies | {cplain}/{} | {} |",
        plain.len(),
        pct(cplain, plain.len())
    );
    let _ = writeln!(
        out,
        "\nExtra items with completion: {cextra} of {n} questions."
    );
    let _ = writeln!(out, "\nThe completion rules (each only adds items):\n");
    let _ = writeln!(out, "| The model wrote | Added | Rule |\n|---|---|---|");
    for r in nl_query::COMPLETION_RULES {
        let _ = writeln!(out, "| {} | {} | `{}` |", r.wrote, r.adds, r.id);
    }
    let _ = writeln!(out, "\n## Breakdown (without completion)\n");
    let _ = writeln!(
        out,
        "- Wrong value {}, stray period {}, run error {}, no parse {}, no answer {}",
        count(Outcome::WrongValue),
        count(Outcome::StrayPeriod),
        count(Outcome::RunError),
        count(Outcome::NoParse),
        count(Outcome::NoAnswer)
    );
    let _ = writeln!(
        out,
        "- Extra items (more answers than expected): {extra} of {n} questions"
    );
    let _ = writeln!(
        out,
        "- Evidence ids agree with TD.3 on {agree} of {judged} questions whose values all matched"
    );
    let _ = writeln!(
        out,
        "- Mean confidence: {} bp when correct, {} bp when wrong",
        conf(true),
        conf(false)
    );
    let mut times: Vec<u64> = rows.iter().map(|r| r.millis).collect();
    times.sort_unstable();
    let at = |q: usize| {
        times
            .get((times.len() * q).div_ceil(100).saturating_sub(1))
            .copied()
            .unwrap_or(0)
    };
    let _ = writeln!(
        out,
        "- Time per question: mean {} ms, median {} ms, p95 {} ms",
        mean(rows.iter().map(|r| r.millis).collect()),
        at(50),
        at(95)
    );
    // How many items completion adds to an answer (each is badged in the UI).
    let added: Vec<usize> = rows
        .iter()
        .filter_map(|r| serde_json::from_str::<serde_json::Value>(&r.query).ok())
        .map(|q| nl_query::complete(&q).added.len())
        .collect();
    let mut by_count = std::collections::BTreeMap::new();
    for n in &added {
        *by_count.entry(*n).or_insert(0usize) += 1;
    }
    let spread: Vec<String> = by_count
        .iter()
        .map(|(k, v)| format!("{k} added: {v}"))
        .collect();
    let _ = writeln!(
        out,
        "- Items completion adds per answer: {} (of {} answered questions)",
        spread.join(", "),
        added.len()
    );
    let _ = writeln!(out, "\n## Every question\n");
    let _ = writeln!(
        out,
        "| Id | Type | Proxy | Outcome | Completed | Extra | Confidence | ms |\n|---|---|---|---|---|---|---|---|"
    );
    for r in rows {
        let _ = writeln!(
            out,
            "| {} | {} | {} | {:?} | {:?} | {} | {} | {} |",
            r.id,
            r.kind,
            if r.proxy { "yes" } else { "" },
            r.outcome,
            r.completed,
            if r.extra_items { "yes" } else { "" },
            r.confidence_bp,
            r.millis
        );
    }
    let _ = writeln!(out, "\n## What went wrong\n");
    for r in rows.iter().filter(|r| r.outcome != Outcome::Correct) {
        let _ = writeln!(out, "### {} ({:?})\n", r.id, r.outcome);
        if !r.note.is_empty() {
            let _ = writeln!(out, "{}\n", r.note);
        }
        for line in &r.reading {
            let _ = writeln!(out, "- {line}");
        }
        if !r.query.is_empty() {
            let _ = writeln!(out, "\n```json\n{}\n```\n", r.query);
        }
    }
    out
}
