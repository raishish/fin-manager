//! Editing the user's category list (T3.3): rename, merge, hide.
//!
//! Every edit is a pure function from one [`Taxonomy`] to the next, so the UI, the CLI and
//! tests share one set of rules. [`edit`] applies one to the stored ledger.
//!
//! - **Rename** changes a display name. Decisions and rules refer to ids, so past
//!   transactions show the new name with nothing re-written.
//! - **Merge** files one category into another: `merged_into` is set, and everything that
//!   names the merged category (decisions, rules, refunds) counts toward the target from
//!   then on ([`Taxonomy::resolve`]). Categories already merged into the one being merged
//!   move to the target too, so no chain forms. Merging can't be undone yet.
//! - **Hide** removes a category or group from pickers and from the model's list. Totals
//!   don't change: hidden categories still count. Nothing can be merged into a hidden
//!   category (or one in a hidden group): show it first.
//!
//! An edit that changes something bumps [`TaxonomyVersion::revision`] (SPEC decision 27),
//! so a decision records which list it was made against. An edit that changes nothing
//! (the same name, already hidden) returns the list as it was, with no new revision.
//!
//! Errors name ids only, never category names.
//!
//! [`TaxonomyVersion::revision`]: crate::model::TaxonomyVersion

use std::collections::BTreeSet;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::model::{
    Assignment, Bucket, CategoryGroupId, CategoryId, DecisionState, Ledger, ModelError, Taxonomy,
    TransactionId,
};
use crate::storage::{StorageError, Unlocked};

/// Longest display name, in characters.
pub const MAX_NAME_CHARS: usize = 60;

/// One change to the category list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Edit {
    RenameCategory {
        id: CategoryId,
        name: String,
    },
    RenameGroup {
        id: CategoryGroupId,
        name: String,
    },
    SetCategoryHidden {
        id: CategoryId,
        hidden: bool,
    },
    SetGroupHidden {
        id: CategoryGroupId,
        hidden: bool,
    },
    /// Count `from` toward `into` from now on, for past and future transactions.
    Merge {
        from: CategoryId,
        into: CategoryId,
    },
}

/// Why an edit was refused. Names ids only.
#[derive(Debug)]
#[non_exhaustive]
pub enum EditError {
    UnknownCategory(CategoryId),
    UnknownGroup(CategoryGroupId),
    /// Empty after trimming, or longer than [`MAX_NAME_CHARS`].
    InvalidName,
    /// Another category in the same group (or another group) already has this name,
    /// ignoring case.
    NameTaken,
    /// The category was merged away; edit the one it was merged into.
    Merged(CategoryId),
    MergeIntoItself,
    /// The merge target is hidden (or in a hidden group): show it first, so money never
    /// moves somewhere the pickers don't offer.
    HiddenTarget(CategoryId),
    /// The edited list didn't validate (a bug in the edit, or a damaged list).
    Invalid(ModelError),
    Storage(StorageError),
}

impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownCategory(id) => write!(f, "no category {}", id.0),
            Self::UnknownGroup(id) => write!(f, "no category group {}", id.0),
            Self::InvalidName => write!(
                f,
                "a name needs 1 to {MAX_NAME_CHARS} characters, not counting spaces at the ends"
            ),
            Self::NameTaken => f.write_str("that name is already used"),
            Self::Merged(id) => write!(f, "category {} was merged into another", id.0),
            Self::MergeIntoItself => f.write_str("a category can't be merged into itself"),
            Self::HiddenTarget(id) => write!(
                f,
                "category {} is hidden; show it before merging into it",
                id.0
            ),
            Self::Invalid(e) => write!(f, "the edited category list is invalid: {e}"),
            Self::Storage(e) => write!(f, "storage: {e}"),
        }
    }
}

impl std::error::Error for EditError {}

impl From<StorageError> for EditError {
    fn from(e: StorageError) -> Self {
        Self::Storage(e)
    }
}

/// Applies `edit` to `taxonomy`. Returns the new list, with its revision bumped if
/// anything changed.
pub fn apply(taxonomy: &Taxonomy, edit: &Edit) -> Result<Taxonomy, EditError> {
    let mut next = taxonomy.clone();
    match edit {
        Edit::RenameCategory { id, name } => {
            let name = clean_name(name)?;
            let category = live_category(&next, *id)?;
            let group = category.group_id;
            let taken = next.categories.iter().any(|c| {
                c.id != *id
                    && c.group_id == group
                    && c.merged_into.is_none()
                    && c.name.to_lowercase() == name.to_lowercase()
            });
            if taken {
                return Err(EditError::NameTaken);
            }
            category_mut(&mut next, *id).name = name;
        }
        Edit::RenameGroup { id, name } => {
            let name = clean_name(name)?;
            group_index(&next, *id)?;
            let taken = next
                .groups
                .iter()
                .any(|g| g.id != *id && g.name.to_lowercase() == name.to_lowercase());
            if taken {
                return Err(EditError::NameTaken);
            }
            let i = group_index(&next, *id)?;
            next.groups[i].name = name;
        }
        Edit::SetCategoryHidden { id, hidden } => {
            live_category(&next, *id)?;
            category_mut(&mut next, *id).hidden = *hidden;
        }
        Edit::SetGroupHidden { id, hidden } => {
            let i = group_index(&next, *id)?;
            next.groups[i].hidden = *hidden;
        }
        Edit::Merge { from, into } => {
            if from == into {
                return Err(EditError::MergeIntoItself);
            }
            live_category(&next, *from)?;
            let target = live_category(&next, *into)?;
            let group_hidden = next
                .groups
                .iter()
                .any(|g| g.id == target.group_id && g.hidden);
            if target.hidden || group_hidden {
                return Err(EditError::HiddenTarget(*into));
            }
            for c in &mut next.categories {
                if c.id == *from || c.merged_into == Some(*from) {
                    c.merged_into = Some(*into);
                }
            }
        }
    }
    if next == *taxonomy {
        return Ok(next);
    }
    next.version.revision += 1;
    next.validate().map_err(EditError::Invalid)?;
    Ok(next)
}

/// Applies `edit` to the stored category list and saves it. Returns the saved list.
pub fn edit(unlocked: &mut Unlocked, edit: &Edit) -> Result<Taxonomy, EditError> {
    let current = unlocked.taxonomy()?;
    let next = apply(&current, edit)?;
    if next != current {
        unlocked.save_taxonomy(&next)?;
    }
    Ok(next)
}

/// The transactions whose money counts toward `category` today: their one applied bucket
/// decision names it, or a category merged into it, as a category or a refund. What the
/// UI shows before a merge ("N transactions move"). Ascending ids.
pub fn transactions_in(ledger: &Ledger, category: CategoryId) -> Vec<TransactionId> {
    let mut buckets: std::collections::HashMap<TransactionId, Vec<&Bucket>> =
        std::collections::HashMap::new();
    for d in &ledger.decisions {
        if d.state == DecisionState::Applied
            && let Assignment::Bucket(bucket) = &d.assignment
        {
            buckets.entry(d.transaction_id).or_default().push(bucket);
        }
    }
    let ids: BTreeSet<TransactionId> = buckets
        .into_iter()
        .filter_map(|(t, b)| match b.as_slice() {
            // Zero or several applied buckets count as Needs review (SPEC constraint 4).
            [Bucket::Category { category_id } | Bucket::Refund { category_id, .. }] => ledger
                .taxonomy
                .resolve(*category_id)
                .filter(|c| c.id == category)
                .map(|_| t),
            _ => None,
        })
        .collect();
    ids.into_iter().collect()
}

fn clean_name(name: &str) -> Result<String, EditError> {
    let name = name.trim();
    let chars = name.chars().count();
    if chars == 0 || chars > MAX_NAME_CHARS || name.chars().any(char::is_control) {
        return Err(EditError::InvalidName);
    }
    Ok(name.to_string())
}

/// The category, if it exists and hasn't been merged away.
fn live_category(
    taxonomy: &Taxonomy,
    id: CategoryId,
) -> Result<&crate::model::Category, EditError> {
    let c = taxonomy
        .category(id)
        .ok_or(EditError::UnknownCategory(id))?;
    if c.merged_into.is_some() {
        return Err(EditError::Merged(id));
    }
    Ok(c)
}

fn category_mut(taxonomy: &mut Taxonomy, id: CategoryId) -> &mut crate::model::Category {
    taxonomy
        .categories
        .iter_mut()
        .find(|c| c.id == id)
        .expect("checked by live_category")
}

fn group_index(taxonomy: &Taxonomy, id: CategoryGroupId) -> Result<usize, EditError> {
    taxonomy
        .groups
        .iter()
        .position(|g| g.id == id)
        .ok_or(EditError::UnknownGroup(id))
}

#[cfg(test)]
mod tests;
