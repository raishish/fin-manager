//! Bank description formats.
//!
//! The same purchase reads differently at each bank: Amex pads columns, Wells Fargo
//! wraps card purchases in "PURCHASE AUTHORIZED ON …", credit unions shout and
//! truncate. Each [`Style`] mimics one institution's layout, as seen in public sample
//! statements, so the clean-up and categorisation code faces realistic noise.

use fm_core::model::Date;

use crate::cal::mm_dd;
use crate::rng::Rng;

/// One institution's description layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    ChaseChecking,
    BofaChecking,
    WellsChecking,
    CreditUnion,
    AllySavings,
    ChaseCard,
    AmexCard,
    CapOneCard,
}

impl Style {
    pub fn is_card(self) -> bool {
        matches!(self, Style::ChaseCard | Style::AmexCard | Style::CapOneCard)
    }
}

/// What a description may mention about an account.
#[derive(Debug, Clone)]
pub struct AcctView {
    pub style: Style,
    pub name: String,
    pub last4: String,
    /// The account holder as banks print it: `"ALEX EXAMPLE"`.
    pub holder: String,
}

fn split_loc(loc: &str) -> (&str, &str) {
    loc.rsplit_once(' ').unwrap_or((loc, ""))
}

fn trunc(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// Most banks drop apostrophes the way card networks transmit them: `TRADER JOE S`.
fn strip_apostrophes(s: &str) -> String {
    s.replace('\'', " ")
}

fn yymmdd(d: Date) -> String {
    format!("{:02}{:02}{:02}", d.year() % 100, d.month(), d.day())
}

/// A card purchase (or its refund) as printed by `acct`'s bank.
pub fn purchase(
    acct: &AcctView,
    desc: &str,
    loc: &str,
    date: Date,
    refund: bool,
    rng: &mut Rng,
) -> String {
    let (city, st) = split_loc(loc);
    match acct.style {
        Style::ChaseCard => strip_apostrophes(desc),
        Style::AmexCard => format!("{:<20} {:<19} {}", trunc(desc, 20), trunc(city, 19), st),
        Style::CapOneCard => {
            let d = strip_apostrophes(desc);
            if loc.contains('.') || loc.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                trunc(&d, 25)
            } else {
                format!("{} {}{}", trunc(&d, 25), trunc(city, 13), st)
            }
        }
        Style::ChaseChecking => {
            let d = strip_apostrophes(desc);
            if refund {
                format!("{d} {city} {st} REFUND {}", mm_dd(date))
            } else {
                format!("{d} {city} {st} {}", mm_dd(date))
            }
        }
        Style::BofaChecking => {
            let mmdd = mm_dd(date).replace('/', "");
            let kind = if refund {
                "CHECKCARD REFUND"
            } else {
                "CHECKCARD"
            };
            format!(
                "{kind} {mmdd} {} {city} {st} 24{}",
                strip_apostrophes(desc),
                rng.digits(21)
            )
        }
        Style::WellsChecking => {
            let what = if refund {
                "PURCHASE RETURN AUTHORIZED ON"
            } else {
                "PURCHASE AUTHORIZED ON"
            };
            format!(
                "{what} {} {} {city} {st} S{} CARD {}",
                mm_dd(date),
                strip_apostrophes(desc),
                rng.digits(15),
                acct.last4
            )
        }
        Style::CreditUnion => {
            let what = if refund { "POS RETURN" } else { "POS PURCHASE" };
            trunc(
                &format!("{what} {} {city} {st}", strip_apostrophes(desc)).to_uppercase(),
                40,
            )
        }
        Style::AllySavings => panic!("savings accounts have no card"),
    }
}

/// A bank debit or deposit (payroll, utility autopay, tax payment), as printed by
/// `acct`'s bank.
pub fn ach(
    acct: &AcctView,
    company: &str,
    entry: &str,
    credit: bool,
    date: Date,
    rng: &mut Rng,
) -> String {
    let holder = &acct.holder;
    match acct.style {
        Style::ChaseChecking => {
            let class = if credit {
                "PPD"
            } else {
                *rng.pick(&["PPD", "WEB"])
            };
            format!("{company} {entry} {class} ID: {}", rng.digits(10))
        }
        Style::BofaChecking => {
            let class = if credit {
                "PPD"
            } else {
                *rng.pick(&["PPD", "WEB", "CCD"])
            };
            let (last, first) = holder
                .rsplit_once(' ')
                .map_or((holder.as_str(), ""), |(f, l)| (l, f));
            format!(
                "{company} DES:{entry} ID:{} INDN:{last} {first} CO ID:{} {class}",
                rng.alnum(10),
                rng.digits(10)
            )
        }
        Style::WellsChecking => {
            format!(
                "{company} {entry} {} {} {holder}",
                yymmdd(date),
                rng.digits(10)
            )
        }
        Style::CreditUnion => {
            let dir = if credit { "ACH CREDIT" } else { "ACH DEBIT" };
            format!("{dir} {company} {entry}")
        }
        Style::AllySavings | Style::ChaseCard | Style::AmexCard | Style::CapOneCard => {
            panic!("no ACH on {:?}", acct.style)
        }
    }
}

/// A Zelle payment to or from a person. The name is always an invented one.
pub fn zelle(
    acct: &AcctView,
    first: &str,
    last: &str,
    out: bool,
    date: Date,
    rng: &mut Rng,
) -> String {
    let dir = if out { "to" } else { "from" };
    match acct.style {
        Style::ChaseChecking => format!(
            "Zelle payment {dir} {first} {last} JPM99{}",
            rng.alnum(6).to_lowercase()
        ),
        Style::BofaChecking => format!(
            "Zelle payment {dir} {first} {last} Conf# {}",
            rng.alnum(9).to_lowercase()
        ),
        Style::WellsChecking => format!(
            "ZELLE {} {} {} ON {} REF # PP0{}",
            dir.to_uppercase(),
            last.to_uppercase(),
            first.to_uppercase(),
            mm_dd(date),
            rng.alnum(7)
        ),
        Style::CreditUnion => format!(
            "ZELLE {} {} {}",
            if out { "DEBIT" } else { "CREDIT" },
            first.to_uppercase(),
            last.to_uppercase()
        ),
        _ => panic!("no Zelle on {:?}", acct.style),
    }
}

/// A Venmo payment funded from the bank account. Banks don't show who it went to.
pub fn venmo(acct: &AcctView, date: Date, rng: &mut Rng) -> String {
    match acct.style {
        Style::ChaseChecking => format!("VENMO PAYMENT {} WEB ID: 3264681992", rng.digits(10)),
        Style::CreditUnion => "ACH DEBIT VENMO PAYMENT".to_string(),
        _ => ach(acct, "VENMO", "PAYMENT", false, date, rng),
    }
}

/// A PayPal payment to a person, charged to a card. PayPal squeezes the name into the
/// descriptor, so it arrives truncated.
pub fn paypal(acct: &AcctView, first: &str, last: &str, date: Date, rng: &mut Rng) -> String {
    let desc = trunc(
        &format!("PAYPAL *{}{}", first.to_uppercase(), last.to_uppercase()),
        22,
    );
    purchase(acct, &desc, "402-935-7733 CA", date, false, rng)
}

/// A transfer between two accounts at the same bank.
pub fn internal_transfer(
    acct: &AcctView,
    other: &AcctView,
    out: bool,
    date: Date,
    rng: &mut Rng,
) -> String {
    let short = if other.name.to_uppercase().contains("SAV") {
        "SAV"
    } else {
        "CHK"
    };
    match acct.style {
        Style::ChaseChecking => format!(
            "Online Transfer {} {short} ...{} transaction#: {} {}",
            if out { "to" } else { "from" },
            other.last4,
            rng.digits(11),
            mm_dd(date)
        ),
        Style::BofaChecking => format!(
            "Online Banking transfer {} {short} {} Confirmation# {}",
            if out { "to" } else { "from" },
            other.last4,
            rng.alnum(10)
        ),
        Style::WellsChecking => format!(
            "ONLINE TRANSFER {} {} {} XXXXXX{} REF #IB0{} ON {:02}/{:02}/{:02}",
            if out { "TO" } else { "FROM" },
            acct.holder,
            other.name.to_uppercase(),
            other.last4,
            rng.alnum(7),
            date.month(),
            date.day(),
            date.year() % 100
        ),
        Style::CreditUnion => format!(
            "SHARE TRANSFER {} {}",
            if out { "TO" } else { "FROM" },
            other.last4
        ),
        _ => panic!("no internal transfers on {:?}", acct.style),
    }
}

/// The online-savings side of a transfer to or from an account at another bank.
pub fn external_savings_side(other: &AcctView, out: bool) -> String {
    format!(
        "Requested transfer {} {} *{}",
        if out { "to" } else { "from" },
        other.name,
        other.last4
    )
}

/// The checking side of a card payment.
pub fn card_payment_from(acct: &AcctView, card: &AcctView, date: Date, rng: &mut Rng) -> String {
    match (acct.style, card.style) {
        (Style::ChaseChecking, Style::ChaseCard) => {
            format!(
                "Payment to Chase card ending in {} {}",
                card.last4,
                mm_dd(date)
            )
        }
        (_, Style::ChaseCard) => ach(acct, "CHASE CREDIT CRD", "EPAY", false, date, rng),
        (_, Style::AmexCard) => ach(acct, "AMERICAN EXPRESS", "ACH PMT", false, date, rng),
        (_, Style::CapOneCard) => ach(acct, "CAPITAL ONE", "MOBILE PMT", false, date, rng),
        _ => panic!("{:?} is not a card", card.style),
    }
}

/// The card side of a card payment.
pub fn card_payment_to(card: &AcctView, rng: &mut Rng) -> String {
    let options: &[&str] = match card.style {
        Style::ChaseCard => &["AUTOMATIC PAYMENT - THANK", "Payment Thank You-Mobile"],
        Style::AmexCard => &["AUTOPAY PAYMENT - THANK YOU", "ONLINE PAYMENT - THANK YOU"],
        Style::CapOneCard => &["CAPITAL ONE AUTOPAY PYMT", "CAPITAL ONE MOBILE PYMT"],
        _ => panic!("{:?} is not a card", card.style),
    };
    rng.pick(options).to_string()
}

pub fn card_interest(card: &AcctView) -> &'static str {
    match card.style {
        Style::ChaseCard => "PURCHASE INTEREST CHARGE",
        Style::AmexCard => "INTEREST CHARGE ON PURCHASES",
        _ => "INTEREST CHARGE:PURCHASES",
    }
}

pub fn foreign_fee(card: &AcctView) -> &'static str {
    match card.style {
        Style::AmexCard => "FOREIGN TRANSACTION FEE - 2.7%",
        _ => "FOREIGN TRANSACTION FEE",
    }
}

pub fn savings_interest(acct: &AcctView) -> &'static str {
    match acct.style {
        Style::AllySavings => "Interest Paid",
        Style::BofaChecking => "Interest Earned",
        Style::CreditUnion => "DIVIDEND EARNED",
        _ => "INTEREST PAYMENT",
    }
}

const STREETS: &[&str] = &[
    "MAIN ST",
    "MARKET ST",
    "OAK AVE",
    "BROADWAY",
    "HIGH ST",
    "CONGRESS AVE",
    "SPEEDWAY BLVD",
];

pub fn atm(acct: &AcctView, loc: &str, date: Date, rng: &mut Rng) -> String {
    let (city, st) = split_loc(loc);
    let addr = format!("{} {}", rng.range(100, 4999), rng.pick(STREETS));
    match acct.style {
        Style::ChaseChecking => format!(
            "NON-CHASE ATM WITHDRAW {} {} {addr} {city} {st}",
            rng.digits(6),
            mm_dd(date)
        ),
        Style::BofaChecking => format!(
            "BKOFAMERICA ATM {} #{} WITHDRWL {city} {st}",
            mm_dd(date),
            rng.digits(9)
        ),
        Style::WellsChecking => format!(
            "ATM WITHDRAWAL AUTHORIZED ON {} {addr} {city} {st} {} ATM ID {} CARD {}",
            mm_dd(date),
            rng.digits(7),
            rng.digits(4),
            acct.last4
        ),
        Style::CreditUnion => format!("ATM WITHDRAWAL {addr} {city}"),
        _ => panic!("no ATM on {:?}", acct.style),
    }
}

pub fn atm_fee(acct: &AcctView) -> &'static str {
    match acct.style {
        Style::ChaseChecking => "NON-CHASE ATM FEE-WITH",
        Style::BofaChecking => "ATM FEE-NON-BANK OF AMERICA",
        Style::WellsChecking => "NON-WELLS FARGO ATM TRANSACTION FEE",
        _ => "ATM SURCHARGE FEE",
    }
}

pub fn check(acct: &AcctView, number: u32) -> String {
    match acct.style {
        Style::ChaseChecking => format!("CHECK {number}"),
        Style::BofaChecking => format!("Check {number}"),
        Style::WellsChecking => format!("CHECK # {number}"),
        _ => format!("SHARE DRAFT #{number}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cal::ymd;

    fn view(style: Style) -> AcctView {
        AcctView {
            style,
            name: "Everyday Checking".into(),
            last4: "4821".into(),
            holder: "ALEX EXAMPLE".into(),
        }
    }

    #[test]
    fn same_purchase_differs_by_bank() {
        let d = ymd(2025, 4, 12);
        let mut rng = Rng::new(1);
        let lines: Vec<String> = [
            Style::ChaseCard,
            Style::AmexCard,
            Style::CapOneCard,
            Style::WellsChecking,
            Style::CreditUnion,
        ]
        .into_iter()
        .map(|s| {
            purchase(
                &view(s),
                "TRADER JOE'S #552",
                "SAN FRANCISCO CA",
                d,
                false,
                &mut rng,
            )
        })
        .collect();
        assert_eq!(lines[0], "TRADER JOE S #552");
        assert_eq!(lines[1], "TRADER JOE'S #552    SAN FRANCISCO       CA");
        assert_eq!(lines[2], "TRADER JOE S #552 SAN FRANCISCOCA");
        assert!(
            lines[3]
                .starts_with("PURCHASE AUTHORIZED ON 04/12 TRADER JOE S #552 SAN FRANCISCO CA S")
        );
        assert!(lines[3].ends_with("CARD 4821"));
        assert_eq!(lines[4], "POS PURCHASE TRADER JOE S #552 SAN FRANC");
    }
}
