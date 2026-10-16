use serde::{Deserialize, Serialize};

use super::{AccountId, Currency, Timestamp};

/// What kind of account this is. Every kind exists from day one (SPEC §2) even though
/// the MVP imports only checking, savings, credit card and cash accounts
/// (see [`AccountKind::in_mvp`]).
///
/// Serialised in `snake_case`, e.g. `"credit_card"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountKind {
    Checking,
    Savings,
    CreditCard,
    /// Physical cash or a cash-like wallet the user tracks by hand.
    Cash,
    /// Personal, auto or student loan.
    Loan,
    Mortgage,
    /// Taxable investment account.
    Brokerage,
    /// Tax-advantaged retirement account (401(k), IRA and similar).
    Retirement,
    Other,
}

impl AccountKind {
    /// Whether the balance is money the user owes (credit card, loan, mortgage).
    pub const fn is_liability(self) -> bool {
        matches!(self, Self::CreditCard | Self::Loan | Self::Mortgage)
    }

    /// Whether the MVP imports and analyses this kind of account.
    pub const fn in_mvp(self) -> bool {
        matches!(
            self,
            Self::Checking | Self::Savings | Self::CreditCard | Self::Cash
        )
    }
}

/// An account the user has, at a bank, card issuer, broker or lender.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Account {
    pub id: AccountId,
    /// Display name; the user can change it.
    pub name: String,
    pub kind: AccountKind,
    /// The currency the account is held in. Its transactions' `amount`s are in it.
    pub currency: Currency,
    /// Bank or issuer name as the user or the source gives it.
    pub institution: Option<String>,
    /// The last few digits only (e.g. `"1234"`), for telling accounts apart. Never the
    /// full number.
    pub masked_number: Option<String>,
    /// The source's own id for this account (SimpleFIN account id, OFX `ACCTID`), used to
    /// match later imports to the same account. Can be a full account number; it lives
    /// only in the encrypted database.
    pub external_id: Option<String>,
    /// The user's designation that this is their own account, so money moving between
    /// it and another own account is a transfer, not spending. A user input: never
    /// changed by a model (SPEC constraint 2).
    pub own_account: bool,
    pub created_at: Timestamp,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn liabilities_and_mvp_kinds() {
        assert!(AccountKind::CreditCard.is_liability());
        assert!(AccountKind::Mortgage.is_liability());
        assert!(!AccountKind::Checking.is_liability());
        assert!(AccountKind::CreditCard.in_mvp());
        assert!(!AccountKind::Brokerage.in_mvp());
    }
}
