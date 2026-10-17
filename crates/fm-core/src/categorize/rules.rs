//! Which rules match a transaction, which one wins, and what its bucket means for this
//! transaction.

use std::cmp::Ordering;

use super::normalize::{normalize, normalized_matches, text_matches};
use crate::model::{AmountMatch, Bucket, Rule, RuleMatcher, Transaction};

/// Whether `matcher` matches `transaction`: its description (see
/// [`super::normalize::text_matches`]), its account if one is given, and its signed
/// amount if a condition is given. An amount condition in another currency never
/// matches.
pub fn matcher_matches(matcher: &RuleMatcher, transaction: &Transaction) -> bool {
    other_conditions_match(matcher, transaction)
        && text_matches(&matcher.description, &transaction.description)
}

/// The account and amount conditions.
fn other_conditions_match(matcher: &RuleMatcher, transaction: &Transaction) -> bool {
    if matcher
        .account_id
        .is_some_and(|account| account != transaction.account_id)
    {
        return false;
    }
    let amount = transaction.amount;
    match matcher.amount {
        None => true,
        Some(AmountMatch::Exact(m)) => m == amount,
        Some(AmountMatch::Between { min, max }) => {
            min.currency() == amount.currency()
                && max.currency() == amount.currency()
                && (min.minor()..=max.minor()).contains(&amount.minor())
        }
    }
}

/// Whether `rule` is in force and matches `transaction`. Disabled rules and rules that
/// fail [`Rule::validate`] never match.
pub fn rule_matches(rule: &Rule, transaction: &Transaction) -> bool {
    rule.enabled && rule.validate().is_ok() && matcher_matches(&rule.matcher, transaction)
}

/// The rules that win for one transaction, chosen separately for the bucket and for the
/// merchant name, so a merchant-only rule and a category-only rule can both apply.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Winners<'r> {
    /// The winning rule among those with a bucket action.
    pub bucket: Option<&'r Rule>,
    /// The winning rule among those with a merchant action.
    pub merchant: Option<&'r Rule>,
}

/// Picks the winning rules for `transaction`: the highest priority, then the newest
/// (`created_at`, then the higher id).
pub fn winners<'r>(rules: &'r [Rule], transaction: &Transaction) -> Winners<'r> {
    RuleSet::new(rules).winners(transaction)
}

/// The rules in force, with their patterns normalised once, for matching many
/// transactions.
#[derive(Debug, Clone)]
pub struct RuleSet<'r> {
    rules: Vec<(&'r Rule, String)>,
}

impl<'r> RuleSet<'r> {
    /// Enabled rules that pass [`Rule::validate`].
    pub fn new(rules: &'r [Rule]) -> RuleSet<'r> {
        RuleSet {
            rules: rules
                .iter()
                .filter(|r| r.enabled && r.validate().is_ok())
                .map(|r| (r, normalize(r.matcher.description.pattern())))
                .collect(),
        }
    }

    /// See [`winners`].
    pub fn winners(&self, transaction: &Transaction) -> Winners<'r> {
        let mut won = Winners::default();
        let mut text: Option<String> = None;
        for (rule, pattern) in &self.rules {
            if !other_conditions_match(&rule.matcher, transaction) {
                continue;
            }
            let text = text.get_or_insert_with(|| normalize(&transaction.description));
            if !normalized_matches(&rule.matcher.description, pattern, text) {
                continue;
            }
            if rule.action.bucket.is_some() && beats(rule, won.bucket) {
                won.bucket = Some(*rule);
            }
            if rule.action.merchant.is_some() && beats(rule, won.merchant) {
                won.merchant = Some(*rule);
            }
        }
        won
    }
}

fn beats(rule: &Rule, current: Option<&Rule>) -> bool {
    current.is_none_or(|c| precedence(rule, c) == Ordering::Greater)
}

/// Order between two rules: priority, then created_at, then id.
pub fn precedence(a: &Rule, b: &Rule) -> Ordering {
    (a.priority, a.created_at, a.id.0).cmp(&(b.priority, b.created_at, b.id.0))
}

/// What a rule's bucket means for one transaction, given which way its money moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fit {
    /// Apply this bucket.
    Apply(Bucket),
    /// The bucket can't hold this transaction (income on money out). Leave the
    /// transaction as it is.
    DoesNotFit,
}

/// Adapts a rule's bucket to the transaction's sign, so a rule written for purchases
/// also files the merchant's refunds correctly (labelling conventions 3 and 4):
///
/// | Rule says | Money out | Money in |
/// |---|---|---|
/// | Category(c) | Category(c) | Refund(c), original unknown |
/// | Refund(c) | Category(c) | Refund(c), original unknown |
/// | Income | does not fit | Income |
/// | Transfer | Transfer, counterpart unknown | Transfer, counterpart unknown |
/// | Needs review | Needs review | Needs review |
///
/// A rule never names a counterpart or an original: it can't know the other side of a
/// particular transaction. Pairs are the transfer matcher's job.
pub fn fit(bucket: Bucket, transaction: &Transaction) -> Fit {
    let money_in = transaction.amount.minor() > 0;
    Fit::Apply(match bucket {
        Bucket::Category { category_id } | Bucket::Refund { category_id, .. } => {
            if money_in {
                Bucket::Refund {
                    category_id,
                    original: None,
                }
            } else {
                Bucket::Category { category_id }
            }
        }
        Bucket::Income if !money_in => return Fit::DoesNotFit,
        Bucket::Income => Bucket::Income,
        Bucket::Transfer { .. } => Bucket::Transfer { counterpart: None },
        Bucket::NeedsReview => Bucket::NeedsReview,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        AccountId, CategoryId, Currency, Date, ImportId, Money, RuleAction, RuleId, RuleOrigin,
        SourceRef, TextMatch, Timestamp, TransactionId, TransactionStatus,
    };

    fn txn(description: &str, minor: i64) -> Transaction {
        Transaction {
            id: TransactionId(1),
            account_id: AccountId(1),
            source: SourceRef {
                import_id: ImportId(1),
                index: 0,
                external_id: None,
            },
            status: TransactionStatus::Posted,
            posted_date: Date::from_ymd(2026, 1, 2).unwrap(),
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

    fn rule(id: i64, pattern: &str, bucket: Option<Bucket>, merchant: Option<&str>) -> Rule {
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
            created_at: Timestamp::from_unix_seconds(id),
        }
    }

    fn category(id: i64) -> Option<Bucket> {
        Some(Bucket::Category {
            category_id: CategoryId(id),
        })
    }

    #[test]
    fn account_and_amount_conditions() {
        let t = txn("APPLE.COM/BILL", -999);
        let mut r = rule(1, "APPLE.COM/BILL", category(1), None);
        assert!(rule_matches(&r, &t));

        r.matcher.account_id = Some(AccountId(2));
        assert!(!rule_matches(&r, &t));
        r.matcher.account_id = Some(AccountId(1));

        r.matcher.amount = Some(AmountMatch::Exact(Money::new(-999, Currency::USD)));
        assert!(rule_matches(&r, &t));
        r.matcher.amount = Some(AmountMatch::Exact(Money::new(-299, Currency::USD)));
        assert!(!rule_matches(&r, &t));
        r.matcher.amount = Some(AmountMatch::Between {
            min: Money::new(-1000, Currency::USD),
            max: Money::new(-999, Currency::USD),
        });
        assert!(rule_matches(&r, &t));
        r.matcher.amount = Some(AmountMatch::Between {
            min: Money::new(-1000, Currency::EUR),
            max: Money::new(0, Currency::EUR),
        });
        assert!(!rule_matches(&r, &t));
    }

    #[test]
    fn disabled_and_invalid_rules_never_match() {
        let t = txn("NETFLIX.COM", -1599);
        let mut r = rule(1, "NETFLIX", category(1), None);
        r.enabled = false;
        assert!(!rule_matches(&r, &t));
        let mut r = rule(1, "NETFLIX", None, None);
        assert!(!rule_matches(&r, &t), "no action");
        r.action.merchant = Some("Netflix".into());
        assert!(rule_matches(&r, &t));
    }

    #[test]
    fn priority_then_newest_wins_separately_for_bucket_and_merchant() {
        let t = txn("UBER *EATS", -2500);
        let rules = vec![
            rule(1, "UBER", category(1), Some("Uber")),
            rule(2, "UBER EATS", category(2), None),
            rule(3, "EATS", None, Some("Uber Eats")),
        ];
        let w = winners(&rules, &t);
        assert_eq!(
            w.bucket.map(|r| r.id),
            Some(RuleId(2)),
            "newest with a bucket"
        );
        assert_eq!(
            w.merchant.map(|r| r.id),
            Some(RuleId(3)),
            "newest with a merchant"
        );

        let mut rules = rules;
        rules[0].priority = 5;
        let w = winners(&rules, &t);
        assert_eq!(w.bucket.map(|r| r.id), Some(RuleId(1)));
        assert_eq!(w.merchant.map(|r| r.id), Some(RuleId(1)));

        // Same priority and time: the higher id.
        let mut a = rule(7, "UBER", category(1), None);
        let mut b = rule(8, "UBER", category(2), None);
        a.created_at = Timestamp::from_unix_seconds(5);
        b.created_at = Timestamp::from_unix_seconds(5);
        assert_eq!(winners(&[b, a], &t).bucket.map(|r| r.id), Some(RuleId(8)));

        assert_eq!(winners(&[], &t), Winners::default());
    }

    #[test]
    fn buckets_fit_the_direction_of_money() {
        let out = txn("X", -100);
        let inn = txn("X", 100);
        let c = Bucket::Category {
            category_id: CategoryId(4),
        };
        let refund = Bucket::Refund {
            category_id: CategoryId(4),
            original: None,
        };
        assert_eq!(fit(c, &out), Fit::Apply(c));
        assert_eq!(fit(c, &inn), Fit::Apply(refund));
        let named_refund = Bucket::Refund {
            category_id: CategoryId(4),
            original: Some(TransactionId(9)),
        };
        assert_eq!(fit(named_refund, &out), Fit::Apply(c));
        assert_eq!(fit(named_refund, &inn), Fit::Apply(refund));
        assert_eq!(fit(Bucket::Income, &out), Fit::DoesNotFit);
        assert_eq!(fit(Bucket::Income, &inn), Fit::Apply(Bucket::Income));
        let paired = Bucket::Transfer {
            counterpart: Some(TransactionId(3)),
        };
        assert_eq!(
            fit(paired, &out),
            Fit::Apply(Bucket::Transfer { counterpart: None })
        );
        assert_eq!(
            fit(Bucket::NeedsReview, &inn),
            Fit::Apply(Bucket::NeedsReview)
        );
    }
}
