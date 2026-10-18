//! The computed values an explanation is about, as plain data: this crate never sees
//! fm-core's types, so the caller (the Ask flow) converts its answers into these.

/// An exact decimal: `mantissa / 10^scale`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Dec {
    pub(crate) mantissa: i128,
    pub(crate) scale: u32,
}

impl Dec {
    pub(crate) fn new(mantissa: i128, scale: u32) -> Dec {
        Dec { mantissa, scale }
    }

    pub(crate) fn abs(self) -> Dec {
        Dec::new(self.mantissa.abs(), self.scale)
    }

    /// The same value with `scale` decimals, if that loses nothing.
    pub(crate) fn rescaled(self, scale: u32) -> Option<i128> {
        if scale >= self.scale {
            self.mantissa
                .checked_mul(10i128.checked_pow(scale - self.scale)?)
        } else {
            let div = 10i128.checked_pow(self.scale - scale)?;
            (self.mantissa % div == 0).then_some(self.mantissa / div)
        }
    }

    /// Rounded to `scale` decimals, halves away from zero.
    pub(crate) fn rounded(self, scale: u32) -> Option<i128> {
        if scale >= self.scale {
            return self.rescaled(scale);
        }
        let div = 10i128.checked_pow(self.scale - scale)?;
        let (q, r) = (self.mantissa / div, self.mantissa % div);
        Some(if r.abs() * 2 >= div {
            q + self.mantissa.signum()
        } else {
            q
        })
    }

    /// The value divided by `10^power` (exactly: the scale grows).
    pub(crate) fn shifted(self, power: u32) -> Dec {
        Dec::new(self.mantissa, self.scale + power)
    }

    /// `self - other`, exactly.
    pub(crate) fn minus(self, other: Dec) -> Option<Dec> {
        let scale = self.scale.max(other.scale);
        Some(Dec::new(
            self.rescaled(scale)?.checked_sub(other.rescaled(scale)?)?,
            scale,
        ))
    }

    pub(crate) fn plus(self, other: Dec) -> Option<Dec> {
        let scale = self.scale.max(other.scale);
        Some(Dec::new(
            self.rescaled(scale)?.checked_add(other.rescaled(scale)?)?,
            scale,
        ))
    }

    pub(crate) fn as_f64(self) -> f64 {
        self.mantissa as f64 / 10f64.powi(self.scale as i32)
    }
}

/// An amount of money.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Money {
    /// Minor units: cents for USD, yen for JPY.
    pub minor: i64,
    /// ISO 4217 code, e.g. `USD`.
    pub currency: String,
    /// Digits after the decimal point in this currency: 2 for USD, 0 for JPY.
    pub minor_digits: u8,
}

impl Money {
    pub(crate) fn dec(&self) -> Dec {
        Dec::new(i128::from(self.minor), u32::from(self.minor_digits))
    }
}

/// What a fact's value is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Money(Money),
    Count(i64),
    /// Basis points: 1234 is 12.34%.
    Percent {
        basis_points: i64,
    },
    Date {
        year: i32,
        month: u8,
        day: u8,
    },
    Month {
        year: i32,
        month: u8,
    },
    Year(i32),
    /// A name or label (a merchant, a category). Numbers inside it are part of it.
    Text(String),
    Texts(Vec<String>),
}

/// One computed value the explanation may state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fact {
    /// What it is, e.g. `total` or `q4_2025`.
    pub name: String,
    pub value: Value,
    /// How many transactions it is computed from, if that may be stated too.
    pub evidence: Option<u64>,
}

/// A number the explanation may state that is computed from the facts, not one of them.
/// Nothing derived is allowed unless the caller lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Derivation {
    /// `a - b`, of two money facts (same currency) or two counts.
    Difference(String, String),
    /// The sum of money facts (one currency) or of counts.
    Sum(Vec<String>),
    /// `a / b` as a percent.
    Ratio(String, String),
    /// `(a - b) / b` as a percent.
    PercentChange(String, String),
}
