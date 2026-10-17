//! The hook through which a language model names a series and says what it is.
//!
//! Fixed logic finds series and knows money in from money out, but not a subscription
//! from a bill, nor "Netflix" from `NETFLIX.COM 365`. A [`Labeler`] answers both. fm-core
//! has none of its own: the model side implements the trait.

use super::{Cadence, Series, SeriesKind};
use crate::model::Money;

/// What a labeler is shown about one series.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeriesSketch<'a> {
    pub merchant_key: &'a str,
    /// Up to three distinct descriptions, as the bank wrote them.
    pub descriptions: &'a [String],
    pub cadence: Cadence,
    pub kind: SeriesKind,
    /// The latest amount.
    pub latest_amount: Money,
}

/// What a series is, in the user's terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelKind {
    Subscription,
    Bill,
    Income,
    Transfer,
    Interest,
    Other,
}

/// A labeler's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    /// A display name, e.g. "Netflix".
    pub name: String,
    pub kind: LabelKind,
}

/// Names series. Returning `None` leaves a series unlabelled.
pub trait Labeler {
    fn label(&self, series: &SeriesSketch<'_>) -> Option<Label>;
}

/// Asks `labeler` about every series and stores its answers.
pub fn label_all(series: &mut [Series], labeler: &dyn Labeler) {
    for s in series {
        let sketch = SeriesSketch {
            merchant_key: &s.merchant_key,
            descriptions: &s.descriptions,
            cadence: s.cadence,
            kind: s.kind,
            latest_amount: s.amount.latest(),
        };
        s.label = labeler.label(&sketch);
    }
}
