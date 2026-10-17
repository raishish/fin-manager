use super::*;
use crate::model::*;
use crate::storage::{KdfParams, Vault, default_taxonomy};
use crate::views::{self, GroupBy, Key, Period, Query};

const CARD: AccountId = AccountId(1);

fn key(t: &Taxonomy, key: &str) -> CategoryId {
    t.categories.iter().find(|c| c.key == key).unwrap().id
}

fn group_of(t: &Taxonomy, id: CategoryId) -> CategoryGroupId {
    t.category(id).unwrap().group_id
}

/// The default list, two months of card spending: coffee and groceries, a refund, one
/// transaction in Needs review.
fn ledger() -> Ledger {
    let taxonomy = default_taxonomy();
    let coffee = key(&taxonomy, "food.coffee");
    let groceries = key(&taxonomy, "food.groceries");
    let mut ledger = Ledger {
        accounts: vec![Account {
            id: CARD,
            name: "card".into(),
            kind: AccountKind::CreditCard,
            currency: Currency::USD,
            institution: None,
            masked_number: None,
            external_id: None,
            own_account: true,
            created_at: Timestamp::from_unix_seconds(0),
        }],
        imports: vec![],
        transactions: vec![],
        taxonomy,
        decisions: vec![],
        rules: vec![],
    };
    let rows: [(i64, &str, i64, Option<Bucket>); 6] = [
        (
            1,
            "2026-08-03",
            -450,
            Some(Bucket::Category {
                category_id: coffee,
            }),
        ),
        (
            2,
            "2026-08-10",
            -6_000,
            Some(Bucket::Category {
                category_id: groceries,
            }),
        ),
        (
            3,
            "2026-08-12",
            1_000,
            Some(Bucket::Refund {
                category_id: groceries,
                original: None,
            }),
        ),
        (
            4,
            "2026-09-02",
            -500,
            Some(Bucket::Category {
                category_id: coffee,
            }),
        ),
        (
            5,
            "2026-09-09",
            -7_500,
            Some(Bucket::Category {
                category_id: groceries,
            }),
        ),
        (6, "2026-09-20", -1_234, None),
    ];
    for (id, posted, minor, bucket) in rows {
        ledger.transactions.push(Transaction {
            id: TransactionId(id),
            account_id: CARD,
            source: SourceRef {
                import_id: ImportId(1),
                index: u32::try_from(id).unwrap(),
                external_id: None,
            },
            status: TransactionStatus::Posted,
            posted_date: posted.parse().unwrap(),
            transaction_date: None,
            amount: Money::new(minor, Currency::USD),
            original_amount: None,
            description: format!("SHOP {id}"),
            payee: None,
            memo: None,
            trn_type: None,
            check_number: None,
        });
        let bucket = bucket.unwrap_or(Bucket::NeedsReview);
        ledger.decisions.push(Decision {
            id: DecisionId(id),
            transaction_id: TransactionId(id),
            assignment: Assignment::Bucket(bucket),
            source: DecisionSource::User,
            taxonomy_version: ledger.taxonomy.version.clone(),
            decided_at: Timestamp::from_unix_seconds(0),
            state: DecisionState::Applied,
            seen_at: None,
            replaces: None,
        });
    }
    ledger
}

fn by_category(ledger: &Ledger) -> views::Breakdown {
    views::breakdown(
        ledger,
        &Query {
            period: Period::months("2026-08".parse().unwrap(), "2026-09".parse().unwrap()).unwrap(),
            filter: Default::default(),
            group_by: GroupBy::Category,
            measure: Default::default(),
            top: None,
            as_of: "2026-10-17".parse().unwrap(),
        },
    )
    .unwrap()
}

fn row(b: &views::Breakdown, id: CategoryId) -> Option<&views::Row> {
    b.rows.iter().find(|r| r.key == Key::Category { id })
}

#[test]
fn rename_shows_on_past_transactions_without_rewriting_them() {
    let mut l = ledger();
    let coffee = key(&l.taxonomy, "food.coffee");
    let decisions_before = l.decisions.clone();
    l.taxonomy = apply(
        &l.taxonomy,
        &Edit::RenameCategory {
            id: coffee,
            name: "  Café runs ".into(),
        },
    )
    .unwrap();
    assert_eq!(l.taxonomy.category(coffee).unwrap().name, "Café runs");
    assert_eq!(l.taxonomy.version.revision, 1);
    assert_eq!(l.decisions, decisions_before, "nothing re-written");
    let view = by_category(&l);
    let coffee_row = row(&view, coffee).unwrap();
    assert_eq!(coffee_row.label, "Café runs");
    assert_eq!(
        coffee_row.transaction_ids,
        [TransactionId(1), TransactionId(4)]
    );
}

#[test]
fn merge_moves_past_totals_into_the_target() {
    let mut l = ledger();
    let coffee = key(&l.taxonomy, "food.coffee");
    let groceries = key(&l.taxonomy, "food.groceries");
    let before = by_category(&l);
    let total_before = before.total;
    assert_eq!(
        row(&before, groceries).unwrap().total,
        Money::new(12_500, Currency::USD)
    );
    assert_eq!(
        transactions_in(&l, coffee),
        [TransactionId(1), TransactionId(4)],
        "what the UI says will move"
    );

    l.taxonomy = apply(
        &l.taxonomy,
        &Edit::Merge {
            from: coffee,
            into: groceries,
        },
    )
    .unwrap();
    let after = by_category(&l);
    assert!(
        row(&after, coffee).is_none(),
        "nothing counts as coffee any more"
    );
    let g = row(&after, groceries).unwrap();
    // 60.00 + 75.00 − 10.00 refund + 4.50 + 5.00
    assert_eq!(g.total, Money::new(13_450, Currency::USD));
    assert_eq!(
        g.transaction_ids,
        [1, 2, 3, 4, 5].map(TransactionId),
        "past coffee transactions now count as groceries"
    );
    assert_eq!(
        after.total, total_before,
        "a merge moves money, never adds or drops it"
    );
    assert_eq!(
        transactions_in(&l, groceries),
        [1, 2, 3, 4, 5].map(TransactionId)
    );
    assert!(transactions_in(&l, coffee).is_empty());
    assert!(
        !l.taxonomy.selectable().any(|c| c.id == coffee),
        "a merged category leaves the pickers"
    );
}

#[test]
fn merging_a_merge_target_moves_its_merged_categories_too() {
    let t = default_taxonomy();
    let a = key(&t, "food.coffee");
    let b = key(&t, "food.groceries");
    let c = key(&t, "food.restaurants");
    let t = apply(&t, &Edit::Merge { from: a, into: b }).unwrap();
    let t = apply(&t, &Edit::Merge { from: b, into: c }).unwrap();
    assert_eq!(
        t.category(a).unwrap().merged_into,
        Some(c),
        "no chain a → b → c"
    );
    assert_eq!(t.category(b).unwrap().merged_into, Some(c));
    assert_eq!(t.version.revision, 2);
    t.validate().unwrap();
}

#[test]
fn hiding_changes_pickers_not_totals() {
    let mut l = ledger();
    let coffee = key(&l.taxonomy, "food.coffee");
    let before = by_category(&l);
    l.taxonomy = apply(
        &l.taxonomy,
        &Edit::SetCategoryHidden {
            id: coffee,
            hidden: true,
        },
    )
    .unwrap();
    assert!(!l.taxonomy.selectable().any(|c| c.id == coffee));
    let after = by_category(&l);
    assert_eq!(after.total, before.total);
    assert_eq!(
        row(&after, coffee).unwrap().total,
        row(&before, coffee).unwrap().total
    );

    let food = group_of(&l.taxonomy, coffee);
    l.taxonomy = apply(
        &l.taxonomy,
        &Edit::SetGroupHidden {
            id: food,
            hidden: true,
        },
    )
    .unwrap();
    assert!(
        !l.taxonomy.selectable().any(|c| c.group_id == food),
        "a hidden group hides its categories from pickers"
    );
    assert_eq!(by_category(&l).total, before.total);

    l.taxonomy = apply(
        &l.taxonomy,
        &Edit::SetGroupHidden {
            id: food,
            hidden: false,
        },
    )
    .unwrap();
    l.taxonomy = apply(
        &l.taxonomy,
        &Edit::SetCategoryHidden {
            id: coffee,
            hidden: false,
        },
    )
    .unwrap();
    assert!(l.taxonomy.selectable().any(|c| c.id == coffee));
    assert_eq!(l.taxonomy.version.revision, 4);
}

#[test]
fn edits_that_change_nothing_keep_the_revision() {
    let t = default_taxonomy();
    let coffee = key(&t, "food.coffee");
    let name = t.category(coffee).unwrap().name.clone();
    let same = apply(&t, &Edit::RenameCategory { id: coffee, name }).unwrap();
    assert_eq!(same, t);
    let same = apply(
        &t,
        &Edit::SetCategoryHidden {
            id: coffee,
            hidden: false,
        },
    )
    .unwrap();
    assert_eq!(same, t);
}

#[test]
fn bad_edits_are_refused() {
    let t = default_taxonomy();
    let coffee = key(&t, "food.coffee");
    let groceries = key(&t, "food.groceries");
    let groceries_name = t.category(groceries).unwrap().name.clone();
    let food = group_of(&t, coffee);
    let other_group = t.groups.iter().find(|g| g.id != food).unwrap();

    let refused = |edit: Edit| apply(&t, &edit).unwrap_err();
    assert!(matches!(
        refused(Edit::RenameCategory {
            id: coffee,
            name: "   ".into()
        }),
        EditError::InvalidName
    ));
    assert!(matches!(
        refused(Edit::RenameCategory {
            id: coffee,
            name: "x".repeat(MAX_NAME_CHARS + 1)
        }),
        EditError::InvalidName
    ));
    assert!(matches!(
        refused(Edit::RenameCategory {
            id: coffee,
            name: "a\nb".into()
        }),
        EditError::InvalidName
    ));
    assert!(matches!(
        refused(Edit::RenameCategory {
            id: coffee,
            name: groceries_name.to_uppercase()
        }),
        EditError::NameTaken
    ));
    assert!(matches!(
        refused(Edit::RenameGroup {
            id: food,
            name: other_group.name.clone()
        }),
        EditError::NameTaken
    ));
    assert!(matches!(
        refused(Edit::RenameCategory {
            id: CategoryId(999_999),
            name: "X".into()
        }),
        EditError::UnknownCategory(_)
    ));
    assert!(matches!(
        refused(Edit::SetGroupHidden {
            id: CategoryGroupId(999_999),
            hidden: true
        }),
        EditError::UnknownGroup(_)
    ));
    assert!(matches!(
        refused(Edit::Merge {
            from: coffee,
            into: coffee
        }),
        EditError::MergeIntoItself
    ));

    let merged = apply(
        &t,
        &Edit::Merge {
            from: coffee,
            into: groceries,
        },
    )
    .unwrap();
    for edit in [
        Edit::Merge {
            from: groceries,
            into: coffee,
        },
        Edit::Merge {
            from: coffee,
            into: groceries,
        },
        Edit::RenameCategory {
            id: coffee,
            name: "Coffee 2".into(),
        },
        Edit::SetCategoryHidden {
            id: coffee,
            hidden: true,
        },
    ] {
        assert!(
            matches!(apply(&merged, &edit), Err(EditError::Merged(id)) if id == coffee),
            "{edit:?}"
        );
    }
    // A merged-away category's old name is free again.
    let coffee_name = t.category(coffee).unwrap().name.clone();
    let other = groceries;
    apply(
        &merged,
        &Edit::RenameCategory {
            id: other,
            name: coffee_name,
        },
    )
    .unwrap();
}

#[test]
fn errors_name_ids_only() {
    let t = default_taxonomy();
    let coffee = key(&t, "food.coffee");
    let name = t.category(key(&t, "food.groceries")).unwrap().name.clone();
    let err = apply(
        &t,
        &Edit::RenameCategory {
            id: coffee,
            name: name.clone(),
        },
    )
    .unwrap_err();
    assert!(!err.to_string().contains(&name));
}

#[test]
fn edits_are_saved_to_the_encrypted_ledger() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::at(&dir.path().join("ledger"));
    let mut u = vault
        .create_with_params(b"correct horse battery staple", KdfParams::INSECURE_TEST)
        .unwrap();
    let t = u.taxonomy().unwrap();
    let coffee = key(&t, "food.coffee");
    let groceries = key(&t, "food.groceries");

    let saved = edit(
        &mut u,
        &Edit::RenameCategory {
            id: coffee,
            name: "Café".into(),
        },
    )
    .unwrap();
    assert_eq!(u.taxonomy().unwrap(), saved);
    edit(
        &mut u,
        &Edit::Merge {
            from: coffee,
            into: groceries,
        },
    )
    .unwrap();
    let stored = u.taxonomy().unwrap();
    assert_eq!(stored.version.revision, t.version.revision + 2);
    assert_eq!(
        stored.category(coffee).unwrap().merged_into,
        Some(groceries)
    );
    assert_eq!(stored.category(coffee).unwrap().name, "Café");

    // A refused edit leaves the stored list as it was.
    assert!(
        edit(
            &mut u,
            &Edit::Merge {
                from: coffee,
                into: groceries
            }
        )
        .is_err()
    );
    assert_eq!(u.taxonomy().unwrap(), stored);
}

/// Review T3.3 #5: merging into a hidden category, or one in a hidden group, is refused.
#[test]
fn merging_into_a_hidden_category_is_refused() {
    let t = default_taxonomy();
    let coffee = key(&t, "food.coffee");
    let groceries = key(&t, "food.groceries");
    let hidden = apply(
        &t,
        &Edit::SetCategoryHidden {
            id: groceries,
            hidden: true,
        },
    )
    .unwrap();
    let err = apply(
        &hidden,
        &Edit::Merge {
            from: coffee,
            into: groceries,
        },
    )
    .unwrap_err();
    assert!(matches!(err, EditError::HiddenTarget(id) if id == groceries));
    assert!(err.to_string().contains("show it before merging"));

    let rent = t
        .categories
        .iter()
        .find(|c| c.group_id != group_of(&t, coffee))
        .unwrap()
        .id;
    let group = group_of(&t, rent);
    let hidden_group = apply(
        &t,
        &Edit::SetGroupHidden {
            id: group,
            hidden: true,
        },
    )
    .unwrap();
    assert!(matches!(
        apply(
            &hidden_group,
            &Edit::Merge {
                from: coffee,
                into: rent
            }
        ),
        Err(EditError::HiddenTarget(_))
    ));
    // Shown again, the merge goes through.
    let shown = apply(
        &hidden,
        &Edit::SetCategoryHidden {
            id: groceries,
            hidden: false,
        },
    )
    .unwrap();
    apply(
        &shown,
        &Edit::Merge {
            from: coffee,
            into: groceries,
        },
    )
    .unwrap();
}
