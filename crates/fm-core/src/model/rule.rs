use serde::{Deserialize, Serialize};

use super::{AccountId, Bucket, DecisionId, ModelError, Money, RuleId, Timestamp, TransactionId};

/// How a rule matches a transaction's text. Matching is case-insensitive; the rules
/// engine (T2.2) decides the exact normalisation (whitespace, punctuation).
///
/// Serialised as a one-key object, e.g. `{"contains": "NETFLIX"}`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextMatch {
    Equals(String),
    StartsWith(String),
    Contains(String),
}

impl TextMatch {
    pub fn pattern(&self) -> &str {
        match self {
            Self::Equals(s) | Self::StartsWith(s) | Self::Contains(s) => s,
        }
    }
}

/// How a rule matches a transaction's amount (signed, as stored).
///
/// Serialised as a one-key object, e.g. `{"exact": {"minor": -1599, "currency": "USD"}}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AmountMatch {
    Exact(Money),
    /// Inclusive at both ends; `min` and `max` in the same currency, `min <= max`.
    Between {
        min: Money,
        max: Money,
    },
}

/// What a transaction must look like for a rule to apply. All the given conditions
/// must hold.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleMatcher {
    /// Matched against the transaction's raw description.
    pub description: TextMatch,
    /// Only transactions on this account.
    pub account_id: Option<AccountId>,
    pub amount: Option<AmountMatch>,
}

/// What a rule does when it matches. At least one field is set.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleAction {
    pub bucket: Option<Bucket>,
    /// Cleaned merchant name to assign.
    pub merchant: Option<String>,
}

/// How a rule came to exist. Every rule is a user input (SPEC constraint 2).
///
/// Serialised with a `"kind"` tag.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RuleOrigin {
    /// The user wrote it.
    User,
    /// Learned from the user correcting one transaction (SPEC §2, feature 3). Deleting
    /// that transaction's data deletes the rule too (SPEC decision 28).
    Correction {
        transaction_id: TransactionId,
        decision_id: DecisionId,
    },
}

/// A categorisation or merchant clean-up rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub id: RuleId,
    pub matcher: RuleMatcher,
    pub action: RuleAction,
    pub origin: RuleOrigin,
    /// Higher wins when several rules match; ties go to the newer rule.
    pub priority: i32,
    pub enabled: bool,
    pub created_at: Timestamp,
}

impl Rule {
    /// Checks the rule can match something and does something: a non-blank pattern, an
    /// action, and a sensible amount range.
    pub fn validate(&self) -> Result<(), ModelError> {
        if self.matcher.description.pattern().trim().is_empty() {
            return Err(ModelError::InvalidRule("blank description pattern"));
        }
        if self.action.bucket.is_none() && self.action.merchant.is_none() {
            return Err(ModelError::InvalidRule("no action"));
        }
        if self
            .action
            .merchant
            .as_deref()
            .is_some_and(|m| m.trim().is_empty())
        {
            return Err(ModelError::InvalidRule("blank merchant name"));
        }
        if let Some(AmountMatch::Between { min, max }) = self.matcher.amount {
            if min.currency() != max.currency() {
                return Err(ModelError::InvalidRule("amount range mixes currencies"));
            }
            if min.minor() > max.minor() {
                return Err(ModelError::InvalidRule("amount range min is above max"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CategoryId, Currency};

    fn rule() -> Rule {
        Rule {
            id: RuleId(1),
            matcher: RuleMatcher {
                description: TextMatch::Contains("NETFLIX".into()),
                account_id: None,
                amount: Some(AmountMatch::Between {
                    min: Money::new(-2000, Currency::USD),
                    max: Money::new(-1000, Currency::USD),
                }),
            },
            action: RuleAction {
                bucket: Some(Bucket::Category {
                    category_id: CategoryId(3),
                }),
                merchant: Some("Netflix".into()),
            },
            origin: RuleOrigin::User,
            priority: 0,
            enabled: true,
            created_at: Timestamp::from_unix_seconds(0),
        }
    }

    #[test]
    fn valid_rule() {
        rule().validate().unwrap();
    }

    #[test]
    fn broken_rules_are_rejected() {
        let breakages: [fn(&mut Rule); 5] = [
            |r| r.matcher.description = TextMatch::Equals("  ".into()),
            |r| {
                r.action = RuleAction {
                    bucket: None,
                    merchant: None,
                }
            },
            |r| r.action.merchant = Some(" ".into()),
            |r| {
                r.matcher.amount = Some(AmountMatch::Between {
                    min: Money::new(-2000, Currency::USD),
                    max: Money::new(-1000, Currency::EUR),
                })
            },
            |r| {
                r.matcher.amount = Some(AmountMatch::Between {
                    min: Money::new(-1000, Currency::USD),
                    max: Money::new(-2000, Currency::USD),
                })
            },
        ];
        for (i, breakage) in breakages.into_iter().enumerate() {
            let mut r = rule();
            breakage(&mut r);
            assert!(
                matches!(r.validate(), Err(ModelError::InvalidRule(_))),
                "breakage {i}"
            );
        }
    }
}
