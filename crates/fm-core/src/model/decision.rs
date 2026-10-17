use serde::{Deserialize, Serialize};

use super::{
    Bucket, DecisionId, ModelError, RuleId, Sha256Hash, TaxonomyVersion, Timestamp, TransactionId,
};

/// How sure a model was, in basis points: 0 (no confidence) to 10 000 (certain).
///
/// An integer so decisions compare and hash exactly. Serialised as a plain integer
/// and checked on deserialisation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "u16", into = "u16")]
pub struct Confidence(u16);

impl Confidence {
    pub const MAX_BASIS_POINTS: u16 = 10_000;

    pub fn from_basis_points(bp: u16) -> Result<Confidence, ModelError> {
        if bp <= Self::MAX_BASIS_POINTS {
            Ok(Confidence(bp))
        } else {
            Err(ModelError::InvalidConfidence)
        }
    }

    /// A probability in 0.0..=1.0, rounded to the nearest basis point.
    pub fn from_probability(p: f64) -> Result<Confidence, ModelError> {
        if !(0.0..=1.0).contains(&p) {
            return Err(ModelError::InvalidConfidence);
        }
        // In range (and not NaN), so the cast is exact after rounding.
        Self::from_basis_points((p * f64::from(Self::MAX_BASIS_POINTS)).round() as u16)
    }

    pub const fn basis_points(self) -> u16 {
        self.0
    }

    pub fn as_probability(self) -> f64 {
        f64::from(self.0) / f64::from(Self::MAX_BASIS_POINTS)
    }
}

impl TryFrom<u16> for Confidence {
    type Error = ModelError;

    fn try_from(bp: u16) -> Result<Confidence, ModelError> {
        Confidence::from_basis_points(bp)
    }
}

impl From<Confidence> for u16 {
    fn from(c: Confidence) -> u16 {
        c.0
    }
}

/// Exactly which model made a decision, so it can be reproduced and evaluated
/// (SPEC constraint 8, decisions 19, 20 and 33).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelProvenance {
    /// Human-readable model name and quantisation, e.g. `"qwen3-4b-instruct-q4_k_m"`.
    pub model_id: String,
    /// SHA-256 of the exact GGUF file that ran.
    pub gguf_sha256: Sha256Hash,
    /// Version of the prompt template (and grammar) used.
    pub prompt_version: String,
    pub confidence: Confidence,
}

/// Fixed logic built into the app that makes decisions without a user rule.
///
/// Serialised in `snake_case`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuiltinLogic {
    /// Transfer detection between the user's own accounts (T1.4).
    TransferMatcher,
    /// The built-in merchant name clean-up (T2.2).
    MerchantCleaner,
    /// The import pipeline (T1.3): files every new transaction under Needs review until
    /// something decides it (SPEC constraint 4).
    ImportPipeline,
}

/// Who or what made a decision (SPEC constraint 8: every decision shows its source).
///
/// The spec's three sources map to: the user → `User`; a rule → `Rule` (one the user
/// made or a correction turned into) or `Builtin` (fixed logic shipped in the app); the
/// model → `Model`.
///
/// Serialised with a `"kind"` tag, e.g. `{"kind": "rule", "rule_id": 4}`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DecisionSource {
    /// The user chose it directly. Never overridden by a model re-run (constraint 2).
    User,
    /// A user rule matched.
    Rule { rule_id: RuleId },
    /// Built-in fixed logic, at a version so a change in the logic is visible.
    Builtin { logic: BuiltinLogic, version: u32 },
    /// The local model chose it.
    Model(ModelProvenance),
}

impl DecisionSource {
    /// Whether this came from the user, directly or through a rule they own. These win
    /// over model decisions (constraint 2).
    pub const fn is_user_input(&self) -> bool {
        matches!(self, Self::User | Self::Rule { .. })
    }
}

/// What a decision says about a transaction.
///
/// Serialised as a one-key object: `{"bucket": {"kind": "income"}}` or
/// `{"merchant": "Blue Bottle Coffee"}`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Assignment {
    /// Where the money counts (a category or a special bucket).
    Bucket(Bucket),
    /// The cleaned-up merchant name. The transaction's raw description is kept.
    Merchant(String),
}

/// Where a decision stands. Decisions are never edited in place: a new decision
/// replaces an old one, and the old one becomes `Superseded`.
///
/// Serialised in `snake_case`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionState {
    /// In effect: views and answers use it.
    Applied,
    /// Proposed but not in effect, waiting for the user. A model re-run that disagrees
    /// with a decision the user has already seen lands here (SPEC constraint 7).
    Suggested,
    /// A suggestion the user turned down.
    Dismissed,
    /// Replaced by a later applied decision.
    Superseded,
}

/// One decision about one transaction, with its source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Decision {
    pub id: DecisionId,
    pub transaction_id: TransactionId,
    pub assignment: Assignment,
    pub source: DecisionSource,
    /// The category list in force when the decision was made.
    pub taxonomy_version: TaxonomyVersion,
    pub decided_at: Timestamp,
    pub state: DecisionState,
    /// When the user first saw it, if they have. Once set, a model may no longer change
    /// this decision silently (SPEC constraint 7).
    pub seen_at: Option<Timestamp>,
    /// The decision this one replaced or, for a suggestion, would replace.
    pub replaces: Option<DecisionId>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confidence_bounds() {
        assert_eq!(
            Confidence::from_basis_points(10_000)
                .unwrap()
                .basis_points(),
            10_000
        );
        assert_eq!(
            Confidence::from_basis_points(10_001),
            Err(ModelError::InvalidConfidence)
        );
        assert_eq!(
            Confidence::from_probability(0.8765).unwrap().basis_points(),
            8765
        );
        assert_eq!(
            Confidence::from_probability(1.0).unwrap().basis_points(),
            10_000
        );
        assert_eq!(Confidence::from_probability(0.0).unwrap().basis_points(), 0);
        for bad in [f64::NAN, -0.01, 1.01, f64::INFINITY] {
            assert!(Confidence::from_probability(bad).is_err(), "{bad}");
        }
        assert_eq!(
            Confidence::from_basis_points(2500)
                .unwrap()
                .as_probability(),
            0.25
        );
    }

    #[test]
    fn user_inputs() {
        assert!(DecisionSource::User.is_user_input());
        assert!(DecisionSource::Rule { rule_id: RuleId(1) }.is_user_input());
        assert!(
            !DecisionSource::Builtin {
                logic: BuiltinLogic::TransferMatcher,
                version: 1
            }
            .is_user_input()
        );
    }
}
