use std::collections::{HashMap, HashSet};
use std::fmt;

use serde::{Deserialize, Serialize};

use super::{CategoryGroupId, CategoryId, ModelError, TransactionId};

/// Which category list a decision was made against (SPEC decision 27), so evals and
/// model decisions can say what they were measured on.
///
/// `base` is the version of the shipped default list (set by the taxonomy file);
/// `revision` counts the user's edits on top of it (rename, add, merge, hide), starting
/// at 0 for the unedited list. Displayed as `<base>+r<revision>`, e.g. `1+r0`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaxonomyVersion {
    pub base: String,
    pub revision: u32,
}

impl fmt::Display for TaxonomyVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}+r{}", self.base, self.revision)
    }
}

/// A top-level group of categories (about 15 in the default list).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CategoryGroup {
    pub id: CategoryGroupId,
    /// Stable machine key from the default list (e.g. `"food"`), kept across renames.
    pub key: String,
    /// Display name; the user can rename it.
    pub name: String,
    pub hidden: bool,
}

/// A category (about 60 in the default list), always inside one group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Category {
    pub id: CategoryId,
    pub group_id: CategoryGroupId,
    /// Stable machine key (e.g. `"food.groceries"`), kept across renames. Keys from the
    /// default list are the same in every install, so evals can compare across users.
    pub key: String,
    /// Display name; the user can rename it. Renaming re-applies to past transactions
    /// for free, since they refer to the id.
    pub name: String,
    /// Hidden from pickers and from the model's list; existing decisions keep it.
    pub hidden: bool,
    /// Set when the user merged this category into another. Decisions that name this
    /// category count toward the target (see [`Taxonomy::resolve`]).
    pub merged_into: Option<CategoryId>,
}

/// The user's category list at one version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Taxonomy {
    pub version: TaxonomyVersion,
    pub groups: Vec<CategoryGroup>,
    pub categories: Vec<Category>,
}

impl Taxonomy {
    /// Checks that ids and keys are unique, every category's group exists, and every
    /// merge points at an existing category that hasn't itself been merged (no chains,
    /// no cycles, no self-merges).
    pub fn validate(&self) -> Result<(), ModelError> {
        let bad = |why: String| Err(ModelError::InvalidTaxonomy(why));

        let mut group_ids = HashSet::new();
        let mut keys = HashSet::new();
        for g in &self.groups {
            if !group_ids.insert(g.id) {
                return bad(format!("duplicate group id {}", g.id.0));
            }
            if !keys.insert(g.key.as_str()) {
                return bad(format!("duplicate key {:?}", g.key));
            }
        }

        let by_id: HashMap<CategoryId, &Category> =
            self.categories.iter().map(|c| (c.id, c)).collect();
        if by_id.len() != self.categories.len() {
            let mut seen = HashSet::new();
            let dup = self
                .categories
                .iter()
                .find(|c| !seen.insert(c.id))
                .map(|c| c.id.0);
            return bad(format!("duplicate category id {}", dup.unwrap_or_default()));
        }
        for c in &self.categories {
            if !keys.insert(c.key.as_str()) {
                return bad(format!("duplicate key {:?}", c.key));
            }
            if !group_ids.contains(&c.group_id) {
                return bad(format!(
                    "category {} is in missing group {}",
                    c.id.0, c.group_id.0
                ));
            }
            if let Some(target) = c.merged_into {
                match by_id.get(&target) {
                    None => {
                        return bad(format!(
                            "category {} merged into missing {}",
                            c.id.0, target.0
                        ));
                    }
                    Some(t) if t.merged_into.is_some() || t.id == c.id => {
                        return bad(format!(
                            "category {} merged into {}, which is itself merged",
                            c.id.0, target.0
                        ));
                    }
                    Some(_) => {}
                }
            }
        }
        Ok(())
    }

    pub fn category(&self, id: CategoryId) -> Option<&Category> {
        self.categories.iter().find(|c| c.id == id)
    }

    /// The category that `id` counts toward: itself, or the one it was merged into.
    /// `None` if `id` isn't in the list. Assumes a [validated](Taxonomy::validate) list.
    pub fn resolve(&self, id: CategoryId) -> Option<&Category> {
        let c = self.category(id)?;
        match c.merged_into {
            Some(target) => self.category(target),
            None => Some(c),
        }
    }

    /// The categories a model may choose from (SPEC constraint 3): not hidden, not
    /// merged away, and in a group that isn't hidden.
    pub fn selectable(&self) -> impl Iterator<Item = &Category> {
        let hidden_groups: HashSet<CategoryGroupId> = self
            .groups
            .iter()
            .filter(|g| g.hidden)
            .map(|g| g.id)
            .collect();
        self.categories.iter().filter(move |c| {
            !c.hidden && c.merged_into.is_none() && !hidden_groups.contains(&c.group_id)
        })
    }
}

/// The one place a transaction's money is counted (SPEC constraint 4: every transaction
/// is in exactly one bucket). A category, or one of the special buckets outside the
/// category list (SPEC decision 27).
///
/// Serialised with a `"kind"` tag, e.g. `{"kind": "category", "category_id": 12}` or
/// `{"kind": "needs_review"}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Bucket {
    /// Spending (or other money out) in a category from the user's list.
    Category { category_id: CategoryId },
    /// Money moving between the user's own accounts, including credit-card payments.
    /// Not spending. `counterpart` is the matching transaction on the other account,
    /// when it is in the ledger; matched pairs net to zero.
    Transfer { counterpart: Option<TransactionId> },
    /// Money earned (salary, interest, and so on).
    Income,
    /// Money back from a merchant. Offsets spending in `category_id`; `original` is the
    /// purchase it reverses, when known.
    Refund {
        category_id: CategoryId,
        original: Option<TransactionId>,
    },
    /// Not decided with enough confidence; waits for the user (SPEC constraint 5). Also
    /// where a model's "uncategorised" answer lands.
    NeedsReview,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn taxonomy() -> Taxonomy {
        let group = |id, key: &str, hidden| CategoryGroup {
            id: CategoryGroupId(id),
            key: key.into(),
            name: key.into(),
            hidden,
        };
        let cat = |id, group, key: &str, merged_into: Option<i64>| Category {
            id: CategoryId(id),
            group_id: CategoryGroupId(group),
            key: key.into(),
            name: key.into(),
            hidden: false,
            merged_into: merged_into.map(CategoryId),
        };
        Taxonomy {
            version: TaxonomyVersion {
                base: "1".into(),
                revision: 2,
            },
            groups: vec![group(1, "food", false), group(2, "legacy", true)],
            categories: vec![
                cat(10, 1, "food.groceries", None),
                cat(11, 1, "food.restaurants", None),
                cat(12, 1, "food.takeout", Some(11)),
                cat(20, 2, "legacy.misc", None),
            ],
        }
    }

    #[test]
    fn valid_taxonomy_resolves_merges() {
        let t = taxonomy();
        t.validate().unwrap();
        assert_eq!(t.resolve(CategoryId(12)).unwrap().id, CategoryId(11));
        assert_eq!(t.resolve(CategoryId(10)).unwrap().id, CategoryId(10));
        assert!(t.resolve(CategoryId(99)).is_none());
        assert_eq!(t.version.to_string(), "1+r2");
    }

    #[test]
    fn selectable_skips_hidden_and_merged() {
        let ids: Vec<_> = taxonomy().selectable().map(|c| c.id.0).collect();
        assert_eq!(ids, [10, 11]);
    }

    type Breakage = (&'static str, fn(&mut Taxonomy));

    #[test]
    fn broken_taxonomies_are_rejected() {
        let breakages: [Breakage; 7] = [
            ("duplicate category id", |t| {
                t.categories[1].id = CategoryId(10)
            }),
            ("duplicate key", |t| {
                t.categories[1].key = "food.groceries".into()
            }),
            ("group/category key clash", |t| {
                t.categories[0].key = "food".into()
            }),
            ("missing group", |t| {
                t.categories[0].group_id = CategoryGroupId(9)
            }),
            ("merge into missing", |t| {
                t.categories[0].merged_into = Some(CategoryId(99))
            }),
            ("merge chain", |t| {
                t.categories[0].merged_into = Some(CategoryId(12))
            }),
            ("self merge", |t| {
                t.categories[0].merged_into = Some(CategoryId(10))
            }),
        ];
        for (name, breakage) in breakages {
            let mut t = taxonomy();
            breakage(&mut t);
            assert!(
                matches!(t.validate(), Err(ModelError::InvalidTaxonomy(_))),
                "{name} not caught"
            );
        }
        let mut t = taxonomy();
        t.groups.push(t.groups[0].clone());
        assert!(t.validate().is_err(), "duplicate group not caught");
    }
}
