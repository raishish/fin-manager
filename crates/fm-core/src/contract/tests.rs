//! Contract checker tests: correct ledgers pass (the T0.5 sample and TD.1's four
//! households with decisions built from their ground truth), and every check has a
//! deliberately broken fixture it catches.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use serde_json::Value;

use super::*;
use crate::model::round_trip_tests::sample_ledger;
use crate::model::*;

fn data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/synthetic/data")
}

/// A TD.1 household with one applied user decision per truth bucket and merchant: the
/// ledger as it would look once everything were labelled correctly.
fn td1_labelled(persona: &str) -> Ledger {
    let dir = data_dir().join(persona);
    let mut ledger: Ledger =
        serde_json::from_slice(&fs::read(dir.join("ledger.json")).unwrap()).unwrap();
    let truth: Value = serde_json::from_slice(&fs::read(dir.join("truth.json")).unwrap()).unwrap();
    let by_key: HashMap<&str, CategoryId> = ledger
        .taxonomy
        .categories
        .iter()
        .map(|c| (c.key.as_str(), c.id))
        .collect();
    let category = |key: &Value| by_key[key.as_str().unwrap()];
    let tx_id = |v: &Value| v.as_i64().map(TransactionId);

    let mut decisions = Vec::new();
    let mut next_id = 1;
    let mut decide = |transaction_id, assignment| {
        decisions.push(Decision {
            id: DecisionId(next_id),
            transaction_id,
            assignment,
            source: DecisionSource::User,
            taxonomy_version: ledger.taxonomy.version.clone(),
            decided_at: Timestamp::from_unix_seconds(1_760_000_000),
            state: DecisionState::Applied,
            seen_at: None,
            replaces: None,
        });
        next_id += 1;
    };
    for t in truth["transactions"].as_array().unwrap() {
        let id = TransactionId(t["id"].as_i64().unwrap());
        let b = &t["bucket"];
        let bucket = match b["kind"].as_str().unwrap() {
            "category" => Bucket::Category {
                category_id: category(&b["key"]),
            },
            "transfer" => Bucket::Transfer {
                counterpart: tx_id(&b["counterpart"]),
            },
            "income" => Bucket::Income,
            "refund" => Bucket::Refund {
                category_id: category(&b["key"]),
                original: tx_id(&b["original"]),
            },
            other => panic!("unexpected truth bucket {other}"),
        };
        decide(id, Assignment::Bucket(bucket));
        if let Some(m) = t["merchant"].as_str() {
            decide(id, Assignment::Merchant(m.into()));
        }
    }
    ledger.decisions = decisions;
    ledger
}

const PERSONAS: [&str; 4] = ["renter", "family", "freelancer", "retiree"];

#[test]
fn td1_households_labelled_from_truth_pass() {
    for persona in PERSONAS {
        let ledger = td1_labelled(persona);
        let report = check(&ledger);
        assert!(report.is_ok(), "{persona}: {report}");
        assert_eq!(
            report.checked,
            [
                Constraint::ExactNumbers,
                Constraint::UserInputsWin,
                Constraint::ClosedList,
                Constraint::EverythingAddsUp,
                Constraint::AdmitUncertainty,
                Constraint::NoSilentChanges,
                Constraint::SourceShown
            ]
        );
    }
}

#[test]
fn the_t0_5_sample_ledger_passes() {
    let report = check(&sample_ledger());
    assert!(report.is_ok(), "{report}");
}

#[test]
fn an_unlabelled_ledger_fails_constraint_4_for_every_transaction() {
    let ledger: Ledger =
        serde_json::from_slice(&fs::read(data_dir().join("retiree/ledger.json")).unwrap()).unwrap();
    let report = check(&ledger);
    let no_bucket = report
        .violations
        .iter()
        .filter(|v| v.code == "no_bucket")
        .count();
    assert_eq!(no_bucket, ledger.transactions.len());
    assert!(report.has("outflows_dont_add_up", Some(Subject::Ledger)));
    assert_eq!(report.of(Constraint::ExactNumbers).count(), 0);
    assert_eq!(report.of(Constraint::SourceShown).count(), 0);
}

// ---- Broken fixtures. Each starts from the passing T0.5 sample and breaks one thing.

type Breakage = (&'static str, fn(&mut Ledger), Subject);

fn tx(l: &mut Ledger, id: i64) -> &mut Transaction {
    l.transactions.iter_mut().find(|t| t.id.0 == id).unwrap()
}

fn decision(l: &mut Ledger, id: i64) -> &mut Decision {
    l.decisions.iter_mut().find(|d| d.id.0 == id).unwrap()
}

fn set_bucket(l: &mut Ledger, decision_id: i64, bucket: Bucket) {
    decision(l, decision_id).assignment = Assignment::Bucket(bucket);
}

fn model(l: &mut Ledger, decision_id: i64) -> &mut ModelProvenance {
    match &mut decision(l, decision_id).source {
        DecisionSource::Model(p) => p,
        _ => panic!("decision {decision_id} is not a model decision"),
    }
}

fn correction(l: &mut Ledger, transaction: i64, decision: i64) {
    l.rules[0].origin = RuleOrigin::Correction {
        transaction_id: TransactionId(transaction),
        decision_id: DecisionId(decision),
    };
}

fn applied(l: &Ledger, id: i64, transaction: i64, assignment: Assignment) -> Decision {
    Decision {
        id: DecisionId(id),
        transaction_id: TransactionId(transaction),
        assignment,
        source: DecisionSource::User,
        state: DecisionState::Applied,
        replaces: None,
        seen_at: None,
        ..l.decisions[0].clone()
    }
}

/// Applies each breakage to a fresh sample and checks the expected code, on the expected
/// subject, under the expected constraint.
fn assert_caught(constraint: Constraint, breakages: &[Breakage]) {
    assert!(check(&sample_ledger()).is_ok());
    for (code, breakage, subject) in breakages {
        let mut l = sample_ledger();
        breakage(&mut l);
        let report = check(&l);
        assert!(
            report
                .of(constraint)
                .any(|v| v.code == *code && v.subject == *subject),
            "{code} on {subject} not caught; got:\n{report}"
        );
    }
}

#[test]
fn constraint_2_broken_fixtures() {
    assert_caught(
        Constraint::UserInputsWin,
        &[
            (
                // A model decision replaced the user's correction of transaction 2.
                "user_input_overridden",
                |l| {
                    decision(l, 3).state = DecisionState::Superseded;
                    let mut model = decision(l, 9).clone();
                    model.id = DecisionId(12);
                    model.transaction_id = TransactionId(2);
                    model.state = DecisionState::Applied;
                    model.replaces = Some(DecisionId(3));
                    l.decisions.push(model);
                },
                Subject::Decision(DecisionId(12)),
            ),
            (
                // A rule replaced the user's correction.
                "user_input_overridden",
                |l| {
                    decision(l, 3).state = DecisionState::Superseded;
                    let mut by_rule = decision(l, 7).clone();
                    by_rule.id = DecisionId(12);
                    by_rule.transaction_id = TransactionId(2);
                    by_rule.replaces = Some(DecisionId(3));
                    l.decisions.push(by_rule);
                },
                Subject::Decision(DecisionId(12)),
            ),
            (
                // The correction was set aside and the model's decision put back.
                "user_choice_not_in_effect",
                |l| {
                    decision(l, 3).state = DecisionState::Superseded;
                    decision(l, 2).state = DecisionState::Applied;
                },
                Subject::Transaction(TransactionId(2)),
            ),
            (
                // Rule 1 matches transaction 5, but the model's decision is in effect.
                "model_over_matching_rule",
                |l| {
                    l.rules[0].matcher.amount = None;
                    tx(l, 5).description = "TRADER JOE'S #9".into();
                },
                Subject::Decision(DecisionId(9)),
            ),
        ],
    );
}

#[test]
fn constraint_2_allows_a_pair_refining_a_rules_unpaired_transfer() {
    let mut l = sample_ledger();
    // Transaction 6's user transfer becomes a rule's; the matcher then pairs it.
    let rule_transfer = Decision {
        id: DecisionId(12),
        source: DecisionSource::Rule { rule_id: RuleId(1) },
        state: DecisionState::Superseded,
        ..decision(&mut l, 8).clone()
    };
    l.decisions.retain(|d| d.id.0 != 8);
    l.decisions.push(rule_transfer);
    let pair = Decision {
        id: DecisionId(13),
        source: DecisionSource::Builtin {
            logic: BuiltinLogic::TransferMatcher,
            version: 1,
        },
        assignment: Assignment::Bucket(Bucket::Transfer {
            counterpart: Some(TransactionId(1)),
        }),
        replaces: Some(DecisionId(12)),
        state: DecisionState::Applied,
        ..decision(&mut l, 12).clone()
    };
    l.decisions.push(pair);
    let report = Checker::empty()
        .with(super::user_inputs_win::UserInputsWin)
        .run(&l);
    assert!(report.is_ok(), "{report}");
}

/// The sample's model decision on transaction 5 (decision 9), changed.
fn model_says(l: &mut Ledger, bucket: Bucket, confidence_bp: u16) {
    set_bucket(l, 9, bucket);
    model(l, 9).confidence = Confidence::from_basis_points(confidence_bp).unwrap();
}

#[test]
fn constraint_3_broken_fixtures() {
    let d = |id| Subject::Decision(DecisionId(id));
    assert_caught(
        Constraint::ClosedList,
        &[
            (
                "model_chose_transfer",
                |l| model_says(l, Bucket::Transfer { counterpart: None }, 9_500),
                d(9),
            ),
            (
                // Transaction 5 is money going out.
                "model_bucket_does_not_fit",
                |l| model_says(l, Bucket::Income, 9_500),
                d(9),
            ),
            (
                "model_bucket_does_not_fit",
                |l| {
                    model_says(
                        l,
                        Bucket::Refund {
                            category_id: CategoryId(1),
                            original: None,
                        },
                        9_500,
                    )
                },
                d(9),
            ),
        ],
    );
}

#[test]
fn constraint_5_broken_fixtures() {
    assert_caught(
        Constraint::AdmitUncertainty,
        &[(
            "below_threshold_applied",
            |l| {
                model_says(
                    l,
                    Bucket::Category {
                        category_id: CategoryId(1),
                    },
                    7_999,
                )
            },
            Subject::Decision(DecisionId(9)),
        )],
    );
    // At the threshold, and Needs review at any confidence, are fine.
    for (bucket, bp) in [
        (
            Bucket::Category {
                category_id: CategoryId(1),
            },
            8_000,
        ),
        (Bucket::NeedsReview, 10),
    ] {
        let mut l = sample_ledger();
        model_says(&mut l, bucket, bp);
        assert!(check(&l).of(Constraint::AdmitUncertainty).next().is_none());
    }
}

/// The user saw the model's decision on transaction 5 (9); a new decision replaces it.
fn replace_seen_model_decision(l: &mut Ledger, source: DecisionSource, seen: bool, bucket: Bucket) {
    let old = decision(l, 9);
    old.seen_at = Some(Timestamp::from_unix_seconds(1_760_002_000));
    old.state = DecisionState::Superseded;
    let mut new = old.clone();
    new.id = DecisionId(12);
    new.source = source;
    new.assignment = Assignment::Bucket(bucket);
    new.state = DecisionState::Applied;
    new.seen_at = seen.then(|| Timestamp::from_unix_seconds(1_760_003_000));
    new.replaces = Some(DecisionId(9));
    l.decisions.push(new);
}

/// The sample's model, more confident.
fn confident_model() -> DecisionSource {
    let mut s = sample_ledger()
        .decisions
        .into_iter()
        .find(|d| d.id == DecisionId(9))
        .unwrap()
        .source;
    if let DecisionSource::Model(p) = &mut s {
        p.confidence = Confidence::from_basis_points(9_900).unwrap();
    }
    s
}

#[test]
fn constraint_7_broken_fixtures() {
    let coffee = Bucket::Category {
        category_id: CategoryId(1),
    };
    assert_caught(
        Constraint::NoSilentChanges,
        &[(
            "silent_change",
            |l| {
                let coffee = Bucket::Category {
                    category_id: CategoryId(1),
                };
                replace_seen_model_decision(l, confident_model(), false, coffee);
            },
            Subject::Decision(DecisionId(12)),
        )],
    );
    // Not silent: an accepted suggestion (seen), the user's own correction, a rule the
    // user made, and going back to Needs review (undoing a rule, breaking a pair).
    for (source, seen, bucket) in [
        (confident_model(), true, coffee),
        (DecisionSource::User, false, coffee),
        (DecisionSource::Rule { rule_id: RuleId(1) }, false, coffee),
        (
            DecisionSource::Builtin {
                logic: BuiltinLogic::ImportPipeline,
                version: 1,
            },
            false,
            Bucket::NeedsReview,
        ),
    ] {
        let mut l = sample_ledger();
        replace_seen_model_decision(&mut l, source.clone(), seen, bucket);
        let report = check(&l);
        assert!(
            report.of(Constraint::NoSilentChanges).next().is_none(),
            "{source:?}: {report}"
        );
    }
}

#[test]
fn constraint_1_broken_fixtures() {
    let t = |id| Subject::Transaction(TransactionId(id));
    assert_caught(
        Constraint::ExactNumbers,
        &[
            (
                "currency_mismatch",
                |l| tx(l, 1).amount = Money::new(250_000, Currency::EUR),
                t(1),
            ),
            (
                "unknown_account",
                |l| tx(l, 1).account_id = AccountId(99),
                t(1),
            ),
            (
                "original_same_currency",
                |l| tx(l, 5).original_amount = Some(Money::new(-1_299, Currency::USD)),
                t(5),
            ),
            (
                "original_sign_mismatch",
                |l| tx(l, 5).original_amount = Some(Money::new(1_150, Currency::EUR)),
                t(5),
            ),
            (
                "total_overflow",
                |l| {
                    let mut big = l.transactions[0].clone();
                    big.id = TransactionId(8);
                    big.source.index = 5;
                    big.amount = Money::new(i64::MAX, Currency::USD);
                    l.transactions.push(big);
                    let d = applied(l, 20, 8, Assignment::Bucket(Bucket::Income));
                    l.decisions.push(d);
                },
                Subject::Account(AccountId(1)),
            ),
            (
                "duplicate_account_id",
                |l| {
                    let copy = l.accounts[0].clone();
                    l.accounts.push(copy);
                },
                Subject::Account(AccountId(1)),
            ),
        ],
    );
}

#[test]
fn constraint_4_broken_fixtures() {
    let t = |id| Subject::Transaction(TransactionId(id));
    assert_caught(
        Constraint::EverythingAddsUp,
        &[
            (
                "no_bucket",
                |l| l.decisions.retain(|d| d.transaction_id.0 != 1),
                t(1),
            ),
            (
                "several_buckets",
                |l| {
                    let d = applied(l, 20, 1, Assignment::Bucket(Bucket::NeedsReview));
                    l.decisions.push(d);
                },
                t(1),
            ),
            (
                "several_merchants",
                |l| {
                    let d = applied(l, 20, 2, Assignment::Merchant("Someone else".into()));
                    l.decisions.push(d);
                },
                t(2),
            ),
            (
                "unknown_category",
                |l| {
                    set_bucket(
                        l,
                        3,
                        Bucket::Category {
                            category_id: CategoryId(99),
                        },
                    )
                },
                t(2),
            ),
            (
                "unknown_refund_original",
                |l| {
                    set_bucket(
                        l,
                        7,
                        Bucket::Refund {
                            category_id: CategoryId(1),
                            original: Some(TransactionId(99)),
                        },
                    )
                },
                t(7),
            ),
            (
                "unknown_counterpart",
                |l| {
                    set_bucket(
                        l,
                        5,
                        Bucket::Transfer {
                            counterpart: Some(TransactionId(99)),
                        },
                    )
                },
                t(3),
            ),
            (
                "self_counterpart",
                |l| {
                    set_bucket(
                        l,
                        5,
                        Bucket::Transfer {
                            counterpart: Some(TransactionId(3)),
                        },
                    )
                },
                t(3),
            ),
            (
                "counterpart_not_reciprocal",
                |l| set_bucket(l, 6, Bucket::Transfer { counterpart: None }),
                t(3),
            ),
            (
                "transfer_not_net_zero",
                |l| tx(l, 4).amount = Money::new(49_999, Currency::USD),
                t(3),
            ),
            (
                "matched_transfers_not_net_zero",
                |l| tx(l, 4).amount = Money::new(49_999, Currency::USD),
                Subject::Ledger,
            ),
            (
                "same_account_transfer",
                |l| {
                    tx(l, 4).account_id = AccountId(1);
                },
                t(3),
            ),
            (
                "transfer_from_foreign_account",
                |l| l.accounts[0].own_account = false,
                t(3),
            ),
            (
                "outflow_not_spending_or_transfer",
                |l| set_bucket(l, 3, Bucket::Income),
                t(2),
            ),
            (
                "outflows_dont_add_up",
                |l| l.decisions.retain(|d| d.transaction_id.0 != 2),
                Subject::Ledger,
            ),
            (
                "duplicate_transaction_id",
                |l| {
                    let copy = l.transactions[0].clone();
                    l.transactions.push(copy);
                },
                t(1),
            ),
        ],
    );
}

#[test]
fn constraint_8_broken_fixtures() {
    let d = |id| Subject::Decision(DecisionId(id));
    let r = |id| Subject::Rule(RuleId(id));
    assert_caught(
        Constraint::SourceShown,
        &[
            (
                "unknown_rule",
                |l| {
                    decision(l, 7).source = DecisionSource::Rule {
                        rule_id: RuleId(99),
                    }
                },
                d(7),
            ),
            (
                "model_id_missing",
                |l| model(l, 9).model_id = " ".into(),
                d(9),
            ),
            (
                "model_hash_missing",
                |l| model(l, 9).gguf_sha256 = Sha256Hash::from_bytes([0; 32]),
                d(9),
            ),
            (
                "prompt_version_missing",
                |l| model(l, 9).prompt_version = String::new(),
                d(9),
            ),
            (
                "unknown_transaction",
                |l| {
                    let x = applied(l, 20, 99, Assignment::Merchant("Ghost".into()));
                    l.decisions.push(x);
                },
                d(20),
            ),
            (
                "taxonomy_version_missing",
                |l| decision(l, 1).taxonomy_version.base = String::new(),
                d(1),
            ),
            (
                "unknown_replaced_decision",
                |l| decision(l, 3).replaces = Some(DecisionId(99)),
                d(3),
            ),
            (
                "replaces_other_transaction",
                |l| decision(l, 3).replaces = Some(DecisionId(1)),
                d(3),
            ),
            (
                "replaces_itself",
                |l| decision(l, 3).replaces = Some(DecisionId(3)),
                d(3),
            ),
            (
                "duplicate_decision_id",
                |l| {
                    let copy = l.decisions[0].clone();
                    l.decisions.push(copy);
                },
                d(1),
            ),
            (
                "duplicate_rule_id",
                |l| {
                    let copy = l.rules[1].clone();
                    l.rules.push(copy);
                },
                r(2),
            ),
            (
                "unknown_correction_transaction",
                |l| correction(l, 99, 3),
                r(1),
            ),
            (
                "unknown_correction_decision",
                |l| correction(l, 2, 99),
                r(1),
            ),
            (
                "correction_about_other_transaction",
                |l| correction(l, 2, 1),
                r(1),
            ),
            ("correction_not_by_user", |l| correction(l, 2, 4), r(1)),
        ],
    );
}

#[test]
fn reports_are_deterministic_and_carry_no_financial_data() {
    let mut l = sample_ledger();
    l.decisions.retain(|d| d.transaction_id.0 != 2);
    tx(&mut l, 1).amount = Money::new(250_000, Currency::EUR);
    decision(&mut l, 7).source = DecisionSource::Rule {
        rule_id: RuleId(99),
    };
    let a = check(&l);
    // Same ledger with rows in another order: same report.
    let mut shuffled = l.clone();
    shuffled.transactions.reverse();
    shuffled.decisions.reverse();
    assert_eq!(check(&shuffled), a);
    assert!(!a.is_ok());

    let printed = a.to_string();
    assert!(
        printed.starts_with("contract check (constraints 1, 2, 3, 4, 5, 7, 8): "),
        "{printed}"
    );
    for secret in [
        "TRADER", "Trader", "2500", "250000", "CAFE", "Netflix", "ACME",
    ] {
        assert!(!printed.contains(secret), "{secret} in {printed}");
    }
    assert!(a.has("no_bucket", Some(Subject::Transaction(TransactionId(2)))));
    assert_eq!(
        a.violations[0].to_string(),
        "constraint 1 (numbers are exact): transaction 1: the amount is not in its account's currency [currency_mismatch]"
    );
}

/// Later constraints plug in through the same trait.
#[test]
fn checks_plug_into_the_checker() {
    struct NoPending;
    impl Check for NoPending {
        fn constraint(&self) -> Constraint {
            Constraint::NoSilentChanges
        }
        fn check(&self, ix: &LedgerIndex<'_>, out: &mut Vec<Violation>) {
            for t in &ix.ledger.transactions {
                if t.status == TransactionStatus::Pending {
                    out.push(Violation {
                        constraint: Constraint::NoSilentChanges,
                        subject: Subject::Transaction(t.id),
                        code: "pending",
                        message: "test check",
                    });
                }
            }
        }
    }
    let report = Checker::empty().with(NoPending).run(&sample_ledger());
    assert_eq!(report.checked, [Constraint::NoSilentChanges]);
    assert!(report.has("pending", Some(Subject::Transaction(TransactionId(6)))));
    assert!(Checker::empty().run(&sample_ledger()).is_ok());
    assert_eq!(
        Constraint::SourceShown.to_string(),
        "constraint 8 (every decision shows its source)"
    );
}
