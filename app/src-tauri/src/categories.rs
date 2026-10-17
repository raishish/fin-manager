//! Category editing and rule management (T3.3).
//!
//! Edits are `fm_core::categories` (rename, merge, hide); a rename or merge shows on past
//! transactions at once because decisions refer to category ids. Rules come from
//! `fm_core::categorize`, as do disabling and deleting one and undoing what it decided (a
//! disabled or deleted rule stops matching; what it decided stays unless undone).

use std::collections::HashMap;

use fm_core::categories::{self, Edit, EditError};
use fm_core::categorize::{self, LedgerView, Scope, plan};
use fm_core::model::{
    AmountMatch, DecisionSource, DecisionState, Ledger, RuleId, RuleOrigin, TextMatch, Timestamp,
};
use serde::Serialize;

use crate::ledger_view::{MoneyView, Result, UiError, bucket_view, locked, ui_error};
use crate::unlock::AppUnlock;

impl From<EditError> for UiError {
    fn from(e: EditError) -> Self {
        let code = match e {
            EditError::UnknownCategory(_) | EditError::UnknownGroup(_) => "NotFound",
            EditError::InvalidName => "InvalidName",
            EditError::NameTaken => "NameTaken",
            EditError::Merged(_) => "Merged",
            EditError::MergeIntoItself => "MergeIntoItself",
            EditError::HiddenTarget(_) => "HiddenTarget",
            _ => "Storage",
        };
        ui_error(code, e.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryView {
    pub id: i64,
    pub key: String,
    pub name: String,
    pub hidden: bool,
    pub merged_into: Option<i64>,
    pub merged_into_name: Option<String>,
    /// Transactions whose money counts toward it now (merged categories included).
    pub transactions: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupView {
    pub id: i64,
    pub key: String,
    pub name: String,
    pub hidden: bool,
    pub categories: Vec<CategoryView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaxonomyView {
    /// `<base>+r<revision>`.
    pub version: String,
    pub groups: Vec<GroupView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleView {
    pub id: i64,
    /// `contains`, `startsWith` or `equals`.
    pub match_kind: &'static str,
    pub pattern: String,
    pub account: Option<String>,
    pub amount: Option<AmountView>,
    /// The bucket it files into, as labelled in lists.
    pub bucket: Option<String>,
    pub merchant: Option<String>,
    /// `you` (written) or `correction` (learned).
    pub origin: &'static str,
    pub enabled: bool,
    pub priority: i32,
    /// Applied decisions this rule made: what "undo" would re-plan.
    pub decisions: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum AmountView {
    Exact { amount: MoneyView },
    Between { min: MoneyView, max: MoneyView },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RulesView {
    pub rules: Vec<RuleView>,
    /// Transactions a matching rule's bucket doesn't fit (an income rule on money going
    /// out), so they are left as they are: "a rule didn't fit". Counted now, by a dry run
    /// of the rules over every transaction; nothing is written.
    pub rule_did_not_fit: usize,
}

pub fn taxonomy_view(ledger: &Ledger) -> TaxonomyView {
    let t = &ledger.taxonomy;
    let names: HashMap<_, _> = t
        .categories
        .iter()
        .map(|c| (c.id, c.name.clone()))
        .collect();
    TaxonomyView {
        version: t.version.to_string(),
        groups: t
            .groups
            .iter()
            .map(|g| GroupView {
                id: g.id.0,
                key: g.key.clone(),
                name: g.name.clone(),
                hidden: g.hidden,
                categories: t
                    .categories
                    .iter()
                    .filter(|c| c.group_id == g.id)
                    .map(|c| CategoryView {
                        id: c.id.0,
                        key: c.key.clone(),
                        name: c.name.clone(),
                        hidden: c.hidden,
                        merged_into: c.merged_into.map(|m| m.0),
                        merged_into_name: c.merged_into.and_then(|m| names.get(&m).cloned()),
                        transactions: if c.merged_into.is_some() {
                            0
                        } else {
                            categories::transactions_in(ledger, c.id).len()
                        },
                    })
                    .collect(),
            })
            .collect(),
    }
}

pub fn rules_view(ledger: &Ledger, now: Timestamp) -> RulesView {
    let mut decisions: HashMap<RuleId, usize> = HashMap::new();
    for d in &ledger.decisions {
        if d.state == DecisionState::Applied
            && let DecisionSource::Rule { rule_id } = d.source
        {
            *decisions.entry(rule_id).or_default() += 1;
        }
    }
    let accounts: HashMap<_, _> = ledger
        .accounts
        .iter()
        .map(|a| (a.id, a.name.clone()))
        .collect();
    let mut rules: Vec<RuleView> = ledger
        .rules
        .iter()
        .map(|r| RuleView {
            id: r.id.0,
            match_kind: match r.matcher.description {
                TextMatch::Contains(_) => "contains",
                TextMatch::StartsWith(_) => "startsWith",
                TextMatch::Equals(_) => "equals",
            },
            pattern: r.matcher.description.pattern().to_string(),
            account: r.matcher.account_id.map(|a| {
                accounts
                    .get(&a)
                    .cloned()
                    .unwrap_or_else(|| format!("account {}", a.0))
            }),
            amount: r.matcher.amount.map(|a| match a {
                AmountMatch::Exact(m) => AmountView::Exact { amount: m.into() },
                AmountMatch::Between { min, max } => AmountView::Between {
                    min: min.into(),
                    max: max.into(),
                },
            }),
            bucket: r
                .action
                .bucket
                .as_ref()
                .map(|b| bucket_view(&ledger.taxonomy, Some(b)).label),
            merchant: r.action.merchant.clone(),
            origin: match r.origin {
                RuleOrigin::User => "you",
                RuleOrigin::Correction { .. } => "correction",
            },
            enabled: r.enabled,
            priority: r.priority,
            decisions: decisions.get(&r.id).copied().unwrap_or(0),
        })
        .collect();
    // Newest first: the rule just learned is at the top.
    rules.reverse();
    let dry_run = plan::plan(LedgerView::from(ledger), Scope::All, now);
    RulesView {
        rules,
        rule_did_not_fit: dry_run.report.rule_did_not_fit,
    }
}

fn rule_exists(ledger: &Ledger, id: i64) -> Result<RuleId> {
    let id = RuleId(id);
    if ledger.rules.iter().any(|r| r.id == id) {
        Ok(id)
    } else {
        Err(ui_error("NotFound", format!("no rule {}", id.0)))
    }
}

impl AppUnlock {
    pub fn taxonomy(&self) -> Result<TaxonomyView> {
        self.with_ledger(|open| Ok(taxonomy_view(&open.ledger().load_ledger()?)))
            .ok_or_else(locked)?
    }

    pub fn edit_taxonomy(&self, edit: &Edit) -> Result<TaxonomyView> {
        self.write_ledger(|open| {
            categories::edit(open.ledger_mut(), edit)?;
            Ok(taxonomy_view(&open.ledger().load_ledger()?))
        })
        .ok_or_else(locked)?
    }

    pub fn rules(&self) -> Result<RulesView> {
        self.with_ledger(|open| Ok(rules_view(&open.ledger().load_ledger()?, Timestamp::now())))
            .ok_or_else(locked)?
    }

    /// Enables or disables a rule; with `undo`, also undoes the decisions it made.
    pub fn set_rule_enabled(&self, rule: i64, enabled: bool, undo: bool) -> Result<RulesView> {
        self.write_ledger(|open| {
            let id = rule_exists(&open.ledger().load_ledger()?, rule)?;
            let unlocked = open.ledger_mut();
            categorize::set_rule_enabled(unlocked, id, enabled)?;
            if undo && !enabled {
                categorize::undo_rule_decisions(unlocked, id, Timestamp::now())?;
            }
            Ok(rules_view(&unlocked.load_ledger()?, Timestamp::now()))
        })
        .ok_or_else(locked)?
    }

    /// Deletes a rule; with `undo`, first undoes the decisions it made.
    pub fn delete_rule(&self, rule: i64, undo: bool) -> Result<RulesView> {
        self.write_ledger(|open| {
            let id = rule_exists(&open.ledger().load_ledger()?, rule)?;
            let unlocked = open.ledger_mut();
            if undo {
                categorize::undo_rule_decisions(unlocked, id, Timestamp::now())?;
            }
            categorize::delete_rule(unlocked, id, Timestamp::now())?;
            Ok(rules_view(&unlocked.load_ledger()?, Timestamp::now()))
        })
        .ok_or_else(locked)?
    }
}

pub mod commands {
    use fm_core::categories::Edit;
    use tauri::State;

    use super::{RulesView, TaxonomyView};
    use crate::ledger_view::Result;
    use crate::unlock::AppUnlock;

    #[tauri::command(async)]
    pub fn taxonomy(unlock: State<'_, AppUnlock>) -> Result<TaxonomyView> {
        unlock.taxonomy()
    }

    /// `edit` is `fm_core::categories::Edit` as JSON, e.g.
    /// `{"kind": "merge", "from": 12, "into": 10}`.
    #[tauri::command(async)]
    pub fn edit_taxonomy(unlock: State<'_, AppUnlock>, edit: Edit) -> Result<TaxonomyView> {
        unlock.edit_taxonomy(&edit)
    }

    #[tauri::command(async)]
    pub fn rules(unlock: State<'_, AppUnlock>) -> Result<RulesView> {
        unlock.rules()
    }

    #[tauri::command(async)]
    pub fn set_rule_enabled(
        unlock: State<'_, AppUnlock>,
        rule_id: i64,
        enabled: bool,
        undo: bool,
    ) -> Result<RulesView> {
        unlock.set_rule_enabled(rule_id, enabled, undo)
    }

    #[tauri::command(async)]
    pub fn delete_rule(
        unlock: State<'_, AppUnlock>,
        rule_id: i64,
        undo: bool,
    ) -> Result<RulesView> {
        unlock.delete_rule(rule_id, undo)
    }
}
