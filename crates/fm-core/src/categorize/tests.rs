//! Categorisation tests: the precedence policy (constraint 2), seen decisions
//! (constraint 7), rules against pairs, corrections re-applying to history, and the same
//! through a real encrypted database with TD.1's renter household.

use std::fs;
use std::path::PathBuf;

use super::correct::{Correction, Learn, correct, correct_ledger, learned_matcher, preview};
use super::plan::{Held, LedgerView, Outcome, Scope, decide, needs_review_source, plan};
use super::{CategorizeError, apply_to_ledger, run, run_on_ledger};
use crate::contract;
use crate::model::*;
use crate::storage::{KdfParams, Unlocked, Vault, default_taxonomy};

const NOW: Timestamp = Timestamp::from_unix_seconds(1_770_000_000);

fn usd(minor: i64) -> Money {
    Money::new(minor, Currency::USD)
}

fn category(taxonomy: &Taxonomy, key: &str) -> CategoryId {
    taxonomy
        .categories
        .iter()
        .find(|c| c.key == key)
        .unwrap_or_else(|| panic!("no category {key}"))
        .id
}

fn cat(taxonomy: &Taxonomy, key: &str) -> Bucket {
    Bucket::Category {
        category_id: category(taxonomy, key),
    }
}

fn txn(id: i64, account: i64, minor: i64, description: &str) -> Transaction {
    Transaction {
        id: TransactionId(id),
        account_id: AccountId(account),
        source: SourceRef {
            import_id: ImportId(account),
            index: u32::try_from(id).unwrap(),
            external_id: None,
        },
        status: TransactionStatus::Posted,
        posted_date: Date::from_ymd(2026, 3, 1).unwrap(),
        transaction_date: None,
        amount: usd(minor),
        original_amount: None,
        description: description.into(),
        payee: None,
        memo: None,
        trn_type: None,
        check_number: None,
    }
}

/// Checking (1) and savings (2), both the user's, with the default category list.
fn mini(transactions: Vec<Transaction>) -> Ledger {
    let account = |id: i64, kind| Account {
        id: AccountId(id),
        name: format!("Account {id}"),
        kind,
        currency: Currency::USD,
        institution: None,
        masked_number: None,
        external_id: None,
        own_account: true,
        created_at: NOW,
    };
    let import = |id: i64| {
        let mut raw = RawImport::new(ImportId(id), ImportSource::Manual, NOW, vec![id as u8]);
        raw.account_id = Some(AccountId(id));
        raw
    };
    Ledger {
        accounts: vec![
            account(1, AccountKind::Checking),
            account(2, AccountKind::Savings),
        ],
        imports: vec![import(1), import(2)],
        transactions,
        taxonomy: default_taxonomy(),
        decisions: Vec::new(),
        rules: Vec::new(),
    }
}

fn user_rule(id: i64, pattern: &str, bucket: Option<Bucket>, merchant: Option<&str>) -> Rule {
    Rule {
        id: RuleId(id),
        matcher: RuleMatcher {
            description: TextMatch::Contains(pattern.into()),
            account_id: None,
            amount: None,
        },
        action: RuleAction {
            bucket,
            merchant: merchant.map(Into::into),
        },
        origin: RuleOrigin::User,
        priority: 0,
        enabled: true,
        created_at: NOW,
    }
}

/// The one applied decision of a kind (bucket or merchant) for a transaction.
fn applied(l: &Ledger, t: i64, bucket_kind: bool) -> &Decision {
    let mut found = l.decisions.iter().filter(|d| {
        d.transaction_id.0 == t
            && d.state == DecisionState::Applied
            && matches!(d.assignment, Assignment::Bucket(_)) == bucket_kind
    });
    let d = found
        .next()
        .unwrap_or_else(|| panic!("no applied decision for {t}"));
    assert!(found.next().is_none(), "several applied decisions for {t}");
    d
}

fn bucket(l: &Ledger, t: i64) -> Bucket {
    match applied(l, t, true).assignment {
        Assignment::Bucket(b) => b,
        Assignment::Merchant(_) => unreachable!(),
    }
}

fn merchant(l: &Ledger, t: i64) -> String {
    match &applied(l, t, false).assignment {
        Assignment::Merchant(m) => m.clone(),
        Assignment::Bucket(_) => unreachable!(),
    }
}

fn assert_contract(l: &Ledger) {
    let report = contract::check(l);
    assert!(report.is_ok(), "{report}");
}

fn model_source() -> DecisionSource {
    DecisionSource::Model(ModelProvenance {
        model_id: "test-model".into(),
        gguf_sha256: Sha256Hash::of(b"model"),
        prompt_version: "v1".into(),
        confidence: Confidence::from_basis_points(9000).unwrap(),
    })
}

fn transfer_matcher() -> DecisionSource {
    DecisionSource::Builtin {
        logic: BuiltinLogic::TransferMatcher,
        version: 1,
    }
}

fn decision(id: i64, t: i64, assignment: Assignment, source: DecisionSource) -> Decision {
    Decision {
        id: DecisionId(id),
        transaction_id: TransactionId(t),
        assignment,
        source,
        taxonomy_version: default_taxonomy().version,
        decided_at: NOW,
        state: DecisionState::Applied,
        seen_at: None,
        replaces: None,
    }
}

fn pair_with(t: i64) -> Assignment {
    Assignment::Bucket(Bucket::Transfer {
        counterpart: Some(TransactionId(t)),
    })
}

// ---- The policy, row by row.

#[test]
fn decide_follows_the_precedence_table() {
    let coffee = Assignment::Bucket(Bucket::Category {
        category_id: CategoryId(1),
    });
    let dining = Assignment::Bucket(Bucket::Category {
        category_id: CategoryId(2),
    });
    let rule = DecisionSource::Rule { rule_id: RuleId(1) };
    let cleaner = DecisionSource::Builtin {
        logic: BuiltinLogic::MerchantCleaner,
        version: 1,
    };
    let placeholder = needs_review_source();

    // 1. Nothing yet.
    assert_eq!(decide(None, &placeholder, &coffee), Outcome::Apply);
    // 2. Same thing again.
    let by_rule = decision(1, 1, coffee.clone(), rule.clone());
    assert_eq!(decide(Some(&by_rule), &rule, &coffee), Outcome::Keep(None));
    // 3. The user's decision holds against everything but the user.
    let users = decision(1, 1, coffee.clone(), DecisionSource::User);
    for source in [
        rule.clone(),
        cleaner.clone(),
        transfer_matcher(),
        model_source(),
        placeholder.clone(),
    ] {
        assert_eq!(
            decide(Some(&users), &source, &dining),
            Outcome::Keep(Some(Held::UserDecision))
        );
    }
    assert_eq!(
        decide(Some(&users), &DecisionSource::User, &dining),
        Outcome::Apply
    );
    // 4. A pair holds against everything but the user...
    let paired = decision(1, 1, pair_with(2), transfer_matcher());
    for source in [rule.clone(), model_source(), placeholder.clone()] {
        assert_eq!(
            decide(Some(&paired), &source, &coffee),
            Outcome::Keep(Some(Held::TransferPair))
        );
    }
    assert_eq!(
        decide(Some(&paired), &DecisionSource::User, &coffee),
        Outcome::Apply
    );
    //    ...and the matcher's pair refines a rule's unpaired transfer.
    let unpaired = decision(
        1,
        1,
        Assignment::Bucket(Bucket::Transfer { counterpart: None }),
        rule.clone(),
    );
    assert_eq!(
        decide(Some(&unpaired), &transfer_matcher(), &pair_with(2)),
        Outcome::Apply
    );
    // 5. Weaker sources never replace stronger ones (constraint 2: a model re-run never
    //    replaces a rule's decision).
    assert_eq!(
        decide(Some(&by_rule), &model_source(), &dining),
        Outcome::Keep(None)
    );
    assert_eq!(
        decide(Some(&by_rule), &cleaner, &dining),
        Outcome::Keep(None)
    );
    let by_model = decision(1, 1, coffee.clone(), model_source());
    assert_eq!(
        decide(Some(&by_model), &placeholder, &dining),
        Outcome::Keep(None)
    );
    assert_eq!(decide(Some(&by_model), &rule, &dining), Outcome::Apply);
    // 6. Seen: the model and fixed logic suggest; the user's rules apply.
    let mut seen = by_model.clone();
    seen.seen_at = Some(NOW);
    assert_eq!(
        decide(Some(&seen), &model_source(), &dining),
        Outcome::Suggest
    );
    assert_eq!(decide(Some(&seen), &rule, &dining), Outcome::Apply);
    let mut seen_placeholder = decision(1, 1, Assignment::Bucket(Bucket::NeedsReview), placeholder);
    seen_placeholder.seen_at = Some(NOW);
    assert_eq!(
        decide(Some(&seen_placeholder), &model_source(), &dining),
        Outcome::Suggest
    );
}

// ---- Planning over a ledger.

#[test]
fn a_bare_ledger_gets_needs_review_and_clean_merchants() {
    let mut l = mini(vec![
        txn(1, 1, -450, "SQ *SAKURA RAMEN     SAN FRANCISCO       CA"),
        txn(2, 1, -1200, "Check 1085"),
    ]);
    let report = run_on_ledger(&mut l, Scope::All, NOW);
    assert_eq!(report.needs_review_added, 2);
    assert_eq!(report.cleaned_merchants, 1);
    assert_eq!(bucket(&l, 1), Bucket::NeedsReview);
    assert_eq!(applied(&l, 1, true).source, needs_review_source());
    assert_eq!(merchant(&l, 1), "Sakura Ramen");
    assert!(
        !l.decisions
            .iter()
            .any(|d| d.transaction_id.0 == 2 && matches!(d.assignment, Assignment::Merchant(_))),
        "a paper check names no merchant"
    );
    assert_contract(&l);

    // Running again changes nothing.
    let again = plan(LedgerView::from(&l), Scope::All, NOW);
    assert!(
        again.new_decisions.is_empty() && again.superseded.is_empty(),
        "{again:?}"
    );
}

#[test]
fn rules_fit_the_sign_and_replace_the_placeholder() {
    let mut l = mini(vec![
        txn(1, 1, -2599, "AMAZON MKTPL*I7BX79OSG"),
        txn(2, 1, 1999, "AMAZON MKTPL*REFUND77"),
        txn(3, 1, -500, "PAYROLL CORRECTION"),
        txn(4, 1, 250_000, "PAYROLL ACME"),
    ]);
    run_on_ledger(&mut l, Scope::All, NOW);
    let shopping = category(&l.taxonomy, "shopping.general");
    l.rules = vec![
        user_rule(
            1,
            "AMAZON",
            Some(Bucket::Category {
                category_id: shopping,
            }),
            None,
        ),
        user_rule(2, "PAYROLL", Some(Bucket::Income), Some("Acme Corp")),
    ];
    let report = run_on_ledger(&mut l, Scope::All, NOW);

    assert_eq!(
        bucket(&l, 1),
        Bucket::Category {
            category_id: shopping
        }
    );
    assert_eq!(
        bucket(&l, 2),
        Bucket::Refund {
            category_id: shopping,
            original: None
        },
        "money back from a category rule's merchant is a refund"
    );
    assert_eq!(
        bucket(&l, 3),
        Bucket::NeedsReview,
        "income doesn't fit money out"
    );
    assert_eq!(bucket(&l, 4), Bucket::Income);
    assert_eq!(report.rule_did_not_fit, 1);
    assert_eq!(report.rule_buckets, 3);
    // The rule's merchant beats the clean-up's.
    assert_eq!(merchant(&l, 4), "Acme Corp");
    assert_eq!(merchant(&l, 1), "Amazon");
    // The placeholder was superseded, not edited.
    let first = l
        .decisions
        .iter()
        .find(|d| d.transaction_id.0 == 1 && d.source == needs_review_source())
        .unwrap();
    assert_eq!(first.state, DecisionState::Superseded);
    assert_eq!(applied(&l, 1, true).replaces, Some(first.id));
    assert_contract(&l);
}

#[test]
fn rules_never_override_the_user_or_a_transfer_pair() {
    let mut l = mini(vec![
        txn(1, 1, -50_000, "ONLINE TRANSFER TO SAV"),
        txn(2, 2, 50_000, "ONLINE TRANSFER FROM CHK"),
        txn(3, 1, -1_000, "ONLINE TRANSFER TO FRIEND"),
    ]);
    let fees = cat(&l.taxonomy, "financial.bank_fees");
    l.decisions = vec![
        decision(1, 1, pair_with(2), transfer_matcher()),
        decision(2, 2, pair_with(1), transfer_matcher()),
        decision(3, 3, Assignment::Bucket(fees), DecisionSource::User),
    ];
    l.rules = vec![user_rule(
        1,
        "ONLINE TRANSFER",
        Some(Bucket::Transfer { counterpart: None }),
        None,
    )];

    let report = run_on_ledger(&mut l, Scope::All, NOW);
    assert_eq!(report.kept_transfer_pair, 2);
    assert_eq!(report.kept_user_decision, 1);
    assert_eq!(report.rule_buckets, 0);
    assert_eq!(
        bucket(&l, 1),
        Bucket::Transfer {
            counterpart: Some(TransactionId(2))
        }
    );
    assert_eq!(
        bucket(&l, 2),
        Bucket::Transfer {
            counterpart: Some(TransactionId(1))
        }
    );
    assert_eq!(bucket(&l, 3), fees);
    assert_contract(&l);
}

#[test]
fn seen_decisions_get_suggestions_from_fixed_logic_but_rules_apply() {
    let mut l = mini(vec![txn(
        1,
        1,
        -450,
        "SQ *SAKURA RAMEN     SAN FRANCISCO       CA",
    )]);
    // An older clean-up named it differently, and the user saw it and the model's bucket.
    let old_cleaner = DecisionSource::Builtin {
        logic: BuiltinLogic::MerchantCleaner,
        version: 0,
    };
    let mut seen_merchant = decision(1, 1, Assignment::Merchant("Sq Sakura".into()), old_cleaner);
    seen_merchant.seen_at = Some(NOW);
    let mut seen_model = decision(
        2,
        1,
        Assignment::Bucket(Bucket::NeedsReview),
        model_source(),
    );
    seen_model.seen_at = Some(NOW);
    l.decisions = vec![seen_merchant, seen_model];

    let report = run_on_ledger(&mut l, Scope::All, NOW);
    assert_eq!(report.suggestions, 1);
    assert_eq!(merchant(&l, 1), "Sq Sakura", "the seen name stays");
    let suggestion = l
        .decisions
        .iter()
        .find(|d| d.state == DecisionState::Suggested)
        .unwrap();
    assert_eq!(
        suggestion.assignment,
        Assignment::Merchant("Sakura Ramen".into())
    );
    assert_eq!(suggestion.replaces, Some(DecisionId(1)));
    // Not suggested twice.
    assert_eq!(run_on_ledger(&mut l, Scope::All, NOW).suggestions, 0);

    // The user's rule applies over the seen decisions, and the stale suggestion goes.
    let restaurants = cat(&l.taxonomy, "food.restaurants");
    l.rules = vec![user_rule(
        1,
        "SAKURA RAMEN",
        Some(restaurants),
        Some("Sakura Ramen"),
    )];
    let report = run_on_ledger(&mut l, Scope::All, NOW);
    assert_eq!(report.suggestions, 0);
    assert_eq!(bucket(&l, 1), restaurants);
    assert_eq!(merchant(&l, 1), "Sakura Ramen");
    assert_eq!(
        applied(&l, 1, false).source,
        DecisionSource::Rule { rule_id: RuleId(1) }
    );
    assert!(
        l.decisions
            .iter()
            .all(|d| d.state != DecisionState::Suggested)
    );
    assert_contract(&l);
}

#[test]
fn scope_limits_the_run() {
    let mut l = mini(vec![
        txn(1, 1, -100, "NETFLIX.COM"),
        txn(2, 1, -100, "NETFLIX.COM"),
    ]);
    let ids = [TransactionId(2), TransactionId(2), TransactionId(99)];
    let report = run_on_ledger(&mut l, Scope::Transactions(&ids), NOW);
    assert_eq!(report.transactions, 1);
    assert!(l.decisions.iter().all(|d| d.transaction_id.0 == 2));
}

#[test]
fn apply_to_ledger_marks_superseded_and_appends() {
    let mut l = mini(vec![txn(1, 1, -100, "NETFLIX.COM")]);
    run_on_ledger(&mut l, Scope::All, NOW);
    l.rules = vec![user_rule(1, "NETFLIX", Some(Bucket::Income), None)];
    l.rules[0].action.bucket = Some(cat(&l.taxonomy, "entertainment.streaming"));
    let p = plan(LedgerView::from(&l), Scope::All, NOW);
    assert_eq!(p.superseded.len(), 1);
    assert_eq!(p.new_decisions.len(), 1);
    let before = l.decisions.len();
    apply_to_ledger(&mut l, &p);
    assert_eq!(l.decisions.len(), before + 1);
    assert_contract(&l);
}

// ---- Corrections.

#[test]
fn learned_matchers_take_the_merchants_first_words() {
    let pattern = |d: &str| learned_matcher(d).map(|m| m.description.pattern().to_string());
    assert_eq!(
        pattern("SQ *BLUE BOTTLE COFF SAN FRANCISCO       CA").as_deref(),
        Some("BLUE BOTTLE")
    );
    assert_eq!(
        pattern("BLUE BOTTLE COFFEE 6 BERKELEY            CA").as_deref(),
        Some("BLUE BOTTLE")
    );
    assert_eq!(
        pattern("THE HOME DEPOT #4882").as_deref(),
        Some("THE HOME DEPOT")
    );
    assert_eq!(
        pattern("CITY OF COLUMBUS DES:UTILITIES ID:P8PJBASSJK INDN:X CO ID:1 CCD").as_deref(),
        Some("CITY OF COLUMBUS")
    );
    assert_eq!(pattern("NETFLIX.COM 901").as_deref(), Some("NETFLIX COM"));
    assert_eq!(
        pattern("Zelle payment to Harper Pretendley Conf# jwndcq6w7").as_deref(),
        Some("HARPER PRETENDLEY")
    );
    assert_eq!(pattern("Check 1085"), None);

    let l = mini(vec![
        txn(1, 1, -1, "SQ *BLUE BOTTLE COFF SAN FRANCISCO       CA"),
        txn(2, 1, -1, "BLUE BOTTLE COFFEE 6 BERKELEY            CA"),
        txn(3, 1, -1, "BLUE HERON BISTRO"),
    ]);
    let m = learned_matcher(&l.transactions[0].description).unwrap();
    assert_eq!(preview(LedgerView::from(&l), &m), 2);
}

#[test]
fn a_correction_becomes_a_rule_that_reapplies_to_history() {
    let mut l = mini(vec![
        txn(1, 1, -450, "SQ *BLUE BOTTLE COFF SAN FRANCISCO       CA"),
        txn(2, 1, -520, "BLUE BOTTLE COFFEE 6 BERKELEY            CA"),
        txn(3, 1, 450, "SQ *BLUE BOTTLE COFF SAN FRANCISCO       CA"),
        txn(4, 1, -800, "BLUE HERON BISTRO"),
    ]);
    run_on_ledger(&mut l, Scope::All, NOW);
    // The user has seen transaction 2's placeholder.
    let seen = applied(&l, 2, true).id;
    l.decisions
        .iter_mut()
        .find(|d| d.id == seen)
        .unwrap()
        .seen_at = Some(NOW);
    let coffee = category(&l.taxonomy, "food.coffee");
    let coffee_bucket = Bucket::Category {
        category_id: coffee,
    };

    let report = correct_ledger(
        &mut l,
        TransactionId(1),
        Correction::Bucket(coffee_bucket),
        Learn::Default,
        NOW,
    )
    .unwrap();
    let rule = l
        .rules
        .iter()
        .find(|r| Some(r.id) == report.rule)
        .expect("a rule was learned")
        .clone();
    assert_eq!(
        rule.matcher.description,
        TextMatch::Contains("BLUE BOTTLE".into())
    );
    assert_eq!(
        rule.origin,
        RuleOrigin::Correction {
            transaction_id: TransactionId(1),
            decision_id: report.decision
        }
    );
    assert_eq!(applied(&l, 1, true).source, DecisionSource::User);
    assert_eq!(
        bucket(&l, 2),
        coffee_bucket,
        "re-applied over a seen placeholder"
    );
    assert_eq!(
        applied(&l, 2, true).source,
        DecisionSource::Rule { rule_id: rule.id }
    );
    assert_eq!(
        bucket(&l, 3),
        Bucket::Refund {
            category_id: coffee,
            original: None
        }
    );
    assert_eq!(
        bucket(&l, 4),
        Bucket::NeedsReview,
        "another merchant is untouched"
    );
    assert_eq!(report.history.rule_buckets, 2);
    assert_contract(&l);

    // A merchant correction renames every Blue Bottle transaction.
    correct_ledger(
        &mut l,
        TransactionId(2),
        Correction::Merchant("Blue Bottle Coffee".into()),
        Learn::Default,
        NOW,
    )
    .unwrap();
    for t in 1..=3 {
        assert_eq!(merchant(&l, t), "Blue Bottle Coffee");
    }
    assert_eq!(merchant(&l, 4), "Blue Heron Bistro");

    // A later correction of one transaction beats the rule there, and re-running the
    // rules never takes it back.
    let restaurants = cat(&l.taxonomy, "food.restaurants");
    correct_ledger(
        &mut l,
        TransactionId(2),
        Correction::Bucket(restaurants),
        Learn::No,
        NOW,
    )
    .unwrap();
    let rerun = run_on_ledger(&mut l, Scope::All, NOW);
    assert_eq!(bucket(&l, 2), restaurants);
    assert_eq!(
        rerun.kept_user_decision, 3,
        "1's bucket, 2's bucket and merchant"
    );
    assert_eq!(bucket(&l, 1), coffee_bucket);
    assert_contract(&l);

    // Paper checks name no merchant: no rule.
    let mut l = mini(vec![txn(1, 1, -12_000, "Check 1085")]);
    run_on_ledger(&mut l, Scope::All, NOW);
    let report = correct_ledger(
        &mut l,
        TransactionId(1),
        Correction::Bucket(coffee_bucket),
        Learn::Default,
        NOW,
    )
    .unwrap();
    assert!(report.no_pattern && report.rule.is_none() && l.rules.is_empty());
}

#[test]
fn corrections_pair_and_unpair_transfers() {
    let mut l = mini(vec![
        txn(1, 1, -50_000, "ONLINE TRANSFER TO SAV"),
        txn(2, 2, 50_000, "ONLINE TRANSFER FROM CHK"),
        txn(3, 2, 50_000, "DEPOSIT"),
        txn(4, 1, -50_000, "ONLINE TRANSFER TO SAV"),
        txn(6, 2, 50_000, "ONLINE TRANSFER FROM CHK"),
    ]);
    run_on_ledger(&mut l, Scope::All, NOW);
    let to = |x: i64| {
        Correction::Bucket(Bucket::Transfer {
            counterpart: Some(TransactionId(x)),
        })
    };

    // Pairing by hand gives both sides a user decision.
    let report = correct_ledger(&mut l, TransactionId(1), to(2), Learn::No, NOW).unwrap();
    assert!(report.unpaired.is_empty());
    assert_eq!(
        bucket(&l, 1),
        Bucket::Transfer {
            counterpart: Some(TransactionId(2))
        }
    );
    assert_eq!(
        bucket(&l, 2),
        Bucket::Transfer {
            counterpart: Some(TransactionId(1))
        }
    );
    assert_eq!(applied(&l, 2, true).source, DecisionSource::User);
    assert_contract(&l);

    // Re-pairing 1 with 3 breaks 1–2. The user had paired 2, so it stays a transfer, to
    // an untracked account.
    let report = correct_ledger(&mut l, TransactionId(1), to(3), Learn::No, NOW).unwrap();
    assert_eq!(report.unpaired, [TransactionId(2)]);
    assert_eq!(bucket(&l, 2), Bucket::Transfer { counterpart: None });
    assert_eq!(
        bucket(&l, 3),
        Bucket::Transfer {
            counterpart: Some(TransactionId(1))
        }
    );
    assert_contract(&l);

    // A matcher-made pair (4–6) broken by a correction: the other side goes to review.
    let next = l.decisions.iter().map(|d| d.id.0).max().unwrap();
    for (offset, (t, other)) in [(4, 6), (6, 4)].into_iter().enumerate() {
        let current = applied(&l, t, true).id;
        l.decisions
            .iter_mut()
            .find(|d| d.id == current)
            .unwrap()
            .state = DecisionState::Superseded;
        let mut d = decision(
            next + 1 + offset as i64,
            t,
            pair_with(other),
            transfer_matcher(),
        );
        d.replaces = Some(current);
        l.decisions.push(d);
    }
    let fees = cat(&l.taxonomy, "financial.bank_fees");
    let report = correct_ledger(
        &mut l,
        TransactionId(4),
        Correction::Bucket(fees),
        Learn::No,
        NOW,
    )
    .unwrap();
    assert_eq!(report.unpaired, [TransactionId(6)]);
    assert_eq!(bucket(&l, 6), Bucket::NeedsReview);
    assert_eq!(applied(&l, 6, true).source, needs_review_source());
    assert_contract(&l);

    // Pairs that can't be one are refused: itself, the same account, a missing one, and
    // amounts that don't net to zero.
    l.transactions.push(txn(5, 2, 49_999, "ALMOST"));
    for (t, other) in [(1, 1), (1, 4), (1, 99), (4, 5)] {
        let err = correct_ledger(&mut l, TransactionId(t), to(other), Learn::No, NOW).unwrap_err();
        assert!(
            matches!(err, CategorizeError::InvalidPair),
            "{t}–{other}: {err}"
        );
    }
    assert!(matches!(
        correct_ledger(
            &mut l,
            TransactionId(99),
            Correction::Bucket(Bucket::Income),
            Learn::No,
            NOW
        ),
        Err(CategorizeError::UnknownTransaction)
    ));
    assert!(matches!(
        correct_ledger(
            &mut l,
            TransactionId(1),
            Correction::Bucket(Bucket::Category {
                category_id: CategoryId(9999)
            }),
            Learn::No,
            NOW
        ),
        Err(CategorizeError::UnknownCategory)
    ));
}

// ---- Through the encrypted database, on TD.1's renter household (the done criterion).

fn td1(persona: &str) -> (Ledger, serde_json::Value) {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/synthetic/data")
        .join(persona);
    let ledger: Ledger =
        serde_json::from_slice(&fs::read(dir.join("ledger.json")).unwrap()).unwrap();
    let truth = serde_json::from_slice(&fs::read(dir.join("truth.json")).unwrap()).unwrap();
    (ledger, truth)
}

fn new_database() -> (tempfile::TempDir, Unlocked) {
    let dir = tempfile::tempdir().unwrap();
    let u = Vault::at(&dir.path().join("ledger"))
        .create_with_params(b"correct horse battery staple", KdfParams::INSECURE_TEST)
        .unwrap();
    (dir, u)
}

#[test]
fn a_correction_reapplies_to_matching_history_in_the_database() {
    let (ledger, truth) = td1("renter");
    let (_dir, mut u) = new_database();
    u.save_ledger(&ledger).unwrap();

    let first = run(&mut u, Scope::All, NOW).unwrap();
    assert_eq!(first.needs_review_added, ledger.transactions.len());
    assert_contract(&u.load_ledger().unwrap());

    // Every truth "Blue Bottle Coffee" transaction.
    let blue_bottle: Vec<i64> = truth["transactions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["merchant"] == "Blue Bottle Coffee")
        .map(|t| t["id"].as_i64().unwrap())
        .collect();
    assert!(blue_bottle.len() > 100);
    let corrected = TransactionId(blue_bottle[0]);
    // The user has looked at some of them.
    for id in &blue_bottle[1..10] {
        let d = u
            .decisions_for_transaction(TransactionId(*id))
            .unwrap()
            .into_iter()
            .find(|d| {
                d.state == DecisionState::Applied && matches!(d.assignment, Assignment::Bucket(_))
            })
            .unwrap();
        u.mark_decision_seen(d.id, NOW).unwrap();
    }
    let coffee = cat(&ledger.taxonomy, "food.coffee");

    let report = correct(
        &mut u,
        corrected,
        Correction::Bucket(coffee),
        Learn::Default,
        NOW,
    )
    .unwrap();
    let rule_id = report.rule.unwrap();
    assert_eq!(report.history.rule_buckets, blue_bottle.len() - 1);

    let stored = u.load_ledger().unwrap();
    assert_contract(&stored);
    assert_eq!(
        applied(&stored, corrected.0, true).source,
        DecisionSource::User
    );
    for id in &blue_bottle[1..] {
        let d = applied(&stored, *id, true);
        assert_eq!(
            d.source,
            DecisionSource::Rule { rule_id },
            "transaction {id}"
        );
        assert_eq!(d.assignment, Assignment::Bucket(coffee));
    }
    // Only Blue Bottle moved.
    let by_rule = stored
        .decisions
        .iter()
        .filter(|d| {
            d.state == DecisionState::Applied && d.source == DecisionSource::Rule { rule_id }
        })
        .count();
    assert_eq!(by_rule, blue_bottle.len() - 1);

    // Running again changes nothing.
    let again = run(&mut u, Scope::All, NOW).unwrap();
    assert_eq!(
        (
            again.rule_buckets,
            again.needs_review_added,
            again.cleaned_merchants
        ),
        (0, 0, 0)
    );
    assert_eq!(
        u.load_ledger().unwrap().decisions.len(),
        stored.decisions.len()
    );

    // Deleting the corrected transaction's import deletes the learned rule and what it
    // decided (SPEC decision 28). A run afterwards puts the transactions the rule had
    // decided on other accounts back in Needs review.
    let import = stored
        .transactions
        .iter()
        .find(|t| t.id == corrected)
        .unwrap()
        .source
        .import_id;
    let deleted = u.delete_import(import).unwrap();
    assert_eq!(deleted.rules, 1);
    run(&mut u, Scope::All, NOW).unwrap();
    let after = u.load_ledger().unwrap();
    assert!(after.rules.is_empty());
    assert!(
        after
            .decisions
            .iter()
            .all(|d| !matches!(d.source, DecisionSource::Rule { .. }))
    );
    assert_contract(&after);
}

#[test]
fn the_database_run_refuses_an_open_transaction() {
    let (_dir, mut u) = new_database();
    u.connection().execute_batch("BEGIN").unwrap();
    assert!(matches!(
        run(&mut u, Scope::All, NOW),
        Err(CategorizeError::TransactionOpen)
    ));
    u.connection().execute_batch("ROLLBACK").unwrap();
    assert!(run(&mut u, Scope::All, NOW).is_ok());
}
