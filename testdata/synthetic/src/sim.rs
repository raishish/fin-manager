//! The simulation engine.
//!
//! A persona script (see `personas/`) opens accounts and calls the primitives here
//! (purchases, bills, paychecks, transfers, habits, recurring series). Each call adds
//! *drafts*: transactions with their ground truth attached. [`Sim::finish`] then runs
//! the passes that depend on everything else (refunds, card statements and payments,
//! savings interest, pending items at the end of the period), assigns ids in posting
//! order and splits the drafts into the ledger and the truth file.

use std::collections::BTreeMap;

use fm_core::model::{
    Account, AccountId, AccountKind, Currency, Date, ImportId, ImportSource, Ledger, Money,
    RawImport, SourceRef, Taxonomy, Timestamp, Transaction, TransactionId, TransactionStatus,
};

use crate::cal::{
    add_days, business_on_or_after, business_on_or_before, day_of_month, months, nth_weekday,
    weekday,
};
use crate::catalog::{
    DELIVERY_RESTAURANTS, FAKE_FIRST_NAMES, FAKE_SURNAMES, Label, Merchant, merchant,
    shares_descriptor,
};
use crate::describe::{self, AcctView, Style};
use crate::rng::Rng;
use crate::truth::{
    AmountTruth, PairKind, PendingTruth, PersonaTruth, SeriesTruth, Step, TransferPair,
    TruthBucket, TxTruth,
};

/// Bumped whenever a change to the generator changes its output for the same seed.
pub const GENERATOR_VERSION: u32 = 1;

pub fn usd() -> Currency {
    Currency::new("USD").expect("USD is a currency")
}

/// Index of an account within one persona.
pub type Acct = usize;

/// Truth bucket while drafting; links are draft indices until ids are assigned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DraftBucket {
    Category(&'static str),
    Transfer(Option<usize>),
    Income,
    Refund(&'static str, Option<usize>),
}

#[derive(Debug, Clone)]
pub struct Draft {
    pub account: Acct,
    /// When it happened (purchase date, transfer date).
    pub date: Date,
    /// When it posted to the account.
    pub posted: Date,
    /// Whether the bank reports a separate transaction date (card and debit purchases).
    pub has_txn_date: bool,
    /// Signed cents in the account's currency.
    pub amount: i64,
    pub original: Option<Money>,
    pub description: String,
    pub bucket: DraftBucket,
    pub merchant: Option<String>,
    pub recurring: Option<String>,
    pub one_off: bool,
    pub tags: Vec<&'static str>,
    /// While pending: the amount and description the bank showed before it posted.
    pub pending: Option<(i64, String)>,
    /// For card purchases: the processor descriptor and location, to render a refund.
    pub core: Option<(String, String)>,
    pub pair: Option<PairKind>,
}

/// Credit card terms that drive statements, payments and interest.
#[derive(Debug, Clone)]
pub struct CardTerms {
    pub close_day: u8,
    pub pay_from: Acct,
    /// Days from statement close to the payment date.
    pub due_days: i32,
    /// Days the payment takes to reach the card: (min, max).
    pub lag: (i32, i32),
    /// Statement months the holder pays only part of the balance; full otherwise.
    pub partial_months: Vec<(i32, u8)>,
    /// Share of the balance paid in a partial month, in percent.
    pub partial_pct: i64,
    /// Annual percentage rate in basis points.
    pub apr_bp: i64,
    /// Foreign transaction fee in basis points (0 for none).
    pub foreign_fee_bp: i64,
}

#[derive(Debug, Clone)]
pub struct SavingsTerms {
    pub opening: i64,
    /// (from date, APY in basis points).
    pub apy: Vec<(Date, i64)>,
}

#[derive(Debug, Clone)]
pub struct AcctSpec {
    pub key: &'static str,
    pub kind: AccountKind,
    pub institution: &'static str,
    pub view: AcctView,
    pub card: Option<CardTerms>,
    pub savings: Option<SavingsTerms>,
    pub own_account: bool,
}

#[derive(Debug, Clone, Copy)]
pub enum Cadence {
    Weekly(usize),
    /// Every 14 days from this date.
    Biweekly(Date),
    SemiMonthly(u8, u8),
    Monthly(u8),
    /// The nth (1-based) weekday of each month, Monday = 0.
    NthWeekday(usize, u8),
    /// Day `d` of the listed months (quarterly, semiannual, annual).
    Months(&'static [u8], u8),
}

impl Cadence {
    pub fn label(self) -> &'static str {
        match self {
            Cadence::Weekly(_) => "weekly",
            Cadence::Biweekly(_) => "biweekly",
            Cadence::SemiMonthly(..) => "semimonthly",
            Cadence::Monthly(_) | Cadence::NthWeekday(..) => "monthly",
            Cadence::Months(list, _) => match list.len() {
                4 => "quarterly",
                2 => "semiannual",
                1 => "annual",
                _ => "monthly",
            },
        }
    }

    pub fn dates(self, from: Date, to: Date) -> Vec<Date> {
        let mut out = Vec::new();
        match self {
            Cadence::Weekly(wd) => {
                let mut d = add_days(from, ((wd + 7 - weekday(from)) % 7) as i32);
                while d <= to {
                    out.push(d);
                    d = add_days(d, 7);
                }
            }
            Cadence::Biweekly(anchor) => {
                let mut d = anchor;
                while d <= to {
                    if d >= from {
                        out.push(d);
                    }
                    d = add_days(d, 14);
                }
            }
            Cadence::SemiMonthly(a, b) => {
                for (y, m) in months(from, to) {
                    out.push(day_of_month(y, m, a));
                    out.push(day_of_month(y, m, b));
                }
            }
            Cadence::Monthly(day) => {
                for (y, m) in months(from, to) {
                    out.push(day_of_month(y, m, day));
                }
            }
            Cadence::NthWeekday(wd, n) => {
                for (y, m) in months(from, to) {
                    out.push(nth_weekday(y, m, wd, n));
                }
            }
            Cadence::Months(list, day) => {
                for (y, m) in months(from, to) {
                    if list.contains(&m) {
                        out.push(day_of_month(y, m, day));
                    }
                }
            }
        }
        out.retain(|d| (from..=to).contains(d));
        out
    }
}

/// How a recurring amount varies.
#[derive(Debug, Clone)]
pub enum Amount {
    /// A fixed amount that changes at the given dates: `(date, cents)`, first entry's
    /// date ignored.
    Steps(Vec<(Date, i64)>),
    /// Varies between two values.
    Range(i64, i64),
    /// Varies with the season: `lo + (hi - lo) * factor[month] / 100`, ±6%.
    Seasonal(i64, i64, [i64; 12]),
}

impl Amount {
    pub fn fixed(cents: i64) -> Amount {
        Amount::Steps(vec![(Date::MIN, cents)])
    }

    fn is_fixed(&self) -> bool {
        matches!(self, Amount::Steps(_))
    }

    pub fn at(&self, d: Date, rng: &mut Rng) -> i64 {
        match self {
            Amount::Steps(steps) => steps
                .iter()
                .enumerate()
                .filter(|(i, (from, _))| *i == 0 || *from <= d)
                .map(|(_, (_, c))| *c)
                .next_back()
                .expect("at least one step"),
            Amount::Range(lo, hi) => rng.around(*lo, *hi),
            Amount::Seasonal(lo, hi, factor) => {
                let base = lo + (hi - lo) * factor[usize::from(d.month()) - 1] / 100;
                base + rng.range(-base * 6 / 100, base * 6 / 100)
            }
        }
    }
}

/// How a recurring payment is made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    /// Charged to a card (or a debit card on checking).
    Card,
    /// A bank debit or deposit.
    Ach,
    /// A paper check to the merchant.
    Check,
}

/// A recurring series: subscription, bill, paycheck or scheduled transfer.
#[derive(Debug, Clone)]
pub struct Recurring {
    pub id: &'static str,
    /// Truth kind: `subscription`, `bill`, `income`, `transfer` or `interest`.
    pub kind: &'static str,
    pub acct: Acct,
    pub merchant: &'static str,
    pub cadence: Cadence,
    pub from: Option<Date>,
    pub to: Option<Date>,
    pub amount: Amount,
    pub via: Via,
    /// Only these months (1–12), e.g. a school-year program.
    pub only_months: Option<&'static [u8]>,
    /// Money comes in (paychecks, withdrawals from an untracked own account).
    pub inflow: bool,
}

impl Recurring {
    pub fn new(
        id: &'static str,
        kind: &'static str,
        acct: Acct,
        merchant_key: &'static str,
        cadence: Cadence,
        amount: Amount,
        via: Via,
    ) -> Recurring {
        Recurring {
            id,
            kind,
            acct,
            merchant: merchant_key,
            cadence,
            from: None,
            to: None,
            amount,
            via,
            only_months: None,
            inflow: merchant(merchant_key).label == Label::Income,
        }
    }

    pub fn from(mut self, d: Date) -> Self {
        self.from = Some(d);
        self
    }

    pub fn to(mut self, d: Date) -> Self {
        self.to = Some(d);
        self
    }

    pub fn only(mut self, months: &'static [u8]) -> Self {
        self.only_months = Some(months);
        self
    }

    pub fn inflow(mut self) -> Self {
        self.inflow = true;
        self
    }
}

/// Everyday spending that happens at some rate, at merchants picked by weight.
#[derive(Debug, Clone)]
pub struct Habit {
    pub label: &'static str,
    pub accounts: Vec<(Acct, u32)>,
    pub merchants: Vec<(&'static str, u32)>,
    pub per_week: f64,
    /// Relative weight of each weekday, Monday first.
    pub days: [u32; 7],
    /// Price scale in percent of the merchant's typical range.
    pub scale_pct: i64,
    /// Rate multiplier per month, in percent.
    pub season: Option<[u32; 12]>,
}

pub const EVERY_DAY: [u32; 7] = [1, 1, 1, 1, 1, 1, 1];
pub const WEEKDAYS: [u32; 7] = [1, 1, 1, 1, 1, 0, 0];
pub const WEEKENDS: [u32; 7] = [0, 0, 0, 0, 1, 2, 2];
pub const EVENINGS_OUT: [u32; 7] = [1, 1, 2, 3, 5, 5, 2];
/// Holiday shopping: busier in November and December.
pub const HOLIDAY_SEASON: [u32; 12] = [80, 85, 95, 95, 100, 100, 100, 105, 95, 100, 135, 175];
/// Home and garden work: spring and summer.
pub const GARDEN_SEASON: [u32; 12] = [40, 50, 110, 160, 170, 160, 140, 120, 100, 80, 50, 50];

impl Habit {
    pub fn new(
        label: &'static str,
        accounts: &[(Acct, u32)],
        merchants: &[(&'static str, u32)],
        per_week: f64,
    ) -> Habit {
        Habit {
            label,
            accounts: accounts.to_vec(),
            merchants: merchants.to_vec(),
            per_week,
            days: EVERY_DAY,
            scale_pct: 100,
            season: None,
        }
    }

    pub fn days(mut self, days: [u32; 7]) -> Self {
        self.days = days;
        self
    }

    pub fn scale(mut self, pct: i64) -> Self {
        self.scale_pct = pct;
        self
    }

    pub fn season(mut self, season: [u32; 12]) -> Self {
        self.season = Some(season);
        self
    }
}

struct SeriesMeta {
    id: String,
    kind: &'static str,
    merchant: String,
    account: Acct,
    cadence: &'static str,
    fixed: bool,
    ends: bool,
}

/// One persona's simulation.
pub struct Sim {
    pub key: &'static str,
    pub seed: u64,
    pub start: Date,
    pub end: Date,
    /// The persona's city, as statements print it: `"SAN FRANCISCO CA"`.
    pub home: &'static str,
    /// Nearby towns where some purchases happen.
    pub nearby: Vec<&'static str>,
    pub holder: String,
    accounts: Vec<AcctSpec>,
    drafts: Vec<Draft>,
    series: Vec<SeriesMeta>,
    stores: BTreeMap<(&'static str, usize), String>,
    checks: BTreeMap<Acct, u32>,
    /// Description noise: reference numbers, descriptor variants, store numbers.
    noise: Rng,
    /// Timing noise: posting lags.
    timing: Rng,
}

impl Sim {
    pub fn new(
        key: &'static str,
        seed: u64,
        start: Date,
        end: Date,
        home: &'static str,
        holder: &str,
    ) -> Sim {
        Sim {
            key,
            seed,
            start,
            end,
            home,
            nearby: Vec::new(),
            holder: holder.to_uppercase(),
            accounts: Vec::new(),
            drafts: Vec::new(),
            series: Vec::new(),
            stores: BTreeMap::new(),
            checks: BTreeMap::new(),
            noise: Rng::derive(seed, &format!("{key}/noise")),
            timing: Rng::derive(seed, &format!("{key}/timing")),
        }
    }

    /// An independent random stream for one part of a persona script.
    pub fn rng(&self, label: &str) -> Rng {
        Rng::derive(self.seed, &format!("{}/{label}", self.key))
    }

    /// Opens an account the user owns.
    #[allow(clippy::too_many_arguments)]
    pub fn account(
        &mut self,
        key: &'static str,
        name: &str,
        kind: AccountKind,
        institution: &'static str,
        last4: &str,
        style: Style,
    ) -> Acct {
        self.accounts.push(AcctSpec {
            key,
            kind,
            institution,
            view: AcctView {
                style,
                name: name.to_string(),
                last4: last4.to_string(),
                holder: self.holder.clone(),
            },
            card: None,
            savings: None,
            own_account: true,
        });
        self.accounts.len() - 1
    }

    pub fn card_terms(&mut self, acct: Acct, terms: CardTerms) {
        self.accounts[acct].card = Some(terms);
    }

    pub fn savings_terms(&mut self, acct: Acct, terms: SavingsTerms) {
        self.accounts[acct].savings = Some(terms);
    }

    pub fn draft(&mut self, i: usize) -> &mut Draft {
        &mut self.drafts[i]
    }

    fn push(&mut self, d: Draft) -> usize {
        assert!(
            d.date <= self.end && d.date >= self.start,
            "{} draft outside the period: {}",
            self.key,
            d.date
        );
        self.drafts.push(d);
        self.drafts.len() - 1
    }

    fn blank(
        &self,
        account: Acct,
        date: Date,
        amount: i64,
        description: String,
        bucket: DraftBucket,
    ) -> Draft {
        Draft {
            account,
            date,
            posted: date,
            has_txn_date: false,
            amount,
            original: None,
            description,
            bucket,
            merchant: None,
            recurring: None,
            one_off: false,
            tags: Vec::new(),
            pending: None,
            core: None,
            pair: None,
        }
    }

    /// The persona's usual store number for a merchant (stable), or occasionally another
    /// branch.
    fn store(&mut self, m: &'static str, digits: usize) -> String {
        if self.noise.chance(0.12) {
            return self.noise.digits(digits);
        }
        let seed = self.seed;
        let key = self.key;
        self.stores
            .entry((m, digits))
            .or_insert_with(|| Rng::derive(seed, &format!("{key}/store/{m}")).digits(digits))
            .clone()
    }

    /// Fills a descriptor's placeholders.
    fn render(&mut self, m: &Merchant) -> String {
        let template = *self.noise.pick(m.descriptors);
        let mut out = String::new();
        let mut rest = template;
        while let Some(open) = rest.find('{') {
            out.push_str(&rest[..open]);
            let close = rest[open..].find('}').expect("closed placeholder") + open;
            let ph = &rest[open + 1..close];
            let n = |s: &str| s[2..].parse::<usize>().expect("placeholder width");
            let city = self.home.rsplit_once(' ').map_or(self.home, |(c, _)| c);
            match ph {
                "dd" => out.push_str(self.noise.pick(DELIVERY_RESTAURANTS)),
                "city" => out.push_str(city),
                p if p.starts_with("s:") => {
                    let s = self.store(m.key, n(p));
                    out.push_str(&s);
                }
                p if p.starts_with("r:") => out.push_str(&self.noise.alnum(n(p))),
                p if p.starts_with("d:") => out.push_str(&self.noise.digits(n(p))),
                p => panic!("unknown placeholder {p}"),
            }
            rest = &rest[close + 1..];
        }
        out.push_str(rest);
        out
    }

    fn bucket_for(label: Label, inflow: bool) -> DraftBucket {
        match (label, inflow) {
            (Label::Cat(k), false) => DraftBucket::Category(k),
            (Label::Cat(k), true) => DraftBucket::Refund(k, None),
            (Label::Income, _) => DraftBucket::Income,
            (Label::Transfer, _) => DraftBucket::Transfer(None),
        }
    }

    /// A card purchase (or debit-card purchase on checking) of `cents` at merchant `m`.
    pub fn buy(&mut self, acct: Acct, m: &'static str, date: Date, cents: i64) -> usize {
        let loc = if merchant(m).loc.is_none() && !self.nearby.is_empty() && self.noise.chance(0.15)
        {
            *self.noise.pick(&self.nearby)
        } else {
            merchant(m).loc.unwrap_or(self.home)
        };
        self.buy_at(acct, m, date, cents, loc)
    }

    /// A purchase at a given location (travel).
    pub fn buy_at(
        &mut self,
        acct: Acct,
        m: &'static str,
        date: Date,
        cents: i64,
        loc: &str,
    ) -> usize {
        let mm = merchant(m);
        let core = self.render(mm);
        let view = self.accounts[acct].view.clone();
        let description = describe::purchase(&view, &core, loc, date, false, &mut self.noise);
        let lag = if view.style.is_card() {
            self.timing.range(1, 2)
        } else {
            self.timing.range(0, 2)
        } as i32;
        let mut d = self.blank(
            acct,
            date,
            -cents,
            description,
            Self::bucket_for(mm.label, false),
        );
        d.posted = add_days(date, lag);
        d.has_txn_date = true;
        d.merchant = Some(mm.name.to_string());
        d.core = Some((core.clone(), loc.to_string()));
        if shares_descriptor(mm) {
            d.tags.push("ambiguous_descriptor");
        }
        if let Label::Cat(k) = mm.label {
            if (k == "food.restaurants" || k == "food.bars") && self.timing.chance(0.7) {
                // Restaurants authorise the bill before the tip.
                let tip_pct = self.timing.range(15, 22);
                d.pending = Some((-(cents * 100 / (100 + tip_pct)), d.description.clone()));
                d.tags.push("tip_adjusted");
            } else if k == "transport.fuel" && self.timing.chance(0.4) {
                // Pay-at-pump holds a fixed amount until the sale settles.
                d.pending = Some((-17_500, d.description.clone()));
                d.tags.push("pump_hold");
            }
        }
        self.push(d)
    }

    /// A purchase abroad: `foreign` minor units of `currency`, converted at `rate_micros`
    /// (cents per foreign minor unit, times a million). Adds the card's foreign
    /// transaction fee as its own line, as US issuers do.
    pub fn buy_foreign(
        &mut self,
        acct: Acct,
        m: &'static str,
        date: Date,
        foreign: i64,
        currency: &str,
        rate_micros: i64,
    ) -> usize {
        let cents = (foreign * rate_micros + 500_000) / 1_000_000;
        let loc = merchant(m).loc.unwrap_or(self.home);
        let i = self.buy_at(acct, m, date, cents, loc);
        let cur = Currency::new(currency).expect("valid currency");
        self.drafts[i].original = Some(Money::new(-foreign, cur));
        self.drafts[i].tags.push("foreign_currency");
        let fee_bp = self.accounts[acct]
            .card
            .as_ref()
            .map_or(0, |c| c.foreign_fee_bp);
        if fee_bp > 0 {
            let fee = (cents * fee_bp + 5_000) / 10_000;
            let view = self.accounts[acct].view.clone();
            let mut f = self.blank(
                acct,
                date,
                -fee,
                describe::foreign_fee(&view).to_string(),
                DraftBucket::Category("financial.bank_fees"),
            );
            f.posted = self.drafts[i].posted;
            f.merchant = Some(self.accounts[acct].institution.to_string());
            f.tags.push("foreign_fee");
            self.push(f);
        }
        i
    }

    /// A bank debit (`cents > 0` leaves the account) or deposit (`cents < 0` arrives),
    /// using merchant `m`'s ACH identity.
    #[track_caller]
    pub fn ach(&mut self, acct: Acct, m: &'static str, date: Date, cents_out: i64) -> usize {
        let mm = merchant(m);
        let Some((company, entry)) = mm.ach else {
            panic!("merchant has no ACH identity; see catalog::MERCHANTS")
        };
        let inflow = cents_out < 0;
        let view = self.accounts[acct].view.clone();
        let description = describe::ach(&view, company, entry, inflow, date, &mut self.noise);
        let mut d = self.blank(
            acct,
            date,
            -cents_out,
            description,
            Self::bucket_for(mm.label, inflow),
        );
        d.merchant = Some(mm.name.to_string());
        if mm.label == Label::Transfer {
            d.tags.push("untracked_counterpart");
        }
        self.push(d)
    }

    /// A paper check for `cents`, cleared a few days after it was written. Only the
    /// check number appears on the statement; the payee is in the truth only.
    pub fn check(
        &mut self,
        acct: Acct,
        date: Date,
        cents: i64,
        payee: &str,
        category: &'static str,
    ) -> usize {
        let base = 1001
            + Rng::derive(self.seed, &format!("{}/checkbook/{acct}", self.key)).below(400) as u32;
        let n = *self
            .checks
            .entry(acct)
            .and_modify(|n| *n += 1)
            .or_insert(base);
        let view = self.accounts[acct].view.clone();
        let lag = self.timing.range(2, 9) as i32;
        let posted = business_on_or_after(add_days(date, lag)).min(self.end);
        let mut d = self.blank(
            acct,
            date,
            -cents,
            describe::check(&view, n),
            DraftBucket::Category(category),
        );
        d.posted = posted;
        d.merchant = Some(payee.to_string());
        d.tags.push("check");
        self.push(d)
    }

    /// A Zelle payment to (`out`) or from a person, in a given truth bucket.
    pub fn zelle(
        &mut self,
        acct: Acct,
        date: Date,
        cents: i64,
        person: (&str, &str),
        out: bool,
        bucket: DraftBucket,
    ) -> usize {
        let view = self.accounts[acct].view.clone();
        let description = describe::zelle(&view, person.0, person.1, out, date, &mut self.noise);
        let mut d = self.blank(
            acct,
            date,
            if out { -cents } else { cents },
            description,
            bucket,
        );
        d.merchant = Some(format!("{} {}", person.0, person.1));
        d.tags.push("p2p");
        self.push(d)
    }

    /// A Venmo payment from the bank account. The statement doesn't name the person.
    pub fn venmo(
        &mut self,
        acct: Acct,
        date: Date,
        cents: i64,
        person: (&str, &str),
        category: &'static str,
    ) -> usize {
        let view = self.accounts[acct].view.clone();
        let description = describe::venmo(&view, date, &mut self.noise);
        let mut d = self.blank(
            acct,
            date,
            -cents,
            description,
            DraftBucket::Category(category),
        );
        d.posted =
            business_on_or_after(add_days(date, self.timing.range(0, 1) as i32)).min(self.end);
        d.merchant = Some(format!("{} {}", person.0, person.1));
        d.tags.extend(["p2p", "no_counterparty_name"]);
        self.push(d)
    }

    /// A PayPal payment to a person, charged to a card.
    pub fn paypal(
        &mut self,
        acct: Acct,
        date: Date,
        cents: i64,
        person: (&str, &str),
        category: &'static str,
    ) -> usize {
        let view = self.accounts[acct].view.clone();
        let description = describe::paypal(&view, person.0, person.1, date, &mut self.noise);
        let mut d = self.blank(
            acct,
            date,
            -cents,
            description,
            DraftBucket::Category(category),
        );
        d.posted = add_days(date, self.timing.range(1, 2) as i32);
        d.has_txn_date = true;
        d.merchant = Some(format!("{} {}", person.0, person.1));
        d.tags.push("p2p");
        self.push(d)
    }

    /// Cash from an ATM, with the out-of-network fee as its own line when charged.
    pub fn atm(&mut self, acct: Acct, date: Date, cents: i64, fee: Option<i64>) -> usize {
        let view = self.accounts[acct].view.clone();
        let description = describe::atm(&view, self.home, date, &mut self.noise);
        let mut d = self.blank(
            acct,
            date,
            -cents,
            description,
            DraftBucket::Category("financial.cash_withdrawal"),
        );
        d.tags.push("atm");
        let i = self.push(d);
        if let Some(fee) = fee {
            let mut f = self.blank(
                acct,
                date,
                -fee,
                describe::atm_fee(&view).to_string(),
                DraftBucket::Category("financial.bank_fees"),
            );
            f.merchant = Some(self.accounts[acct].institution.to_string());
            self.push(f);
        }
        i
    }

    /// Money moved between two of the user's accounts. `lag` is the days before it
    /// reaches `to`. Returns (out, in) draft indices.
    pub fn transfer(
        &mut self,
        from: Acct,
        to: Acct,
        date: Date,
        cents: i64,
        lag: i32,
        kind: PairKind,
    ) -> (usize, usize) {
        let lag = lag.min(self.end.days_since(date)).max(0);
        let fv = self.accounts[from].view.clone();
        let tv = self.accounts[to].view.clone();
        let same_bank = self.accounts[from].institution == self.accounts[to].institution;
        let (out_desc, in_desc) = if kind == PairKind::CardPayment {
            (
                describe::card_payment_from(&fv, &tv, date, &mut self.noise),
                describe::card_payment_to(&tv, &mut self.noise),
            )
        } else if same_bank {
            (
                describe::internal_transfer(&fv, &tv, true, date, &mut self.noise),
                describe::internal_transfer(&tv, &fv, false, date, &mut self.noise),
            )
        } else {
            let side = |sim: &mut Sim, me: &AcctView, other: Acct, out: bool| {
                if me.style == Style::AllySavings {
                    describe::external_savings_side(&sim.accounts[other].view, out)
                } else {
                    let company = sim.accounts[other].institution.to_uppercase();
                    describe::ach(me, &company, "TRANSFER", !out, date, &mut sim.noise)
                }
            };
            (side(self, &fv, to, true), side(self, &tv, from, false))
        };
        let out_i =
            self.push(self.blank(from, date, -cents, out_desc, DraftBucket::Transfer(None)));
        let in_date = add_days(date, lag);
        let mut inn = self.blank(
            to,
            in_date,
            cents,
            in_desc,
            DraftBucket::Transfer(Some(out_i)),
        );
        inn.date = date;
        inn.posted = in_date;
        if lag >= 3 {
            inn.tags.push("lagged_transfer");
            self.drafts[out_i].tags.push("lagged_transfer");
        }
        let in_i = self.push(inn);
        self.drafts[out_i].bucket = DraftBucket::Transfer(Some(in_i));
        self.drafts[out_i].pair = Some(kind);
        (out_i, in_i)
    }

    /// A transfer between own accounts on a schedule (an automatic savings transfer).
    /// The money leaves on the next business day and arrives `lag` days later.
    #[allow(clippy::too_many_arguments)]
    pub fn scheduled_transfer(
        &mut self,
        id: &'static str,
        from: Acct,
        to: Acct,
        cadence: Cadence,
        amount: Amount,
        lag: (i32, i32),
        until: Option<Date>,
    ) -> Vec<usize> {
        let mut r = self.rng(&format!("series/{id}"));
        let last = until.unwrap_or(self.end).min(add_days(self.end, -4));
        let mut outs = Vec::new();
        for date in cadence.dates(self.start, last) {
            let d = business_on_or_after(date);
            let cents = amount.at(d, &mut r);
            let lag = r.range(i64::from(lag.0), i64::from(lag.1)) as i32;
            let (o, _) = self.transfer(from, to, d, cents, lag, PairKind::OwnTransfer);
            self.drafts[o].recurring = Some(id.to_string());
            outs.push(o);
        }
        let name = self.accounts[to].view.name.clone();
        self.declare(
            id,
            "transfer",
            &name,
            from,
            cadence.label(),
            amount.is_fixed(),
            until.is_some_and(|u| u < self.end),
        );
        outs
    }

    /// `n` distinct invented people (friends, a babysitter), stable for this persona.
    pub fn people(&self, label: &str, n: usize) -> Vec<(&'static str, &'static str)> {
        let mut r = self.rng(&format!("people/{label}"));
        let mut out: Vec<(&'static str, &'static str)> = Vec::new();
        while out.len() < n {
            let p = (*r.pick(FAKE_FIRST_NAMES), *r.pick(FAKE_SURNAMES));
            if !out.contains(&p) {
                out.push(p);
            }
        }
        out
    }

    /// Every day of the period.
    pub fn days(&self) -> Vec<Date> {
        let mut out = Vec::new();
        let mut d = self.start;
        while d <= self.end {
            out.push(d);
            d = add_days(d, 1);
        }
        out
    }

    /// Runs a habit over the period.
    pub fn habit(&mut self, h: Habit) {
        let mut r = self.rng(&format!("habit/{}", h.label));
        let weight_sum: u32 = h.days.iter().sum();
        let mut d = self.start;
        while d <= self.end {
            let season = h.season.map_or(100, |s| s[usize::from(d.month()) - 1]);
            let expected = h.per_week * f64::from(h.days[weekday(d)]) / f64::from(weight_sum)
                * f64::from(season)
                / 100.0;
            let mut n = expected.floor() as u32;
            if r.chance(expected - expected.floor()) {
                n += 1;
            }
            for _ in 0..n {
                let acct = *r.pick_weighted(&h.accounts);
                let m = *r.pick_weighted(&h.merchants);
                let (lo, hi) = merchant(m).price;
                let cents = (r.around(lo, hi) * h.scale_pct / 100).max(100);
                self.buy(acct, m, d, cents);
            }
            d = add_days(d, 1);
        }
    }

    /// Generates a recurring series and records it for the truth file.
    pub fn recurring(&mut self, s: Recurring) -> Vec<usize> {
        let mut r = self.rng(&format!("series/{}", s.id));
        let from = s.from.unwrap_or(self.start).max(self.start);
        let to = s.to.unwrap_or(self.end).min(self.end);
        let mm = merchant(s.merchant);
        let inflow = s.inflow;
        let mut out = Vec::new();
        for date in s.cadence.dates(from, to) {
            if s.only_months.is_some_and(|ms| !ms.contains(&date.month())) {
                continue;
            }
            let cents = s.amount.at(date, &mut r);
            let i = match s.via {
                Via::Card => self.buy(s.acct, s.merchant, date, cents),
                Via::Ach => {
                    let d = if inflow {
                        business_on_or_before(date)
                    } else {
                        business_on_or_after(date)
                    };
                    if d > self.end || d < self.start {
                        continue;
                    }
                    self.ach(s.acct, s.merchant, d, if inflow { -cents } else { cents })
                }
                Via::Check => match mm.label {
                    Label::Cat(k) => self.check(s.acct, date, cents, mm.name, k),
                    _ => panic!("checks pay for spending"),
                },
            };
            self.drafts[i].recurring = Some(s.id.to_string());
            out.push(i);
        }
        self.declare(
            s.id,
            s.kind,
            mm.name,
            s.acct,
            s.cadence.label(),
            s.amount.is_fixed(),
            s.to.is_some_and(|t| t < self.end),
        );
        out
    }

    /// Records a series whose transactions the script tags itself.
    #[allow(clippy::too_many_arguments)]
    pub fn declare(
        &mut self,
        id: &str,
        kind: &'static str,
        merchant: &str,
        account: Acct,
        cadence: &'static str,
        fixed: bool,
        ends: bool,
    ) {
        assert!(
            self.series.iter().all(|s| s.id != id),
            "duplicate series {id}"
        );
        self.series.push(SeriesMeta {
            id: id.to_string(),
            kind,
            merchant: merchant.to_string(),
            account,
            cadence,
            fixed,
            ends,
        });
    }

    /// Indices of drafts matching a predicate (for scripts that react to earlier ones).
    pub fn find(&self, f: impl Fn(&Draft) -> bool) -> Vec<usize> {
        (0..self.drafts.len())
            .filter(|&i| f(&self.drafts[i]))
            .collect()
    }

    pub fn drafts(&self) -> &[Draft] {
        &self.drafts
    }

    // ---- Passes run by `finish` ----

    /// Some shopping purchases come back, in full or in part.
    fn refunds(&mut self) {
        let mut r = self.rng("refunds");
        let candidates = self.find(|d| {
            matches!(d.bucket, DraftBucket::Category(k) if k.starts_with("shopping.") || k == "family.kids_supplies")
                && d.amount <= -1500
                && !d.one_off
                && d.core.is_some()
        });
        for i in candidates {
            if !r.chance(0.07) {
                continue;
            }
            let orig = self.drafts[i].clone();
            let date = add_days(orig.date, r.range(4, 25) as i32);
            if date > add_days(self.end, -2) {
                continue;
            }
            let cents = if r.chance(0.75) {
                -orig.amount
            } else {
                -orig.amount * r.range(20, 60) / 100
            };
            let (core, loc) = orig.core.clone().expect("purchase has a descriptor");
            let view = self.accounts[orig.account].view.clone();
            let description = describe::purchase(&view, &core, &loc, date, true, &mut self.noise);
            let DraftBucket::Category(k) = orig.bucket else {
                unreachable!()
            };
            let mut d = self.blank(
                orig.account,
                date,
                cents,
                description,
                DraftBucket::Refund(k, Some(i)),
            );
            d.posted = add_days(date, r.range(1, 3) as i32).min(self.end);
            d.has_txn_date = true;
            d.merchant = orig.merchant.clone();
            d.tags.push("refund");
            if cents < -orig.amount {
                d.tags.push("partial_refund");
            }
            self.push(d);
        }
    }

    /// Statements close monthly; the holder pays from checking; carried balances accrue
    /// interest.
    fn card_payments(&mut self) {
        for c in 0..self.accounts.len() {
            let Some(terms) = self.accounts[c].card.clone() else {
                continue;
            };
            let mut r = self.rng(&format!("card/{}", self.accounts[c].key));
            let mut carried: i64 = 0;
            for (y, m) in months(self.start, self.end) {
                let close = day_of_month(y, m, terms.close_day);
                if close < self.start || close > self.end {
                    continue;
                }
                if carried > 0 {
                    let interest = carried * terms.apr_bp / 10_000 / 12;
                    let view = self.accounts[c].view.clone();
                    let mut d = self.blank(
                        c,
                        close,
                        -interest,
                        describe::card_interest(&view).to_string(),
                        DraftBucket::Category("financial.interest"),
                    );
                    d.merchant = Some(self.accounts[c].institution.to_string());
                    d.tags.push("card_interest");
                    self.push(d);
                }
                let owed: i64 = -self
                    .drafts
                    .iter()
                    .filter(|d| d.account == c && d.posted <= close)
                    .map(|d| d.amount)
                    .sum::<i64>();
                carried = 0;
                if owed <= 0 {
                    continue;
                }
                let pay_date = business_on_or_after(add_days(close, terms.due_days));
                let lag = r.range(i64::from(terms.lag.0), i64::from(terms.lag.1)) as i32;
                if add_days(pay_date, lag) > self.end {
                    continue;
                }
                let pay = if terms.partial_months.contains(&(y, m)) {
                    (owed * terms.partial_pct / 100).max(3_500).min(owed)
                } else {
                    owed
                };
                carried = owed - pay;
                let (o, i) =
                    self.transfer(terms.pay_from, c, pay_date, pay, lag, PairKind::CardPayment);
                self.drafts[o].tags.push("card_payment");
                self.drafts[i].tags.push("card_payment");
                if carried > 0 {
                    self.drafts[o].tags.push("partial_payment");
                    self.drafts[i].tags.push("partial_payment");
                }
            }
        }
    }

    /// Savings accounts earn interest on the month-end balance.
    fn savings_interest(&mut self) {
        for s in 0..self.accounts.len() {
            let Some(terms) = self.accounts[s].savings.clone() else {
                continue;
            };
            let id = format!("{}.{}.interest", self.key, self.accounts[s].key);
            for (y, m) in months(self.start, self.end) {
                let last = day_of_month(y, m, 31);
                if last > self.end {
                    continue;
                }
                let balance = terms.opening
                    + self
                        .drafts
                        .iter()
                        .filter(|d| d.account == s && d.posted <= last)
                        .map(|d| d.amount)
                        .sum::<i64>();
                let apy = terms
                    .apy
                    .iter()
                    .filter(|(from, _)| *from <= last)
                    .map(|(_, bp)| *bp)
                    .next_back()
                    .unwrap_or(0);
                let interest = balance * apy / 10_000 / 12;
                if interest <= 0 {
                    continue;
                }
                let view = self.accounts[s].view.clone();
                let mut d = self.blank(
                    s,
                    last,
                    interest,
                    describe::savings_interest(&view).to_string(),
                    DraftBucket::Income,
                );
                d.merchant = Some(self.accounts[s].institution.to_string());
                d.recurring = Some(id.clone());
                self.push(d);
            }
            let institution = self.accounts[s].institution;
            self.declare(&id, "interest", institution, s, "monthly", false, false);
        }
    }

    /// Splits the drafts into the ledger and its ground truth.
    pub fn finish(mut self, description: &str, taxonomy: &Taxonomy) -> (Ledger, PersonaTruth) {
        self.refunds();
        self.card_payments();
        self.savings_interest();

        // Anything that hasn't posted by the end of the period is still pending.
        let end = self.end;
        for d in &mut self.drafts {
            if d.posted > end {
                d.posted = d.date;
                if let Some((amount, desc)) = d.pending.take() {
                    d.amount = amount;
                    d.description = desc;
                }
                d.tags.push("pending");
            }
        }

        // Ids in posting order.
        let mut order: Vec<usize> = (0..self.drafts.len()).collect();
        order.sort_by_key(|&i| {
            (
                self.drafts[i].posted,
                self.drafts[i].account,
                self.drafts[i].date,
                i,
            )
        });
        let mut id_of = vec![TransactionId(0); self.drafts.len()];
        for (pos, &i) in order.iter().enumerate() {
            id_of[i] = TransactionId(pos as i64 + 1);
        }

        let usd = usd();
        let at = Timestamp::from_unix_seconds(
            i64::from(end.days_since_epoch() + 1) * 86_400 + 12 * 3600,
        );
        let mut ledger = Ledger {
            accounts: Vec::new(),
            imports: Vec::new(),
            transactions: Vec::new(),
            taxonomy: taxonomy.clone(),
            decisions: Vec::new(),
            rules: Vec::new(),
        };
        for (a, spec) in self.accounts.iter().enumerate() {
            let id = AccountId(a as i64 + 1);
            ledger.accounts.push(Account {
                id,
                name: spec.view.name.clone(),
                kind: spec.kind,
                currency: usd,
                institution: Some(spec.institution.to_string()),
                masked_number: Some(spec.view.last4.clone()),
                external_id: Some(format!("{}-{}", self.key, spec.key)),
                own_account: spec.own_account,
                created_at: at,
            });
            // The ledger is generated, not imported; each account gets one manual import
            // whose bytes say so, so every transaction can point at an import.
            let bytes = format!(
                "fm-synth generator_version={GENERATOR_VERSION} seed={} persona={} account={}\n",
                self.seed, self.key, spec.key
            );
            let mut raw = RawImport::new(
                ImportId(a as i64 + 1),
                ImportSource::Manual,
                at,
                bytes.into_bytes(),
            );
            raw.file_name = Some(format!("synthetic/{}/{}", self.key, spec.key));
            raw.account_id = Some(id);
            ledger.imports.push(raw);
        }

        let known_keys: std::collections::BTreeSet<&str> =
            taxonomy.categories.iter().map(|c| c.key.as_str()).collect();
        let mut index_in_account = vec![0u32; self.accounts.len()];
        let mut ext = self.rng("external_ids");
        let mut truth_tx = Vec::new();
        let mut pairs = Vec::new();
        for &i in &order {
            let d = &self.drafts[i];
            let id = id_of[i];
            let a = d.account;
            let index = index_in_account[a];
            index_in_account[a] += 1;
            let status = if d.tags.contains(&"pending") {
                TransactionStatus::Pending
            } else {
                TransactionStatus::Posted
            };
            ledger.transactions.push(Transaction {
                id,
                account_id: AccountId(a as i64 + 1),
                source: SourceRef {
                    import_id: ImportId(a as i64 + 1),
                    index,
                    external_id: Some(ext.alnum(16)),
                },
                status,
                posted_date: d.posted,
                transaction_date: d.has_txn_date.then_some(d.date),
                amount: Money::new(d.amount, usd),
                original_amount: d.original,
                description: d.description.clone(),
                payee: None,
                memo: None,
            });
            let check_key = |k: &'static str| {
                assert!(known_keys.contains(k), "unknown category key {k}");
                k.to_string()
            };
            let bucket = match d.bucket {
                DraftBucket::Category(k) => TruthBucket::Category { key: check_key(k) },
                DraftBucket::Transfer(c) => TruthBucket::Transfer {
                    counterpart: c.map(|c| id_of[c]),
                },
                DraftBucket::Income => TruthBucket::Income,
                DraftBucket::Refund(k, o) => TruthBucket::Refund {
                    key: check_key(k),
                    original: o.map(|o| id_of[o]),
                },
            };
            if let (Some(kind), DraftBucket::Transfer(Some(c))) = (d.pair, &d.bucket) {
                pairs.push(TransferPair {
                    out: id,
                    into: id_of[*c],
                    kind,
                });
            }
            truth_tx.push(TxTruth {
                id,
                bucket,
                merchant: d.merchant.clone(),
                recurring: d.recurring.clone(),
                one_off: d.one_off,
                tags: d.tags.iter().map(|t| t.to_string()).collect(),
                pending: d
                    .pending
                    .as_ref()
                    .map(|(amount, description)| PendingTruth {
                        amount: Money::new(*amount, usd),
                        description: description.clone(),
                    }),
            });
        }
        pairs.sort_by_key(|p| p.out);

        let mut recurring = Vec::new();
        for s in &self.series {
            let mut members: Vec<&usize> = order
                .iter()
                .filter(|&&i| self.drafts[i].recurring.as_deref() == Some(s.id.as_str()))
                .collect();
            members.sort_by_key(|&&i| (self.drafts[i].date, id_of[i]));
            assert!(!members.is_empty(), "series {} has no transactions", s.id);
            let amounts: Vec<(Date, i64)> = members
                .iter()
                .map(|&&i| (self.drafts[i].date, self.drafts[i].amount))
                .collect();
            let amount = if s.fixed {
                let mut steps: Vec<Step> = Vec::new();
                for (date, cents) in &amounts {
                    if steps.last().is_none_or(|st| st.amount.minor() != *cents) {
                        steps.push(Step {
                            from: *date,
                            amount: Money::new(*cents, usd),
                        });
                    }
                }
                AmountTruth::Fixed { steps }
            } else {
                AmountTruth::Variable {
                    min: Money::new(amounts.iter().map(|a| a.1).min().expect("non-empty"), usd),
                    max: Money::new(amounts.iter().map(|a| a.1).max().expect("non-empty"), usd),
                }
            };
            recurring.push(SeriesTruth {
                id: s.id.clone(),
                kind: s.kind.to_string(),
                merchant: s.merchant.clone(),
                account_id: AccountId(s.account as i64 + 1),
                cadence: s.cadence.to_string(),
                amount,
                first: amounts[0].0,
                last: amounts[amounts.len() - 1].0,
                ended: s.ends,
                transaction_ids: members.iter().map(|&&i| id_of[i]).collect(),
            });
        }
        recurring.sort_by(|a, b| a.id.cmp(&b.id));

        let truth = PersonaTruth {
            persona: self.key.to_string(),
            description: description.to_string(),
            seed: self.seed,
            generator_version: GENERATOR_VERSION,
            taxonomy_version: taxonomy.version.clone(),
            start: self.start,
            end: self.end,
            transactions: truth_tx,
            transfer_pairs: pairs,
            recurring,
        };
        (ledger, truth)
    }
}
