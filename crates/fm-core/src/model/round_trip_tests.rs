//! Serialisation round trips for every model type, plus golden JSON that pins the
//! documented shape (`README.md`) so a change to it is a deliberate, visible edit.

use std::fmt::Debug;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::*;

/// Serialises `value`, reads it back, checks it is unchanged, and returns the JSON.
fn round_trip<T: Serialize + DeserializeOwned + PartialEq + Debug>(value: &T) -> Value {
    let text = serde_json::to_string(value).unwrap();
    let back: T = serde_json::from_str(&text).unwrap();
    assert_eq!(&back, value, "round trip changed the value: {text}");
    serde_json::from_str(&text).unwrap()
}

fn parse<T: DeserializeOwned>(json: Value) -> Result<T, serde_json::Error> {
    serde_json::from_value(json)
}

fn ts(seconds: i64) -> Timestamp {
    Timestamp::from_unix_seconds(seconds)
}

fn date(s: &str) -> Date {
    s.parse().unwrap()
}

fn usd(minor: i64) -> Money {
    Money::new(minor, Currency::USD)
}

fn version() -> TaxonomyVersion {
    TaxonomyVersion {
        base: "1".into(),
        revision: 0,
    }
}

fn model_source() -> DecisionSource {
    DecisionSource::Model(ModelProvenance {
        model_id: "example-3b-instruct-q4_k_m".into(),
        gguf_sha256: Sha256Hash::of(b"model"),
        prompt_version: "categorise-v1".into(),
        confidence: Confidence::from_basis_points(9_150).unwrap(),
    })
}

/// A small ledger touching every type and every enum variant: a checking account, a
/// credit card, a card payment matched as a transfer, a purchase, a refund, a foreign
/// purchase, income, a pending charge, decisions from every source and a learned rule.
pub(crate) fn sample_ledger() -> Ledger {
    let checking = Account {
        id: AccountId(1),
        name: "Everyday checking".into(),
        kind: AccountKind::Checking,
        currency: Currency::USD,
        institution: Some("Example Bank".into()),
        masked_number: Some("1234".into()),
        external_id: Some("ACT-0001".into()),
        own_account: true,
        created_at: ts(1_760_000_000),
    };
    let card = Account {
        id: AccountId(2),
        name: "Rewards card".into(),
        kind: AccountKind::CreditCard,
        currency: Currency::USD,
        institution: None,
        masked_number: None,
        external_id: None,
        own_account: true,
        created_at: ts(1_760_000_100),
    };
    let other_kinds = [
        AccountKind::Savings,
        AccountKind::Cash,
        AccountKind::Loan,
        AccountKind::Mortgage,
        AccountKind::Brokerage,
        AccountKind::Retirement,
        AccountKind::Other,
    ]
    .into_iter()
    .enumerate()
    .map(|(i, kind)| Account {
        id: AccountId(10 + i as i64),
        name: format!("{kind:?}"),
        kind,
        currency: Currency::USD,
        institution: None,
        masked_number: None,
        external_id: None,
        own_account: i % 2 == 0,
        created_at: ts(1_760_000_200),
    });

    let mut csv = RawImport::new(
        ImportId(1),
        ImportSource::Csv,
        ts(1_760_000_300),
        b"Date,Description,Amount\n2026-09-01,PAYROLL ACME,2500.00\n".to_vec(),
    );
    csv.file_name = Some("checking-2026-09.csv".into());
    csv.mapping_profile = Some("example-bank-checking-v1".into());
    csv.account_id = Some(AccountId(1));
    let ofx = RawImport::new(
        ImportId(2),
        ImportSource::Ofx,
        ts(1_760_000_400),
        b"OFXHEADER:100\n\0\xff binary-safe".to_vec(),
    );
    let others = [
        ImportSource::Qfx,
        ImportSource::SimpleFin,
        ImportSource::Manual,
    ]
    .into_iter()
    .enumerate()
    .map(|(i, source)| {
        RawImport::new(
            ImportId(3 + i as i64),
            source,
            ts(1_760_000_500),
            Vec::new(),
        )
    });

    let tx = |id: i64,
              account: i64,
              import: i64,
              index: u32,
              posted: &str,
              amount: Money,
              desc: &str| Transaction {
        id: TransactionId(id),
        account_id: AccountId(account),
        source: SourceRef {
            import_id: ImportId(import),
            index,
            external_id: None,
        },
        status: TransactionStatus::Posted,
        posted_date: date(posted),
        transaction_date: None,
        amount,
        original_amount: None,
        description: desc.into(),
        payee: None,
        memo: None,
    };
    let mut groceries = tx(2, 2, 2, 0, "2026-09-03", usd(-8_412), "TRADER JOE'S #123");
    groceries.transaction_date = Some(date("2026-09-02"));
    groceries.payee = Some("TRADER JOE'S".into());
    groceries.memo = Some("POS PURCHASE".into());
    groceries.source.external_id = Some("FITID-2026090301".into());
    let mut abroad = tx(5, 2, 2, 3, "2026-09-10", usd(-1_299), "CAFE DE FLORE PARIS");
    abroad.original_amount = Some(Money::new(-1_150, Currency::EUR));
    let mut pending = tx(6, 2, 2, 4, "2026-09-30", usd(-450), "PENDING COFFEE");
    pending.status = TransactionStatus::Pending;
    let transactions = vec![
        tx(1, 1, 1, 0, "2026-09-01", usd(250_000), "PAYROLL ACME"),
        groceries,
        tx(
            3,
            1,
            1,
            1,
            "2026-09-15",
            usd(-50_000),
            "CARD PAYMENT THANK YOU",
        ),
        tx(4, 2, 2, 1, "2026-09-16", usd(50_000), "PAYMENT RECEIVED"),
        abroad,
        pending,
        tx(7, 2, 2, 2, "2026-09-20", usd(2_100), "TRADER JOE'S REFUND"),
    ];

    let taxonomy = Taxonomy {
        version: version(),
        groups: vec![CategoryGroup {
            id: CategoryGroupId(1),
            key: "food".into(),
            name: "Food & drink".into(),
            hidden: false,
        }],
        categories: vec![
            Category {
                id: CategoryId(1),
                group_id: CategoryGroupId(1),
                key: "food.groceries".into(),
                name: "Groceries".into(),
                hidden: false,
                merged_into: None,
            },
            Category {
                id: CategoryId(2),
                group_id: CategoryGroupId(1),
                key: "food.coffee".into(),
                name: "Coffee".into(),
                hidden: true,
                merged_into: Some(CategoryId(1)),
            },
        ],
    };

    let decision = |id: i64, tx: i64, assignment: Assignment, source: DecisionSource| Decision {
        id: DecisionId(id),
        transaction_id: TransactionId(tx),
        assignment,
        source,
        taxonomy_version: version(),
        decided_at: ts(1_760_001_000 + id),
        state: DecisionState::Applied,
        seen_at: None,
        replaces: None,
    };
    let groceries_bucket = Bucket::Category {
        category_id: CategoryId(1),
    };
    let mut superseded = decision(
        2,
        2,
        Assignment::Bucket(Bucket::NeedsReview),
        model_source(),
    );
    superseded.state = DecisionState::Superseded;
    superseded.seen_at = Some(ts(1_760_001_500));
    let mut corrected = decision(
        3,
        2,
        Assignment::Bucket(groceries_bucket),
        DecisionSource::User,
    );
    corrected.replaces = Some(DecisionId(2));
    let mut suggestion = decision(
        10,
        5,
        Assignment::Bucket(Bucket::Category {
            category_id: CategoryId(2),
        }),
        model_source(),
    );
    suggestion.state = DecisionState::Suggested;
    suggestion.replaces = Some(DecisionId(9));
    let mut dismissed = suggestion.clone();
    dismissed.id = DecisionId(11);
    dismissed.state = DecisionState::Dismissed;
    let transfer = |counterpart| {
        Assignment::Bucket(Bucket::Transfer {
            counterpart: Some(TransactionId(counterpart)),
        })
    };
    let matcher = || DecisionSource::Builtin {
        logic: BuiltinLogic::TransferMatcher,
        version: 1,
    };
    let decisions = vec![
        decision(
            1,
            1,
            Assignment::Bucket(Bucket::Income),
            DecisionSource::User,
        ),
        superseded,
        corrected,
        decision(
            4,
            2,
            Assignment::Merchant("Trader Joe's".into()),
            DecisionSource::Builtin {
                logic: BuiltinLogic::MerchantCleaner,
                version: 3,
            },
        ),
        decision(5, 3, transfer(4), matcher()),
        decision(6, 4, transfer(3), matcher()),
        decision(
            7,
            7,
            Assignment::Bucket(Bucket::Refund {
                category_id: CategoryId(1),
                original: Some(TransactionId(2)),
            }),
            DecisionSource::Rule { rule_id: RuleId(1) },
        ),
        decision(
            8,
            6,
            Assignment::Bucket(Bucket::Transfer { counterpart: None }),
            DecisionSource::User,
        ),
        decision(
            9,
            5,
            Assignment::Bucket(Bucket::NeedsReview),
            model_source(),
        ),
        suggestion,
        dismissed,
    ];

    let rules = vec![
        Rule {
            id: RuleId(1),
            matcher: RuleMatcher {
                description: TextMatch::Contains("TRADER JOE".into()),
                account_id: Some(AccountId(2)),
                amount: Some(AmountMatch::Between {
                    min: usd(0),
                    max: usd(100_000),
                }),
            },
            action: RuleAction {
                bucket: Some(Bucket::Refund {
                    category_id: CategoryId(1),
                    original: None,
                }),
                merchant: Some("Trader Joe's".into()),
            },
            origin: RuleOrigin::Correction {
                transaction_id: TransactionId(2),
                decision_id: DecisionId(3),
            },
            priority: 10,
            enabled: true,
            created_at: ts(1_760_002_000),
        },
        Rule {
            id: RuleId(2),
            matcher: RuleMatcher {
                description: TextMatch::Equals("NETFLIX.COM".into()),
                account_id: None,
                amount: Some(AmountMatch::Exact(usd(-1_599))),
            },
            action: RuleAction {
                bucket: None,
                merchant: Some("Netflix".into()),
            },
            origin: RuleOrigin::User,
            priority: 0,
            enabled: false,
            created_at: ts(1_760_002_100),
        },
        Rule {
            id: RuleId(3),
            matcher: RuleMatcher {
                description: TextMatch::StartsWith("SQ *".into()),
                account_id: None,
                amount: None,
            },
            action: RuleAction {
                bucket: Some(Bucket::NeedsReview),
                merchant: None,
            },
            origin: RuleOrigin::User,
            priority: -1,
            enabled: true,
            created_at: ts(1_760_002_200),
        },
    ];

    Ledger {
        accounts: [checking, card].into_iter().chain(other_kinds).collect(),
        imports: [csv, ofx].into_iter().chain(others).collect(),
        transactions,
        taxonomy,
        decisions,
        rules,
    }
}

#[test]
fn whole_ledger_round_trips() {
    let ledger = sample_ledger();
    ledger.taxonomy.validate().unwrap();
    for rule in &ledger.rules {
        rule.validate().unwrap();
    }
    round_trip(&ledger);

    // Pretty-printed JSON reads back the same too.
    let pretty = serde_json::to_string_pretty(&ledger).unwrap();
    assert_eq!(serde_json::from_str::<Ledger>(&pretty).unwrap(), ledger);
}

#[test]
fn every_part_round_trips_on_its_own() {
    let l = sample_ledger();
    l.accounts.iter().for_each(|v| _ = round_trip(v));
    l.imports.iter().for_each(|v| _ = round_trip(v));
    l.transactions.iter().for_each(|v| _ = round_trip(v));
    l.decisions.iter().for_each(|v| _ = round_trip(v));
    l.rules.iter().for_each(|v| _ = round_trip(v));
    round_trip(&l.taxonomy);
    round_trip(&Date::MIN);
    round_trip(&Date::MAX);
    round_trip(&"2026-02".parse::<YearMonth>().unwrap());
    round_trip(&Money::new(i64::MIN, Currency::JPY));
    round_trip(&Money::new(i64::MAX, Currency::new("KWD").unwrap()));
    round_trip(&Confidence::from_basis_points(0).unwrap());
    round_trip(&Confidence::from_basis_points(10_000).unwrap());
    round_trip(&Timestamp::from_unix_seconds(-1));
}

#[test]
fn golden_transaction() {
    let l = sample_ledger();
    let tx = l
        .transactions
        .iter()
        .find(|t| t.id == TransactionId(5))
        .unwrap();
    assert_eq!(
        round_trip(tx),
        json!({
            "id": 5,
            "account_id": 2,
            "source": {"import_id": 2, "index": 3, "external_id": null},
            "status": "posted",
            "posted_date": "2026-09-10",
            "transaction_date": null,
            "amount": {"minor": -1299, "currency": "USD"},
            "original_amount": {"minor": -1150, "currency": "EUR"},
            "description": "CAFE DE FLORE PARIS",
            "payee": null,
            "memo": null
        })
    );
}

#[test]
fn golden_account() {
    let l = sample_ledger();
    assert_eq!(
        round_trip(&l.accounts[1]),
        json!({
            "id": 2,
            "name": "Rewards card",
            "kind": "credit_card",
            "currency": "USD",
            "institution": null,
            "masked_number": null,
            "external_id": null,
            "own_account": true,
            "created_at": 1_760_000_100
        })
    );
}

#[test]
fn golden_raw_import() {
    let mut raw = RawImport::new(
        ImportId(7),
        ImportSource::SimpleFin,
        ts(1_760_000_000),
        b"abc".to_vec(),
    );
    raw.account_id = Some(AccountId(3));
    assert_eq!(
        round_trip(&raw),
        json!({
            "id": 7,
            "source": "simplefin",
            "file_name": null,
            "mapping_profile": null,
            "account_id": 3,
            "imported_at": 1_760_000_000,
            "sha256": "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            "bytes": "616263"
        })
    );
}

#[test]
fn golden_decisions() {
    let l = sample_ledger();
    let by_id = |id| l.decisions.iter().find(|d| d.id == DecisionId(id)).unwrap();
    assert_eq!(
        round_trip(by_id(10)),
        json!({
            "id": 10,
            "transaction_id": 5,
            "assignment": {"bucket": {"kind": "category", "category_id": 2}},
            "source": {
                "kind": "model",
                "model_id": "example-3b-instruct-q4_k_m",
                "gguf_sha256": Sha256Hash::of(b"model").to_string(),
                "prompt_version": "categorise-v1",
                "confidence": 9150
            },
            "taxonomy_version": {"base": "1", "revision": 0},
            "decided_at": 1_760_001_010,
            "state": "suggested",
            "seen_at": null,
            "replaces": 9
        })
    );
    assert_eq!(
        round_trip(&by_id(4).assignment),
        json!({"merchant": "Trader Joe's"})
    );
    assert_eq!(
        round_trip(&by_id(4).source),
        json!({"kind": "builtin", "logic": "merchant_cleaner", "version": 3})
    );
    assert_eq!(
        round_trip(&by_id(5).assignment),
        json!({"bucket": {"kind": "transfer", "counterpart": 4}})
    );
    assert_eq!(
        round_trip(&by_id(7).source),
        json!({"kind": "rule", "rule_id": 1})
    );
    assert_eq!(round_trip(&by_id(3).source), json!({"kind": "user"}));
}

#[test]
fn golden_buckets() {
    let cases = [
        (
            Bucket::Category {
                category_id: CategoryId(4),
            },
            json!({"kind": "category", "category_id": 4}),
        ),
        (
            Bucket::Transfer { counterpart: None },
            json!({"kind": "transfer", "counterpart": null}),
        ),
        (Bucket::Income, json!({"kind": "income"})),
        (
            Bucket::Refund {
                category_id: CategoryId(4),
                original: Some(TransactionId(9)),
            },
            json!({"kind": "refund", "category_id": 4, "original": 9}),
        ),
        (Bucket::NeedsReview, json!({"kind": "needs_review"})),
    ];
    for (bucket, expected) in cases {
        assert_eq!(round_trip(&bucket), expected);
    }
}

#[test]
fn golden_rule() {
    let l = sample_ledger();
    assert_eq!(
        round_trip(&l.rules[0]),
        json!({
            "id": 1,
            "matcher": {
                "description": {"contains": "TRADER JOE"},
                "account_id": 2,
                "amount": {"between": {
                    "min": {"minor": 0, "currency": "USD"},
                    "max": {"minor": 100_000, "currency": "USD"}
                }}
            },
            "action": {
                "bucket": {"kind": "refund", "category_id": 1, "original": null},
                "merchant": "Trader Joe's"
            },
            "origin": {"kind": "correction", "transaction_id": 2, "decision_id": 3},
            "priority": 10,
            "enabled": true,
            "created_at": 1_760_002_000
        })
    );
    assert_eq!(
        round_trip(&l.rules[1].matcher.amount),
        json!({"exact": {"minor": -1599, "currency": "USD"}})
    );
}

#[test]
fn tampered_raw_import_is_rejected() {
    let raw = RawImport::new(
        ImportId(1),
        ImportSource::Csv,
        ts(0),
        b"amount,-12.34\n".to_vec(),
    );
    let mut json = serde_json::to_value(&raw).unwrap();

    // Change one byte of the payload but keep the stated hash.
    let tampered_bytes = Hex(b"amount,-92.34\n");
    json["bytes"] = Value::String(tampered_bytes.to_string());
    let err = parse::<RawImport>(json.clone()).unwrap_err().to_string();
    assert!(err.contains("hash mismatch"), "{err}");

    // Change the stated hash but keep the bytes.
    let mut json = serde_json::to_value(&raw).unwrap();
    json["sha256"] = Value::String(Sha256Hash::of(b"something else").to_string());
    assert!(parse::<RawImport>(json).is_err());

    // Bytes that aren't lowercase hex.
    let mut json = serde_json::to_value(&raw).unwrap();
    json["bytes"] = Value::String("ZZ".into());
    assert!(parse::<RawImport>(json).is_err());

    // Missing bytes.
    let mut json = serde_json::to_value(&raw).unwrap();
    json.as_object_mut().unwrap().remove("bytes");
    assert!(parse::<RawImport>(json).is_err());
}

/// Formats bytes as hex for building tampered payloads.
struct Hex<'a>(&'a [u8]);

impl std::fmt::Display for Hex<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        super::hash::write_hex(f, self.0)
    }
}

#[test]
fn invalid_values_are_rejected_when_read() {
    let tx = serde_json::to_value(&sample_ledger().transactions[0]).unwrap();
    let with = |pointer: &str, value: Value| {
        let mut v = tx.clone();
        *v.pointer_mut(pointer).unwrap() = value;
        v
    };
    for (what, bad) in [
        ("impossible date", with("/posted_date", json!("2026-02-30"))),
        ("non-ISO date", with("/posted_date", json!("09/01/2026"))),
        ("unknown currency", with("/amount/currency", json!("XYZ"))),
        ("lowercase currency", with("/amount/currency", json!("usd"))),
        ("fractional minor units", with("/amount/minor", json!(12.5))),
        ("amount as a string", with("/amount/minor", json!("1250"))),
        ("unknown status", with("/status", json!("cleared"))),
    ] {
        assert!(parse::<Transaction>(bad).is_err(), "{what} accepted");
    }

    let mut extra = tx.clone();
    extra["category"] = json!("food");
    assert!(
        parse::<Transaction>(extra).is_err(),
        "unknown field accepted"
    );

    let mut missing = tx;
    missing.as_object_mut().unwrap().remove("amount");
    assert!(
        parse::<Transaction>(missing).is_err(),
        "missing amount accepted"
    );

    assert!(parse::<Confidence>(json!(10_001)).is_err());
    assert!(parse::<Confidence>(json!(-1)).is_err());
    assert!(parse::<Sha256Hash>(json!("abc")).is_err());
    assert!(parse::<Bucket>(json!({"kind": "spending"})).is_err());
    assert!(parse::<Bucket>(json!({"kind": "category", "category_id": 1, "extra": 1})).is_err());
    let mut model = serde_json::to_value(model_source()).unwrap();
    model["extra"] = json!(1);
    assert!(parse::<DecisionSource>(model).is_err());
    assert!(
        parse::<RuleOrigin>(
            json!({"kind": "correction", "transaction_id": 1, "decision_id": 1, "x": 1})
        )
        .is_err()
    );
    assert!(parse::<DecisionSource>(json!({"kind": "model", "model_id": "x"})).is_err());
    assert!(parse::<YearMonth>(json!("2026-13")).is_err());
}
