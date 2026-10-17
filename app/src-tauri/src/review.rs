//! The review queue and transaction lists (T3.3): Needs-review transactions, drill-downs,
//! corrections that become rules, and suggestions to accept or dismiss.
//!
//! A correction is `fm_core::categorize::correct`: the user's decision, the rule learned
//! from it (unless the user says not to), and the rules re-applied to every transaction
//! the rule matches, so the merchant's past transactions follow (SPEC §2, feature 3).
//!
//! The queue holds Needs-review transactions and model decisions the user hasn't acted on
//! yet. Showing them marks nothing seen (decision 46): Approve (confirm), Reject and
//! corrections do, over one transaction or several at once.

use fm_core::categories::names::{self, NameKind};
use fm_core::categorize::normalize::normalize;
use fm_core::categorize::{self, Correction, Learn, LedgerView, correct::learned_matcher};
use fm_core::model::{
    Bucket, CategoryGroupId, CategoryId, DecisionId, Ledger, RuleMatcher, TextMatch, Timestamp,
    TransactionId,
};
use serde::{Deserialize, Serialize};

use crate::ledger_view::{
    Result, TransactionView, counting_date, current_by_transaction, locked, transaction_views,
    ui_error,
};
use crate::unlock::AppUnlock;

/// Most transactions one page of the queue holds.
pub const MAX_PAGE: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueView {
    /// Transactions in Needs review, pending ones included.
    pub total: usize,
    pub offset: usize,
    pub transactions: Vec<TransactionView>,
}

/// Where the user files a transaction (`BucketChoice` in the UI).
#[derive(Debug, Clone, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum BucketArg {
    Category {
        category_id: i64,
    },
    Refund {
        category_id: i64,
    },
    Income,
    /// Money between the user's own accounts, without naming the other side.
    Transfer,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PickerGroup {
    pub group_id: i64,
    pub name: String,
    pub categories: Vec<PickerCategory>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PickerCategory {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RulePreview {
    /// The pattern the rule would match (normalised), or `None` when the description names
    /// no merchant (a paper check): then no rule is learned.
    pub pattern: Option<String>,
    /// "This rule matches N transactions".
    pub matches: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CorrectionView {
    pub rule_id: Option<i64>,
    /// No rule was learned: the description names no merchant.
    pub no_pattern: bool,
    /// Transactions whose transfer pair the correction broke.
    pub unpaired: Vec<i64>,
    /// Transactions the learned rule was re-applied to.
    pub reapplied: usize,
    pub rule_buckets: usize,
    pub rule_merchants: usize,
    pub rule_did_not_fit: usize,
}

impl BucketArg {
    /// Checks the choice fits: the category is one the user can choose (not hidden or
    /// merged away), and money going out is never Income or a Refund (fm-core refuses
    /// those as `DoesNotFit`; the picker doesn't offer them).
    fn bucket(self, ledger: &Ledger, money_out: bool) -> Result<Bucket> {
        if money_out && matches!(self, Self::Income | Self::Refund { .. }) {
            return Err(ui_error(
                "DoesNotFit",
                "money going out can't be income or a refund",
            ));
        }
        let selectable = |id: i64| {
            let id = CategoryId(id);
            if ledger.taxonomy.selectable().any(|c| c.id == id) {
                Ok(id)
            } else {
                Err(ui_error(
                    "UnknownCategory",
                    format!("category {} can't be chosen", id.0),
                ))
            }
        };
        Ok(match self {
            Self::Category { category_id } => Bucket::Category {
                category_id: selectable(category_id)?,
            },
            Self::Refund { category_id } => Bucket::Refund {
                category_id: selectable(category_id)?,
                original: None,
            },
            Self::Income => Bucket::Income,
            Self::Transfer => Bucket::Transfer { counterpart: None },
        })
    }
}

/// The review queue, newest first: transactions in Needs review, and those whose bucket is
/// a model decision the user hasn't confirmed or corrected (decision 46).
pub fn review_queue(ledger: &Ledger) -> Vec<TransactionId> {
    let current = current_by_transaction(ledger);
    let mut queue: Vec<_> = ledger
        .transactions
        .iter()
        .filter(|t| current.get(&t.id).is_none_or(|c| c.in_review()))
        .collect();
    queue.sort_by_key(|t| std::cmp::Reverse((counting_date(t), t.id)));
    queue.into_iter().map(|t| t.id).collect()
}

/// The categories the user can file into, by group, in list order.
pub fn picker(ledger: &Ledger) -> Vec<PickerGroup> {
    let mut groups: Vec<PickerGroup> = Vec::new();
    for c in ledger.taxonomy.selectable() {
        let group = match groups.iter_mut().position(|g| g.group_id == c.group_id.0) {
            Some(i) => &mut groups[i],
            None => {
                let name = ledger
                    .taxonomy
                    .groups
                    .iter()
                    .find(|g| g.id == CategoryGroupId(c.group_id.0))
                    .map_or_else(String::new, |g| g.name.clone());
                groups.push(PickerGroup {
                    group_id: c.group_id.0,
                    name,
                    categories: vec![],
                });
                groups.last_mut().expect("just pushed")
            }
        };
        group.categories.push(PickerCategory {
            id: c.id.0,
            name: c.name.clone(),
        });
    }
    groups
}

/// A name the user typed, within its limits (length, no control or invisible
/// characters).
fn checked_name(kind: NameKind, text: &str) -> Result<String> {
    names::clean(kind, text).map_err(|e| ui_error("InvalidName", e.to_string()))
}

fn matcher_for(pattern: &str) -> Result<RuleMatcher> {
    let pattern = normalize(&checked_name(NameKind::RulePattern, pattern)?);
    if pattern.is_empty() {
        return Err(ui_error(
            "EmptyPattern",
            "the pattern has no letters or digits",
        ));
    }
    Ok(RuleMatcher {
        description: TextMatch::Contains(pattern),
        account_id: None,
        amount: None,
    })
}

fn find(ledger: &Ledger, id: i64) -> Result<&fm_core::model::Transaction> {
    ledger
        .transactions
        .iter()
        .find(|t| t.id == TransactionId(id))
        .ok_or_else(|| ui_error("NotFound", format!("no transaction {id}")))
}

/// What the learned rule would be and how many transactions it matches.
pub fn preview_rule(
    ledger: &Ledger,
    transaction: i64,
    pattern: Option<&str>,
) -> Result<RulePreview> {
    let t = find(ledger, transaction)?;
    let matcher = match pattern {
        Some(p) => Some(matcher_for(p)?),
        None => learned_matcher(&t.description),
    };
    Ok(match matcher {
        Some(m) => RulePreview {
            pattern: Some(m.description.pattern().to_string()),
            matches: categorize::correct::preview(LedgerView::from(ledger), &m),
        },
        None => RulePreview {
            pattern: None,
            matches: 0,
        },
    })
}

impl AppUnlock {
    pub fn review_queue(&self, offset: usize, limit: usize) -> Result<QueueView> {
        self.with_ledger(|open| {
            let ledger = open.ledger().load_ledger()?;
            let queue = review_queue(&ledger);
            let page: Vec<TransactionId> = queue
                .iter()
                .skip(offset)
                .take(limit.min(MAX_PAGE))
                .copied()
                .collect();
            Ok(QueueView {
                total: queue.len(),
                offset,
                transactions: transaction_views(&ledger, &page),
            })
        })
        .ok_or_else(locked)?
    }

    /// Transactions by id, for drill-downs.
    pub fn transactions(&self, ids: &[i64]) -> Result<Vec<TransactionView>> {
        let ids: Vec<TransactionId> = ids.iter().copied().map(TransactionId).collect();
        self.with_ledger(|open| {
            let ledger = open.ledger().load_ledger()?;
            Ok(transaction_views(&ledger, &ids))
        })
        .ok_or_else(locked)?
    }

    pub fn picker(&self) -> Result<Vec<PickerGroup>> {
        self.with_ledger(|open| Ok(picker(&open.ledger().load_ledger()?)))
            .ok_or_else(locked)?
    }

    /// Reading the ledger holds it; counting the matches runs after it is released.
    pub fn preview_rule(&self, transaction: i64, pattern: Option<&str>) -> Result<RulePreview> {
        let ledger = self
            .with_ledger(|open| open.ledger().load_ledger())
            .ok_or_else(locked)??;
        preview_rule(&ledger, transaction, pattern)
    }

    /// Files a transaction and/or names its merchant. Each part is one correction; with
    /// `learn`, each also becomes a rule (`pattern`, or the default learned from the
    /// description) and re-applies to the transactions it matches. Both parts are
    /// stored in one write: both or neither (`categorize::correct_all`).
    pub fn correct(
        &self,
        transaction: i64,
        bucket: Option<BucketArg>,
        merchant: Option<String>,
        learn: bool,
        pattern: Option<String>,
    ) -> Result<CorrectionView> {
        if bucket.is_none() && merchant.is_none() {
            return Err(ui_error(
                "NothingToDo",
                "choose a category or a merchant name",
            ));
        }
        let merchant = match merchant.as_deref().map(str::trim) {
            None | Some("") => None,
            Some(m) => Some(checked_name(NameKind::Merchant, m)?),
        };
        self.write_ledger(|open| {
            let unlocked = open.ledger_mut();
            let ledger = unlocked.load_ledger()?;
            let money_out = find(&ledger, transaction)?.amount.is_negative();
            let learn = match (learn, &pattern) {
                (false, _) => Learn::No,
                (true, Some(p)) => Learn::Matcher(matcher_for(p)?),
                (true, None) => Learn::Default,
            };
            let mut corrections = Vec::new();
            if let Some(b) = bucket {
                corrections.push(Correction::Bucket(b.bucket(&ledger, money_out)?));
            }
            if let Some(m) = merchant {
                corrections.push(Correction::Merchant(m));
            }
            let mut view = CorrectionView {
                rule_id: None,
                no_pattern: false,
                unpaired: vec![],
                reapplied: 0,
                rule_buckets: 0,
                rule_merchants: 0,
                rule_did_not_fit: 0,
            };
            let reports = categorize::correct_all(
                unlocked,
                TransactionId(transaction),
                &corrections,
                learn,
                Timestamp::now(),
            )?;
            for report in reports {
                view.rule_id = view.rule_id.or(report.rule.map(|r| r.0));
                view.no_pattern |= report.no_pattern;
                view.unpaired.extend(report.unpaired.iter().map(|t| t.0));
                view.reapplied = view.reapplied.max(report.history.transactions);
                view.rule_buckets += report.history.rule_buckets;
                view.rule_merchants += report.history.rule_merchants;
                view.rule_did_not_fit += report.history.rule_did_not_fit;
            }
            Ok(view)
        })
        .ok_or_else(locked)?
    }

    /// Transactions with suggestions waiting, newest first.
    pub fn suggestions(&self) -> Result<Vec<TransactionView>> {
        self.with_ledger(|open| {
            let ledger = open.ledger().load_ledger()?;
            let current = current_by_transaction(&ledger);
            let mut ids: Vec<&fm_core::model::Transaction> = ledger
                .transactions
                .iter()
                .filter(|t| {
                    current
                        .get(&t.id)
                        .is_some_and(|c| !c.suggestions.is_empty())
                })
                .collect();
            ids.sort_by_key(|t| std::cmp::Reverse((counting_date(t), t.id)));
            let ids: Vec<TransactionId> = ids.into_iter().map(|t| t.id).collect();
            Ok(transaction_views(&ledger, &ids))
        })
        .ok_or_else(locked)?
    }

    /// Approve or Reject (decision 46) the decisions the user was shown: the UI sends the
    /// ids of the bucket decisions on screen, never "whatever is waiting now", so the user
    /// only ever confirms a decision they saw (constraint 7). One that changed since the
    /// page loaded is refused by fm-core (`NotApplied`), shown as "changed, reload".
    ///
    /// Approve keeps them and counts as seen; for a transfer pair the UI sends both sides'
    /// decisions, since confirming marks only the ids given. Reject sends them back to
    /// Needs review, and that answer won't come back from the model or fixed logic;
    /// fm-core rejects both sides of a pair from either one, so only one side per pair is
    /// passed on. Returns how many decisions changed.
    pub fn review_action(&self, decisions: &[i64], approve: bool) -> Result<usize> {
        let mut ids: Vec<DecisionId> = decisions.iter().copied().map(DecisionId).collect();
        ids.sort_unstable();
        ids.dedup();
        if ids.is_empty() {
            return Ok(0);
        }
        self.write_ledger(|open| {
            let ledger = open.ledger().load_ledger()?;
            let current = current_by_transaction(&ledger);
            let by_id: std::collections::HashMap<DecisionId, &fm_core::model::Decision> =
                ledger.decisions.iter().map(|d| (d.id, d)).collect();
            // Every id given must still be the transaction's bucket decision in effect,
            // before any pair is folded to one side: otherwise fm-core, rejecting a pair
            // from its current side, would also reject the other side's *new* decision,
            // which the user never saw (review T3.3 round 2).
            for id in &ids {
                let in_effect = by_id.get(id).is_some_and(|d| {
                    current
                        .get(&d.transaction_id)
                        .and_then(|c| c.bucket_decision())
                        .is_some_and(|b| b.id == *id)
                });
                if !in_effect {
                    return Err(ui_error(
                        "Changed",
                        "this changed since the page loaded; reload to see what is current",
                    ));
                }
            }
            if !approve {
                // The applied bucket decision on the other side of a transfer pair.
                let partner = |id: &DecisionId| {
                    let d = by_id.get(id)?;
                    match &d.assignment {
                        fm_core::model::Assignment::Bucket(Bucket::Transfer {
                            counterpart: Some(other),
                        }) => current
                            .get(other)
                            .and_then(|c| c.bucket_decision())
                            .map(|o| o.id),
                        _ => None,
                    }
                };
                let mut one_side: Vec<DecisionId> = Vec::new();
                for id in &ids {
                    if partner(id).is_none_or(|p| !one_side.contains(&p)) {
                        one_side.push(*id);
                    }
                }
                ids = one_side;
            }
            let unlocked = open.ledger_mut();
            Ok(if approve {
                categorize::confirm_decisions(unlocked, &ids, Timestamp::now())?
            } else {
                categorize::reject_decisions(unlocked, &ids, Timestamp::now())?
            })
        })
        .ok_or_else(locked)?
    }

    pub fn accept_suggestion(&self, decision: i64) -> Result<Vec<i64>> {
        self.write_ledger(|open| {
            let applied = categorize::accept_suggestion(
                open.ledger_mut(),
                DecisionId(decision),
                Timestamp::now(),
            )?;
            Ok(applied.iter().map(|d| d.0).collect())
        })
        .ok_or_else(locked)?
    }

    pub fn dismiss_suggestion(&self, decision: i64) -> Result<Vec<i64>> {
        self.write_ledger(|open| {
            let dismissed = categorize::dismiss_suggestion(
                open.ledger_mut(),
                DecisionId(decision),
                Timestamp::now(),
            )?;
            Ok(dismissed.iter().map(|d| d.0).collect())
        })
        .ok_or_else(locked)?
    }
}

pub mod commands {
    use tauri::State;

    use super::{BucketArg, CorrectionView, PickerGroup, QueueView, RulePreview};
    use crate::ledger_view::{Result, TransactionView};
    use crate::unlock::AppUnlock;

    #[tauri::command(async)]
    pub fn review_queue(
        unlock: State<'_, AppUnlock>,
        offset: usize,
        limit: usize,
    ) -> Result<QueueView> {
        unlock.review_queue(offset, limit)
    }

    #[tauri::command(async)]
    pub fn transactions(
        unlock: State<'_, AppUnlock>,
        ids: Vec<i64>,
    ) -> Result<Vec<TransactionView>> {
        unlock.transactions(&ids)
    }

    #[tauri::command(async)]
    pub fn category_picker(unlock: State<'_, AppUnlock>) -> Result<Vec<PickerGroup>> {
        unlock.picker()
    }

    #[tauri::command(async)]
    pub fn preview_rule(
        unlock: State<'_, AppUnlock>,
        transaction_id: i64,
        pattern: Option<String>,
    ) -> Result<RulePreview> {
        unlock.preview_rule(transaction_id, pattern.as_deref())
    }

    #[tauri::command(async)]
    pub fn correct_transaction(
        unlock: State<'_, AppUnlock>,
        transaction_id: i64,
        bucket: Option<BucketArg>,
        merchant: Option<String>,
        learn: bool,
        pattern: Option<String>,
    ) -> Result<CorrectionView> {
        unlock.correct(transaction_id, bucket, merchant, learn, pattern)
    }

    #[tauri::command(async)]
    pub fn suggestions(unlock: State<'_, AppUnlock>) -> Result<Vec<TransactionView>> {
        unlock.suggestions()
    }

    #[tauri::command(async)]
    /// `decision_ids`: the bucket decisions the user was shown (both sides of a pair).
    pub fn approve_decisions(
        unlock: State<'_, AppUnlock>,
        decision_ids: Vec<i64>,
    ) -> Result<usize> {
        unlock.review_action(&decision_ids, true)
    }

    #[tauri::command(async)]
    /// `decision_ids`: the bucket decisions the user was shown.
    pub fn reject_decisions(unlock: State<'_, AppUnlock>, decision_ids: Vec<i64>) -> Result<usize> {
        unlock.review_action(&decision_ids, false)
    }

    #[tauri::command(async)]
    pub fn accept_suggestion(unlock: State<'_, AppUnlock>, decision_id: i64) -> Result<Vec<i64>> {
        unlock.accept_suggestion(decision_id)
    }

    #[tauri::command(async)]
    pub fn dismiss_suggestion(unlock: State<'_, AppUnlock>, decision_id: i64) -> Result<Vec<i64>> {
        unlock.dismiss_suggestion(decision_id)
    }
}

#[cfg(test)]
mod tests {
    use fm_core::model::*;
    use fm_core::storage::default_taxonomy;

    use super::*;

    fn decision(id: i64, tx: i64, bucket: Bucket, source: DecisionSource, seen: bool) -> Decision {
        Decision {
            id: DecisionId(id),
            transaction_id: TransactionId(tx),
            assignment: Assignment::Bucket(bucket),
            source,
            taxonomy_version: default_taxonomy().version,
            decided_at: Timestamp::from_unix_seconds(id),
            state: DecisionState::Applied,
            seen_at: seen.then(|| Timestamp::from_unix_seconds(100)),
            replaces: None,
        }
    }

    fn model() -> DecisionSource {
        DecisionSource::Model(ModelProvenance {
            model_id: "test-model".into(),
            gguf_sha256: Sha256Hash::of(b"model"),
            prompt_version: "1".into(),
            confidence: Confidence::from_basis_points(8_700).unwrap(),
        })
    }

    /// Decision 46: model and fixed-logic decisions the user hasn't acted on wait in the
    /// queue next to Needs review; seen ones, rules and the user's own don't.
    #[test]
    fn the_queue_holds_needs_review_and_unconfirmed_decisions() {
        let taxonomy = default_taxonomy();
        let cat = Bucket::Category {
            category_id: taxonomy.selectable().next().unwrap().id,
        };
        let account = |id, name: &str| Account {
            id: AccountId(id),
            name: name.into(),
            kind: AccountKind::Checking,
            currency: Currency::USD,
            institution: None,
            masked_number: None,
            external_id: None,
            own_account: true,
            created_at: Timestamp::from_unix_seconds(0),
        };
        let matcher = DecisionSource::Builtin {
            logic: BuiltinLogic::TransferMatcher,
            version: 1,
        };
        let transfer = |other| Bucket::Transfer {
            counterpart: Some(TransactionId(other)),
        };
        let mut ledger = Ledger {
            accounts: vec![account(1, "Checking"), account(2, "Savings")],
            imports: vec![],
            transactions: vec![],
            taxonomy,
            decisions: vec![
                decision(1, 1, Bucket::NeedsReview, needs_review_source(), false),
                decision(2, 2, cat, model(), false),
                decision(3, 3, cat, model(), true),
                decision(4, 4, cat, DecisionSource::User, false),
                decision(
                    5,
                    5,
                    cat,
                    DecisionSource::Rule { rule_id: RuleId(1) },
                    false,
                ),
                // A transfer pair found by fixed logic, not yet confirmed.
                decision(6, 7, transfer(8), matcher.clone(), false),
                decision(7, 8, transfer(7), matcher, false),
            ],
            rules: vec![],
        };
        for id in 1..=8 {
            ledger.transactions.push(Transaction {
                id: TransactionId(id),
                account_id: AccountId(if id == 8 { 2 } else { 1 }),
                source: SourceRef {
                    import_id: ImportId(1),
                    index: u32::try_from(id).unwrap(),
                    external_id: None,
                },
                status: TransactionStatus::Posted,
                posted_date: Date::from_ymd(2026, 9, u8::try_from(id).unwrap()).unwrap(),
                transaction_date: None,
                amount: Money::new(if id == 8 { 100 } else { -100 }, Currency::USD),
                original_amount: None,
                description: format!("SHOP {id}"),
                payee: None,
                memo: None,
                trn_type: None,
                check_number: None,
            });
        }
        // Transaction 6 has no bucket at all: Needs review (constraint 4).
        assert_eq!(
            review_queue(&ledger),
            [8, 7, 6, 2, 1].map(TransactionId),
            "newest first: an unconfirmed transfer pair, no bucket, unconfirmed model, Needs review"
        );
        let pair = transaction_views(&ledger, &[TransactionId(7)]);
        assert!(pair[0].awaits_confirmation);
        assert_eq!(pair[0].bucket_decision_id, Some(6), "what Approve sends");
        assert_eq!(
            pair[0].paired_with,
            Some(crate::ledger_view::PairedView {
                id: 8,
                account: "Savings".into(),
                decision_id: Some(7),
            }),
            "the UI says Approve also applies to the other side"
        );
        let placeholder = transaction_views(&ledger, &[TransactionId(1)]);
        assert!(
            !placeholder[0].awaits_confirmation,
            "a Needs-review placeholder is filed, never approved"
        );
        let views = transaction_views(&ledger, &[TransactionId(2), TransactionId(3)]);
        assert!(views[0].awaits_confirmation);
        assert_eq!(views[0].bucket_confidence.as_deref(), Some("87%"));
        assert_eq!(views[0].bucket_source, Some("Model"));
        assert!(!views[1].awaits_confirmation, "already confirmed");
    }

    fn needs_review_source() -> DecisionSource {
        fm_core::categorize::plan::needs_review_source()
    }

    /// T3.6: a correction with a category and a merchant name is one write, and the
    /// merchant name and the rule pattern are held to their limits.
    #[test]
    fn a_category_and_merchant_correction_is_one_write_within_limits() {
        use tauri::Manager;

        let dir = tempfile::tempdir().unwrap();
        let (app, w) = crate::tests::mock_app(&std::fs::canonicalize(dir.path()).unwrap());
        crate::tests::create_confirmed(&w);
        let unlock = app.state::<AppUnlock>();
        let file = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../testdata/bank-exports/data/chase/checking_2025.csv"),
        )
        .unwrap();
        unlock
            .import_file(
                &file,
                crate::importing::ImportArgs {
                    file_name: None,
                    target: crate::importing::TargetArg::NewAccount {
                        name: "Checking".into(),
                        account_kind: AccountKind::Checking,
                        currency: "USD".into(),
                    },
                    csv_currency: None,
                },
            )
            .unwrap();
        let ledger = || {
            unlock
                .with_ledger(|o| o.ledger().load_ledger().unwrap())
                .unwrap()
        };
        let l = ledger();
        let t = l
            .transactions
            .iter()
            .find(|t| t.amount.is_negative())
            .unwrap()
            .id;
        let category = l.taxonomy.categories.iter().find(|c| !c.hidden).unwrap().id;
        let bucket = || {
            Some(BucketArg::Category {
                category_id: category.0,
            })
        };

        // Refused names change nothing.
        let before = ledger();
        for (merchant, pattern) in [
            (Some("Corner\u{202E}Café".to_string()), None),
            (Some("x".repeat(81)), None),
            (
                Some("Corner Café".to_string()),
                Some("NETFLIX\nCOM".to_string()),
            ),
            (Some("Corner Café".to_string()), Some("y".repeat(101))),
        ] {
            let err = unlock
                .correct(t.0, bucket(), merchant, pattern.is_some(), pattern)
                .unwrap_err();
            assert_eq!(err.code, "InvalidName", "{}", err.message);
        }
        assert_eq!(ledger(), before, "nothing written");

        // Both parts, one write: both are in effect.
        unlock
            .correct(t.0, bucket(), Some("  Corner Café ".into()), false, None)
            .unwrap();
        let l = ledger();
        let current = crate::ledger_view::current_by_transaction(&l);
        let c = &current[&t];
        assert_eq!(
            c.bucket(),
            Some(&Bucket::Category {
                category_id: category
            })
        );
        assert_eq!(
            c.merchants.last().map(|d| &d.assignment),
            Some(&Assignment::Merchant("Corner Café".into()))
        );
        assert!(c.merchants.iter().all(|d| d.source == DecisionSource::User));
    }
}
