//! Transfer detection measured on TD.1's ground truth (directly and through the
//! database), on TD.2's bank exports imported through the pipeline, and on hand-written
//! cases that don't come from either.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use serde_json::Value;
use tempfile::TempDir;

use super::pairing::{PARAMS, Params, candidates, find_pairs_with};
use super::*;
use crate::contract;
use crate::model::*;
use crate::pipeline::{ImportRequest, PIPELINE_VERSION, Target, import_file};
use crate::storage::{KdfParams, Table, Unlocked, Vault};

const PERSONAS: [&str; 4] = ["renter", "family", "freelancer", "retiree"];
const PASSWORD: &[u8] = b"correct horse battery staple";

fn data() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata")
}

fn json(path: PathBuf) -> Value {
    serde_json::from_slice(&fs::read(&path).unwrap()).unwrap()
}

/// A persona's TD.1 ledger and its true transfer pairs, as (out, in).
fn household(persona: &str) -> (Ledger, BTreeSet<(i64, i64)>) {
    let dir = data().join("synthetic/data").join(persona);
    let ledger = serde_json::from_slice(&fs::read(dir.join("ledger.json")).unwrap()).unwrap();
    let truth = json(dir.join("truth.json"));
    let pairs = truth["transfer_pairs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| (p["out"].as_i64().unwrap(), p["in"].as_i64().unwrap()))
        .collect();
    (ledger, pairs)
}

fn pair_ids(pairs: &[Pair]) -> BTreeSet<(i64, i64)> {
    pairs.iter().map(|p| (p.out.0, p.inflow.0)).collect()
}

#[derive(Debug, Default, Clone, Copy)]
struct Score {
    right: usize,
    wrong: usize,
    missed: usize,
}

impl Score {
    fn of(found: &BTreeSet<(i64, i64)>, expected: &BTreeSet<(i64, i64)>) -> Score {
        Score {
            right: found.intersection(expected).count(),
            wrong: found.difference(expected).count(),
            missed: expected.difference(found).count(),
        }
    }

    fn add(&mut self, other: Score) {
        self.right += other.right;
        self.wrong += other.wrong;
        self.missed += other.missed;
    }

    fn precision(self) -> f64 {
        self.right as f64 / (self.right + self.wrong).max(1) as f64
    }

    fn recall(self) -> f64 {
        self.right as f64 / (self.right + self.missed).max(1) as f64
    }
}

// ---- The pairing on its own.

#[test]
fn td1_precision_and_recall() {
    let mut total = Score::default();
    for persona in PERSONAS {
        let (ledger, expected) = household(persona);
        let found = pair_ids(&find_pairs(&ledger.accounts, &ledger.transactions, |_| {
            true
        }));
        let score = Score::of(&found, &expected);
        println!(
            "{persona}: {} right, {} wrong, {} missed of {}",
            score.right,
            score.wrong,
            score.missed,
            expected.len()
        );
        total.add(score);
    }
    println!(
        "TD.1: precision {:.4}, recall {:.4}",
        total.precision(),
        total.recall()
    );
    assert!(total.precision() >= 0.95, "precision {}", total.precision());
}

/// Where the true pairs and the false candidates sit, by evidence score and by lag.
/// The threshold is only meaningful if it sits in a gap between the two.
#[test]
fn td1_near_misses_and_lags() {
    let mut true_scores: BTreeMap<u32, usize> = BTreeMap::new();
    let mut false_scores: BTreeMap<u32, usize> = BTreeMap::new();
    let mut true_lags: BTreeMap<i32, usize> = BTreeMap::new();
    for persona in PERSONAS {
        let (ledger, expected) = household(persona);
        // A wide window, to see true pairs the default window would miss.
        for c in candidates(&ledger.accounts, &ledger.transactions, |_| true, 30) {
            if expected.contains(&(c.out.0, c.inflow.0)) {
                *true_scores.entry(c.score).or_default() += 1;
                *true_lags.entry(c.lag).or_default() += 1;
            } else if c.lag <= PARAMS.max_lag_days {
                *false_scores.entry(c.score).or_default() += 1;
            }
        }
    }
    println!("true pairs by evidence score: {true_scores:?}");
    println!("false candidates within the window, by score: {false_scores:?}");
    println!("true pairs by lag in days: {true_lags:?}");
    let lowest_true = *true_scores.keys().next().unwrap();
    let highest_false = false_scores.keys().last().copied().unwrap_or(0);
    assert!(
        highest_false < PARAMS.min_score && PARAMS.min_score <= lowest_true,
        "the threshold should sit between false ({highest_false}) and true ({lowest_true}) scores"
    );
}

/// Precision and recall across a grid of windows and thresholds: the defaults should sit
/// on a plateau, not on a peak only TD.1 has.
#[test]
fn td1_sensitivity_to_window_and_threshold() {
    let households: Vec<_> = PERSONAS.iter().map(|p| household(p)).collect();
    let mut table = Vec::new();
    for min_score in 1..=4 {
        for max_lag_days in [1, 2, 3, 4, 5, 7, 10, 14] {
            let params = Params {
                max_lag_days,
                min_score,
            };
            let mut total = Score::default();
            for (ledger, expected) in &households {
                let found = pair_ids(&find_pairs_with(
                    params,
                    &ledger.accounts,
                    &ledger.transactions,
                    |_| true,
                ));
                total.add(Score::of(&found, expected));
            }
            println!(
                "min_score {min_score}, window {max_lag_days:>2}: precision {:.4}, recall {:.4}",
                total.precision(),
                total.recall()
            );
            table.push((params, total));
        }
    }
    // Every window from 3 to 14 days at threshold 2 or 3 keeps precision at the bar.
    for (params, score) in &table {
        if (2..=3).contains(&params.min_score) && params.max_lag_days >= 3 {
            assert!(score.precision() >= 0.95, "{params:?}: {score:?}");
        }
    }
}

// ---- Hand-written cases, in the wording banks use, not taken from TD.1 or TD.2.

fn account(id: i64, name: &str, kind: AccountKind, institution: &str, last4: &str) -> Account {
    Account {
        id: AccountId(id),
        name: name.into(),
        kind,
        currency: Currency::USD,
        institution: Some(institution.into()),
        masked_number: Some(last4.into()),
        external_id: None,
        own_account: true,
        created_at: Timestamp::from_unix_seconds(0),
    }
}

fn row(id: i64, account: i64, date: &str, minor: i64, description: &str) -> Transaction {
    Transaction {
        id: TransactionId(id),
        account_id: AccountId(account),
        source: SourceRef {
            import_id: ImportId(1),
            index: id as u32,
            external_id: None,
        },
        status: TransactionStatus::Posted,
        posted_date: date.parse().unwrap(),
        transaction_date: None,
        amount: Money::new(minor, Currency::USD),
        original_amount: None,
        description: description.into(),
        payee: None,
        memo: None,
        trn_type: None,
        check_number: None,
    }
}

#[test]
fn hand_written_cases() {
    let accounts = [
        account(
            1,
            "Everyday Checking",
            AccountKind::Checking,
            "Wells Fargo",
            "5521",
        ),
        account(
            2,
            "Way2Save Savings",
            AccountKind::Savings,
            "Wells Fargo",
            "8830",
        ),
        account(
            3,
            "Discover It",
            AccountKind::CreditCard,
            "Discover",
            "6011",
        ),
        account(
            4,
            "Sapphire Preferred",
            AccountKind::CreditCard,
            "Chase",
            "4417",
        ),
        account(
            5,
            "High Yield Savings",
            AccountKind::Savings,
            "Marcus",
            "0937",
        ),
    ];
    let cases = [
        // Transfers, in each bank's own wording.
        (
            "own transfer, same bank",
            row(
                1,
                1,
                "2025-03-03",
                -50_000,
                "ONLINE TRANSFER TO WAY2SAVE SAVINGS XXXXXX8830 REF #IB0X7",
            ),
            row(
                2,
                2,
                "2025-03-03",
                50_000,
                "ONLINE TRANSFER FROM EVERYDAY CHECKING XXXXXX5521 REF #IB0X7",
            ),
            true,
        ),
        (
            "own transfer to another bank over a holiday weekend",
            row(
                3,
                1,
                "2025-05-23",
                -100_000,
                "MARCUS SAVINGS TRANSFER PPD ID: 9000123",
            ),
            row(
                4,
                5,
                "2025-05-28",
                100_000,
                "ACH Deposit WELLS FARGO TRANSFER",
            ),
            true,
        ),
        (
            "card payment",
            row(
                5,
                1,
                "2025-03-10",
                -23_456,
                "DISCOVER E-PAYMENT 6011 WEB ID: 2510020270",
            ),
            row(6, 3, "2025-03-11", 23_456, "INTERNET PAYMENT - THANK YOU"),
            true,
        ),
        (
            "card payment, autopay",
            row(
                7,
                1,
                "2025-03-15",
                -98_765,
                "CHASE CREDIT CRD AUTOPAY PPD ID: 4760039224",
            ),
            row(8, 4, "2025-03-17", 98_765, "AUTOMATIC PAYMENT - THANK YOU"),
            true,
        ),
        // The same amount out and in, but not a transfer.
        (
            "a card refund the same day as a cash withdrawal",
            row(9, 1, "2025-04-02", -6_000, "ATM WITHDRAWAL 0412 MAIN ST"),
            row(10, 3, "2025-04-02", 6_000, "AMAZON MKTPLACE REFUND"),
            false,
        ),
        (
            "money sent to one person and the same amount from another",
            row(11, 1, "2025-04-05", -4_000, "ZELLE PAYMENT TO JAMIE 8812"),
            row(12, 2, "2025-04-06", 4_000, "ZELLE PAYMENT FROM ALEX 1290"),
            false,
        ),
        (
            "a bill and a deposit of the same amount",
            row(13, 1, "2025-04-10", -12_000, "PG&E WEB ONLINE PAYMENT"),
            row(14, 5, "2025-04-11", 12_000, "INTEREST PAYMENT"),
            false,
        ),
        (
            "money leaving a card is never a transfer",
            row(15, 4, "2025-04-12", -30_000, "PAYMENT TO DISCOVER"),
            row(16, 3, "2025-04-12", 30_000, "INTERNET PAYMENT - THANK YOU"),
            false,
        ),
        (
            "the other side posted three weeks later",
            row(
                17,
                1,
                "2025-06-02",
                -25_000,
                "ONLINE TRANSFER TO WAY2SAVE SAVINGS XXXXXX8830",
            ),
            row(
                18,
                2,
                "2025-06-23",
                25_000,
                "ONLINE TRANSFER FROM EVERYDAY CHECKING XXXXXX5521",
            ),
            false,
        ),
        (
            "transfer wording on one side only",
            row(19, 1, "2025-07-01", -7_500, "ONLINE TRANSFER TO SAVINGS"),
            row(20, 3, "2025-07-02", 7_500, "PURCHASE RETURN BEST BUY 00123"),
            false,
        ),
        (
            "a payment naming a card the user doesn't have",
            row(21, 1, "2025-07-05", -15_000, "CAPITAL ONE MOBILE PMT"),
            row(22, 4, "2025-07-05", 15_000, "STATEMENT CREDIT"),
            false,
        ),
        // Two transfers of one amount three days apart pair by the nearest dates.
        (
            "the first of two equal transfers",
            row(
                23,
                1,
                "2025-08-01",
                -20_000,
                "ONLINE TRANSFER TO WAY2SAVE SAVINGS XXXXXX8830",
            ),
            row(
                24,
                2,
                "2025-08-01",
                20_000,
                "ONLINE TRANSFER FROM EVERYDAY CHECKING XXXXXX5521",
            ),
            true,
        ),
        (
            "the second of two equal transfers",
            row(
                25,
                1,
                "2025-08-04",
                -20_000,
                "ONLINE TRANSFER TO WAY2SAVE SAVINGS XXXXXX8830",
            ),
            row(
                26,
                2,
                "2025-08-04",
                20_000,
                "ONLINE TRANSFER FROM EVERYDAY CHECKING XXXXXX5521",
            ),
            true,
        ),
    ];
    let transactions: Vec<Transaction> = cases
        .iter()
        .flat_map(|(_, o, i, _)| [o.clone(), i.clone()])
        .collect();
    let found = pair_ids(&find_pairs(&accounts, &transactions, |_| true));
    let wrong: Vec<&str> = cases
        .iter()
        .filter(|(_, o, i, is_pair)| found.contains(&(o.id.0, i.id.0)) != *is_pair)
        .map(|c| c.0)
        .collect();
    assert!(wrong.is_empty(), "{wrong:?}");
    assert_eq!(
        found.len(),
        cases.iter().filter(|c| c.3).count(),
        "{found:?}"
    );
}

/// A bill paid the same day, from the same account, for the same amount as a savings
/// move: the bill's `WEB PMT` must not win the deposit (T1.6). Wording is Verizon's and
/// Wells Fargo's, not TD.6's.
#[test]
fn a_biller_payment_does_not_take_a_savings_deposit() {
    let accounts = [
        account(
            1,
            "Everyday Checking",
            AccountKind::Checking,
            "Wells Fargo",
            "5521",
        ),
        account(
            2,
            "Way2Save Savings",
            AccountKind::Savings,
            "Wells Fargo",
            "8830",
        ),
        account(
            3,
            "Sapphire Preferred",
            AccountKind::CreditCard,
            "Chase",
            "4417",
        ),
    ];
    let transactions = [
        // The bill comes first, so it would win a tie.
        row(
            1,
            1,
            "2025-09-12",
            -8_500,
            "VERIZON WIRELESS WEB PMT 250912 PAT EXAMPLE",
        ),
        row(2, 1, "2025-09-12", -8_500, "ONLINE TRANSFER REF #IB0QW9"),
        row(
            3,
            2,
            "2025-09-12",
            8_500,
            "ONLINE TRANSFER FROM EVERYDAY CHECKING XXXXXX5521",
        ),
        // A card payment still counts its payment words.
        row(4, 1, "2025-09-15", -40_000, "CHASE CREDIT CRD EPAY 250915"),
        row(5, 3, "2025-09-16", 40_000, "PAYMENT THANK YOU"),
    ];
    let found = pair_ids(&find_pairs(&accounts, &transactions, |_| true));
    assert_eq!(found, BTreeSet::from([(2, 3), (4, 5)]));
}

// ---- Detection on a ledger.

fn new_ledger() -> (TempDir, Unlocked) {
    let dir = tempfile::tempdir().unwrap();
    let u = Vault::at(&dir.path().join("ledger"))
        .create_with_params(PASSWORD, KdfParams::INSECURE_TEST)
        .unwrap();
    (dir, u)
}

fn now() -> Timestamp {
    Timestamp::from_unix_seconds(1_800_000_000)
}

fn pipeline_source() -> DecisionSource {
    DecisionSource::Builtin {
        logic: BuiltinLogic::ImportPipeline,
        version: PIPELINE_VERSION,
    }
}

fn matcher_source() -> DecisionSource {
    DecisionSource::Builtin {
        logic: BuiltinLogic::TransferMatcher,
        version: MATCHER_VERSION,
    }
}

/// A persona's TD.1 accounts and transactions, each with only the import pipeline's
/// applied Needs review decision, as after importing them.
fn needs_review_ledger(persona: &str) -> (TempDir, Unlocked, BTreeSet<(i64, i64)>) {
    let (mut ledger, expected) = household(persona);
    let version = ledger.taxonomy.version.clone();
    ledger.rules.clear();
    ledger.decisions = ledger
        .transactions
        .iter()
        .map(|t| Decision {
            id: DecisionId(t.id.0),
            transaction_id: t.id,
            assignment: Assignment::Bucket(Bucket::NeedsReview),
            source: pipeline_source(),
            taxonomy_version: version.clone(),
            decided_at: Timestamp::from_unix_seconds(1_700_000_000),
            state: DecisionState::Applied,
            seen_at: None,
            replaces: None,
        })
        .collect();
    let (dir, mut u) = new_ledger();
    u.save_ledger(&ledger).unwrap();
    (dir, u, expected)
}

fn bucket_decisions(u: &Unlocked, id: i64) -> Vec<Decision> {
    u.decisions_for_transaction(TransactionId(id))
        .unwrap()
        .into_iter()
        .filter(|d| matches!(d.assignment, Assignment::Bucket(_)))
        .collect()
}

fn applied_bucket(u: &Unlocked, id: i64) -> Decision {
    let mut applied: Vec<_> = bucket_decisions(u, id)
        .into_iter()
        .filter(|d| d.state == DecisionState::Applied)
        .collect();
    assert_eq!(applied.len(), 1, "transaction {id}");
    applied.remove(0)
}

fn transfer_to(counterpart: i64) -> Assignment {
    Assignment::Bucket(Bucket::Transfer {
        counterpart: Some(TransactionId(counterpart)),
    })
}

/// Replaces transaction `id`'s applied bucket with one from `source`.
fn decide(u: &Unlocked, id: i64, source: DecisionSource, bucket: Bucket) -> DecisionId {
    let old = applied_bucket(u, id);
    u.set_decision_state(old.id, DecisionState::Superseded)
        .unwrap();
    let new = DecisionId(u.next_id(Table::Decisions).unwrap());
    u.insert_decision(&Decision {
        id: new,
        source,
        assignment: Assignment::Bucket(bucket),
        replaces: Some(old.id),
        ..old
    })
    .unwrap();
    new
}

fn a_category(u: &Unlocked) -> Bucket {
    Bucket::Category {
        category_id: u.taxonomy().unwrap().categories[0].id,
    }
}

fn involves(found: &BTreeSet<(i64, i64)>, id: i64) -> bool {
    found.iter().any(|&(o, i)| o == id || i == id)
}

#[test]
fn detection_on_td1_meets_the_bar_keeps_the_contract_and_is_idempotent() {
    let mut total = Score::default();
    for persona in PERSONAS {
        let (_dir, mut u, expected) = needs_review_ledger(persona);
        let report = detect(&mut u, now()).unwrap();
        assert!(report.suggested.is_empty(), "nothing was seen");
        let found = pair_ids(&report.applied);
        total.add(Score::of(&found, &expected));

        // Each pair points both ways, replaces Needs review, and supersedes it.
        for &(out, inflow) in &found {
            for (id, other) in [(out, inflow), (inflow, out)] {
                let d = applied_bucket(&u, id);
                assert_eq!(d.assignment, transfer_to(other));
                assert_eq!(d.source, matcher_source());
                assert_eq!(d.decided_at, now());
                let old = u.decision(d.replaces.unwrap()).unwrap().unwrap();
                assert_eq!(old.source, pipeline_source());
                assert_eq!(old.state, DecisionState::Superseded);
            }
        }
        let ledger = u.load_ledger().unwrap();
        let check = contract::check(&ledger);
        assert!(check.is_ok(), "{persona}: {check}");

        // A second run finds nothing and changes nothing.
        assert_eq!(detect(&mut u, now()).unwrap(), TransferReport::default());
        assert_eq!(u.load_ledger().unwrap(), ledger);
    }
    println!(
        "TD.1 through the ledger: precision {:.4}, recall {:.4} ({} right, {} wrong, {} missed)",
        total.precision(),
        total.recall(),
        total.right,
        total.wrong,
        total.missed
    );
    assert!(total.precision() >= 0.95);
}

#[test]
fn user_and_rule_decisions_are_never_overridden() {
    let (_dir, mut u, expected) = needs_review_ledger("renter");
    let mut pairs = expected.iter();
    let &(user_out, user_in) = pairs.next().unwrap();
    let &(rule_out, rule_in) = pairs.next().unwrap();

    let category = a_category(&u);
    let user = decide(&u, user_out, DecisionSource::User, category);
    let rule = Rule {
        id: RuleId(1),
        matcher: RuleMatcher {
            description: TextMatch::Contains("TRANSFER".into()),
            account_id: None,
            amount: None,
        },
        action: RuleAction {
            bucket: Some(category),
            merchant: None,
        },
        origin: RuleOrigin::User,
        priority: 0,
        enabled: true,
        created_at: now(),
    };
    u.insert_rule(&rule).unwrap();
    let by_rule = decide(
        &u,
        rule_in,
        DecisionSource::Rule { rule_id: rule.id },
        category,
    );

    let found = pair_ids(&detect(&mut u, now()).unwrap().applied);
    assert!(!involves(&found, user_out));
    assert!(!involves(&found, rule_in));
    assert!(!found.contains(&(user_out, user_in)));
    assert!(!found.contains(&(rule_out, rule_in)));
    assert_eq!(applied_bucket(&u, user_out).id, user);
    assert_eq!(applied_bucket(&u, rule_in).id, by_rule);
    let check = contract::check(&u.load_ledger().unwrap());
    assert!(check.is_ok(), "{check}");
}

#[test]
fn a_model_decision_is_superseded() {
    let (_dir, mut u, expected) = needs_review_ledger("family");
    let &(out, inflow) = expected.iter().next().unwrap();
    let provenance = ModelProvenance {
        model_id: "test-model".into(),
        gguf_sha256: Sha256Hash::of(b"test-model"),
        prompt_version: "1".into(),
        confidence: Confidence::from_basis_points(6_000).unwrap(),
    };
    let category = a_category(&u);
    let model = decide(&u, out, DecisionSource::Model(provenance), category);

    let report = detect(&mut u, now()).unwrap();
    assert!(pair_ids(&report.applied).contains(&(out, inflow)));
    let d = applied_bucket(&u, out);
    assert_eq!(d.assignment, transfer_to(inflow));
    assert_eq!(d.replaces, Some(model));
    assert_eq!(
        u.decision(model).unwrap().unwrap().state,
        DecisionState::Superseded
    );
}

#[test]
fn a_seen_side_makes_both_sides_suggestions_and_a_dismissal_sticks() {
    let (_dir, mut u, expected) = needs_review_ledger("freelancer");
    let &(out, inflow) = expected.iter().next().unwrap();
    let seen = applied_bucket(&u, inflow);
    u.mark_decision_seen(seen.id, now()).unwrap();

    let report = detect(&mut u, now()).unwrap();
    assert_eq!(pair_ids(&report.suggested), BTreeSet::from([(out, inflow)]));
    assert!(!pair_ids(&report.applied).contains(&(out, inflow)));
    let mut suggestions = Vec::new();
    for (id, other) in [(out, inflow), (inflow, out)] {
        // Needs review stays applied on both sides; the transfer waits for the user.
        let applied = applied_bucket(&u, id);
        assert_eq!(applied.source, pipeline_source());
        let suggestion = bucket_decisions(&u, id)
            .into_iter()
            .find(|d| d.state == DecisionState::Suggested)
            .unwrap();
        assert_eq!(suggestion.assignment, transfer_to(other));
        assert_eq!(suggestion.replaces, Some(applied.id));
        suggestions.push(suggestion.id);
    }
    let check = contract::check(&u.load_ledger().unwrap());
    assert!(check.is_ok(), "{check}");

    // The user turns the suggestion down: the pair isn't offered again.
    for id in suggestions {
        u.set_decision_state(id, DecisionState::Dismissed).unwrap();
    }
    let before = u.load_ledger().unwrap();
    assert_eq!(detect(&mut u, now()).unwrap(), TransferReport::default());
    assert_eq!(u.load_ledger().unwrap(), before);
}

#[test]
fn detection_refuses_to_run_inside_another_transaction() {
    let (_dir, mut u, _) = needs_review_ledger("retiree");
    let before = u.load_ledger().unwrap();
    u.connection().execute_batch("BEGIN").unwrap();
    assert!(matches!(
        detect(&mut u, now()),
        Err(DetectError::TransactionOpen)
    ));
    u.connection().execute_batch("ROLLBACK").unwrap();
    assert_eq!(u.load_ledger().unwrap(), before);
}

#[test]
fn an_unreleased_savepoint_rolls_back() {
    // detect returns through `?` on any error, which drops the savepoint unreleased.
    let (_dir, u, _) = needs_review_ledger("retiree");
    let before = u.load_ledger().unwrap();
    let first = applied_bucket(&u, before.transactions[0].id.0);
    {
        let _savepoint = super::detect::Savepoint::begin(u.connection()).unwrap();
        u.set_decision_state(first.id, DecisionState::Superseded)
            .unwrap();
        assert!(!u.connection().is_autocommit());
    }
    assert!(u.connection().is_autocommit());
    assert_eq!(u.load_ledger().unwrap(), before);
}

// ---- TD.2's bank exports, imported through the pipeline.

/// Imports every TD.2 file of a persona and returns the ledger with, per TD.1 truth id,
/// the transaction it became.
fn imported(persona: &str, reverse: bool) -> (TempDir, Unlocked, HashMap<i64, TransactionId>) {
    let dir = data().join("bank-exports/data");
    let manifest = json(dir.join("manifest.json"));
    let mut files: Vec<&Value> = manifest["files"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["persona"] == persona)
        .collect();
    if reverse {
        files.reverse();
    }
    let (tmp, mut u) = new_ledger();
    for account in household(persona).0.accounts {
        u.insert_account(&account).unwrap();
    }
    let profiles = fm_import_csv::ProfileSet::with_builtin();
    let mut landed = HashMap::new();
    for f in files {
        let path = f["path"].as_str().unwrap();
        let expected = json(dir.join(f["expected"].as_str().unwrap()));
        let bytes = fs::read(dir.join(path)).unwrap();
        let report = import_file(
            &mut u,
            &ImportRequest {
                bytes: &bytes,
                file_name: Some(path),
                target: Target::Account(AccountId(
                    expected["source"]["account_id"].as_i64().unwrap(),
                )),
                csv_currency: Currency::USD,
                csv_profiles: &profiles,
                now: SystemTime::UNIX_EPOCH + Duration::from_secs(1_790_000_000),
            },
        )
        .unwrap();
        let rows = expected["transactions"].as_array().unwrap();
        for ((_, outcome), row) in report.statements[0].records.iter().zip(rows) {
            landed.insert(row["truth_id"].as_i64().unwrap(), outcome.transaction());
        }
    }
    (tmp, u, landed)
}

#[test]
fn detection_on_td2_imports() {
    for reverse in [false, true] {
        let order = if reverse {
            "reverse order"
        } else {
            "file order"
        };
        let mut total = Score::default();
        for persona in PERSONAS {
            let (_dir, mut u, landed) = imported(persona, reverse);
            let (_, truth) = household(persona);
            let expected: BTreeSet<(i64, i64)> = truth
                .iter()
                .filter_map(|(o, i)| Some((landed.get(o)?.0, landed.get(i)?.0)))
                .collect();
            let report = detect(&mut u, now()).unwrap();
            let mut found = pair_ids(&report.applied);
            found.extend(pair_ids(&report.suggested));
            let score = Score::of(&found, &expected);
            println!(
                "TD.2 {persona} ({order}): {} right, {} wrong, {} missed of {}",
                score.right,
                score.wrong,
                score.missed,
                expected.len()
            );
            total.add(score);
            let check = contract::check(&u.load_ledger().unwrap());
            assert!(check.is_ok(), "{persona}: {check}");
        }
        println!(
            "TD.2 ({order}): precision {:.4}, recall {:.4}",
            total.precision(),
            total.recall()
        );
        assert!(total.precision() >= 0.95, "{total:?}");
    }
}

// ---- TD.6: look-alikes that are not transfers, and hard true transfers.

const TD6_HOUSEHOLDS: [&str; 3] = ["alex", "jordan", "pat"];

/// A TD.6 household: its ledger, its true pairs with their patterns, and every
/// look-alike candidate with its pattern.
struct Td6 {
    ledger: Ledger,
    pairs: BTreeMap<(i64, i64), String>,
    lookalikes: BTreeMap<(i64, i64), String>,
}

fn td6(household: &str) -> Td6 {
    let dir = data().join("transfer-lookalikes/data").join(household);
    let ledger = serde_json::from_slice(&fs::read(dir.join("ledger.json")).unwrap()).unwrap();
    let truth = json(dir.join("truth.json"));
    let read = |key: &str| -> BTreeMap<(i64, i64), String> {
        truth[key]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                (
                    (p["out"].as_i64().unwrap(), p["in"].as_i64().unwrap()),
                    p["pattern"].as_str().unwrap().to_owned(),
                )
            })
            .collect()
    };
    Td6 {
        ledger,
        pairs: read("transfer_pairs"),
        lookalikes: read("lookalikes"),
    }
}

/// Pairs found on TD.6 with `params`: the overall score, and per pattern how many
/// look-alikes were taken as transfers and how many true pairs were missed.
fn td6_score(
    households: &[Td6],
    params: Params,
) -> (Score, BTreeMap<String, usize>, BTreeMap<String, usize>) {
    let mut total = Score::default();
    let mut wrong: BTreeMap<String, usize> = BTreeMap::new();
    let mut missed: BTreeMap<String, usize> = BTreeMap::new();
    for h in households {
        let found = pair_ids(&find_pairs_with(
            params,
            &h.ledger.accounts,
            &h.ledger.transactions,
            |_| true,
        ));
        let expected: BTreeSet<(i64, i64)> = h.pairs.keys().copied().collect();
        total.add(Score::of(&found, &expected));
        for pair in found.difference(&expected) {
            let pattern = h
                .lookalikes
                .get(pair)
                .map_or("not a candidate", String::as_str);
            *wrong.entry(pattern.to_owned()).or_default() += 1;
        }
        for pair in expected.difference(&found) {
            *missed.entry(h.pairs[pair].clone()).or_default() += 1;
        }
    }
    (total, wrong, missed)
}

#[test]
fn td6_precision_and_recall() {
    let households: Vec<Td6> = TD6_HOUSEHOLDS.iter().map(|h| td6(h)).collect();
    let (score, wrong, missed) = td6_score(&households, PARAMS);
    println!(
        "TD.6: precision {:.4}, recall {:.4} ({} right, {} wrong, {} missed)",
        score.precision(),
        score.recall(),
        score.right,
        score.wrong,
        score.missed
    );
    println!("look-alikes taken as transfers, by pattern: {wrong:?}");
    println!("true pairs missed, by pattern: {missed:?}");
    for min_score in 1..=4 {
        for max_lag_days in [3, 5, 7] {
            let (s, w, _) = td6_score(
                &households,
                Params {
                    max_lag_days,
                    min_score,
                },
            );
            println!(
                "min_score {min_score}, window {max_lag_days}: precision {:.4}, recall {:.4}, wrong {w:?}",
                s.precision(),
                s.recall()
            );
        }
    }
    assert!(score.precision() >= 0.95, "precision {}", score.precision());
}
