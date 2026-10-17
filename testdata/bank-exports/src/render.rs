//! Turns rows into file bytes and the matching expected output, following a layout.

use fm_core::model::{Account, AccountKind, Date, TransactionStatus};

use crate::expected::{ExpectedAccount, ExpectedFile, ExpectedTx, SourceAccount};
use crate::source::{Row, money};
use crate::spec::{AmountFormat, CsvSpec, Format, Layout, OfxSpec, Pad, Role};

/// Everything about one file except its rows.
pub struct FileCtx {
    pub file: String,
    pub persona: String,
    pub account: Account,
    pub as_of: Date,
    pub start: Date,
    pub end: Date,
    /// Full (fake) account number.
    pub number: String,
    /// Fake routing number.
    pub routing: String,
    /// Balance once everything posted by `as_of` is counted.
    pub balance_as_of: i64,
    /// Balance before the first row of the window.
    pub balance_before: i64,
}

pub fn kind_name(kind: AccountKind) -> &'static str {
    match kind {
        AccountKind::Checking => "checking",
        AccountKind::Savings => "savings",
        AccountKind::CreditCard => "credit_card",
        _ => "other",
    }
}

pub fn fmt_date(d: Date, format: &str) -> String {
    let (y, m, day) = (d.year(), d.month(), d.day());
    match format {
        "MM/DD/YYYY" => format!("{m:02}/{day:02}/{y:04}"),
        "M/D/YYYY" => format!("{m}/{day}/{y:04}"),
        "MM/DD/YY" => format!("{m:02}/{day:02}/{:02}", y % 100),
        "YYYY-MM-DD" => format!("{y:04}-{m:02}-{day:02}"),
        "YYYYMMDD" => format!("{y:04}{m:02}{day:02}"),
        other => panic!("unknown date format {other}"),
    }
}

pub fn fmt_amount(minor: i64, f: &AmountFormat) -> String {
    let abs = minor.unsigned_abs();
    let (whole, cents) = (abs / 100, abs % 100);
    let mut digits = whole.to_string();
    if f.thousands_separator {
        let mut grouped = String::new();
        for (i, c) in digits.chars().enumerate() {
            if i > 0 && (digits.len() - i) % 3 == 0 {
                grouped.push(',');
            }
            grouped.push(c);
        }
        digits = grouped;
    }
    let body = format!(
        "{}{digits}.{cents:02}",
        if f.currency_symbol { "$" } else { "" }
    );
    match (minor < 0, f.negative) {
        (false, _) => body,
        (true, "parentheses") => format!("({body})"),
        (true, _) => format!("-{body}"),
    }
}

fn csv_field(s: &str, quote: bool, spec: &CsvSpec) -> String {
    let needs =
        s.contains(spec.delimiter) || s.contains('"') || s.contains('\n') || s.contains('\r');
    if needs || (quote && (spec.quote_empty || !s.is_empty())) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn has_role(spec: &CsvSpec, f: impl Fn(&Role) -> bool) -> bool {
    spec.columns.iter().any(|c| f(&c.role))
}

fn pad(s: String, p: Pad) -> String {
    match p {
        Pad::None => s,
        Pad::Trailing(n) => format!("{s}{}", " ".repeat(n)),
        Pad::Width(w) => format!("{s:<w$}"),
    }
}

fn render_csv(
    layout: &Layout,
    spec: &CsvSpec,
    ctx: &FileCtx,
    rows: &[Row],
) -> (Vec<u8>, Vec<ExpectedTx>) {
    let eol = if spec.line_ending == "crlf" {
        "\r\n"
    } else {
        "\n"
    };
    let sep = spec.delimiter.to_string();
    let mut lines: Vec<String> = (layout.preamble)(rows, ctx);
    assert_eq!(
        lines.len(),
        spec.skip_lines,
        "{}: preamble length",
        layout.id
    );
    let row_line = |cells: Vec<String>| {
        let mut line = cells
            .iter()
            .zip(&spec.columns)
            .map(|(v, c)| csv_field(v, spec.quote_all || c.quoted, spec))
            .collect::<Vec<_>>()
            .join(&sep);
        if spec.trailing_delimiter != "none" {
            line.push(spec.delimiter);
        }
        line
    };
    if spec.has_header {
        let mut line = spec
            .columns
            .iter()
            .map(|c| csv_field(c.header, spec.quote_header, spec))
            .collect::<Vec<_>>()
            .join(&sep);
        if spec.trailing_delimiter == "all" {
            line.push(spec.delimiter);
        }
        lines.push(line);
    }
    lines.extend((layout.lead_rows)(rows, ctx));
    let has_posted = has_role(spec, |r| matches!(r, Role::PostedDate));
    let has_txn = has_role(spec, |r| matches!(r, Role::TransactionDate));
    assert!(has_posted || has_txn, "{}: needs a date column", layout.id);
    assert!(
        !layout.includes_pending || has_role(spec, |r| matches!(r, Role::Status { .. })),
        "{}: pending needs a status column",
        layout.id
    );
    assert!(
        spec.trim || spec.columns.iter().all(|c| matches!(c.pad, Pad::None)),
        "{}: padded columns need `trim`",
        layout.id
    );
    let f = &spec.amount_format;
    let mut expected = Vec::new();
    for row in rows {
        let txn_date = row.transaction_date.unwrap_or(row.posted_date);
        let category = (layout.category_of)(row.category.as_deref(), row.kind);
        let reference = |wrap: &str| {
            row.external_id
                .as_ref()
                .map(|r| format!("{wrap}{r}{wrap}"))
                .unwrap_or_default()
        };
        let cells = spec
            .columns
            .iter()
            .map(|c| {
                let v = match &c.role {
                    Role::PostedDate => fmt_date(row.posted_date, spec.date_format),
                    Role::TransactionDate => fmt_date(txn_date, spec.date_format),
                    Role::Description => row.description.clone(),
                    Role::Payee => row.payee(),
                    Role::Memo => String::new(),
                    Role::Amount { money_out } => fmt_amount(
                        if *money_out == "positive" {
                            -row.amount
                        } else {
                            row.amount
                        },
                        f,
                    ),
                    Role::Debit => {
                        if row.amount < 0 {
                            fmt_amount(-row.amount, f)
                        } else {
                            String::new()
                        }
                    }
                    Role::Credit { negative_printed } => match (row.amount > 0, negative_printed) {
                        (false, _) => String::new(),
                        (true, true) => fmt_amount(-row.amount, f),
                        (true, false) => fmt_amount(row.amount, f),
                    },
                    Role::AbsAmount => fmt_amount(row.amount.abs(), f),
                    Role::Indicator { debit, credit } => {
                        if row.amount < 0 {
                            debit.to_string()
                        } else {
                            credit.to_string()
                        }
                    }
                    Role::Balance => row.balance.map(|b| fmt_amount(b, f)).unwrap_or_default(),
                    Role::CheckNumber => row.check_number.clone().unwrap_or_default(),
                    Role::Reference { wrapped_in } => reference(wrapped_in),
                    Role::Status { pending, posted } => {
                        if row.pending {
                            pending.to_string()
                        } else {
                            posted.to_string()
                        }
                    }
                    Role::BankCategory => category.to_string(),
                    Role::BankSubcategory | Role::Extra { .. } => {
                        let what = match &c.role {
                            Role::Extra { what } => what,
                            _ => "subcategory",
                        };
                        (layout.extra_of)(what, row, ctx)
                    }
                    Role::Type => (layout.type_of)(row.kind, row).to_string(),
                    Role::AccountNumber => ctx.account.masked_number.clone().unwrap_or_default(),
                    Role::Fixed { value } => value.to_string(),
                };
                pad(v, c.pad)
            })
            .collect();
        lines.push(row_line(cells));
        let payee = has_role(spec, |r| matches!(r, Role::Payee)).then(|| row.payee());
        expected.push(ExpectedTx {
            truth_id: row.truth_id,
            status: if row.pending {
                TransactionStatus::Pending
            } else {
                TransactionStatus::Posted
            },
            posted_date: if has_posted {
                row.posted_date
            } else {
                txn_date
            },
            transaction_date: has_txn.then_some(txn_date),
            amount: money(row.amount),
            description: row.description.clone(),
            payee,
            memo: None,
            external_id: if has_role(spec, |r| matches!(r, Role::Reference { .. })) {
                row.external_id.clone()
            } else {
                None
            },
            check_number: if has_role(spec, |r| matches!(r, Role::CheckNumber)) {
                row.check_number.clone()
            } else {
                None
            },
            balance: if has_role(spec, |r| matches!(r, Role::Balance)) {
                row.balance.map(money)
            } else {
                None
            },
            bank_category: (has_role(spec, |r| matches!(r, Role::BankCategory))
                && !category.is_empty())
            .then(|| category.to_string()),
        });
    }
    let mut text = lines.join(eol);
    text.push_str(eol);
    let mut bytes = Vec::new();
    if spec.utf8_bom {
        bytes.extend_from_slice(b"\xEF\xBB\xBF");
    }
    bytes.extend_from_slice(text.as_bytes());
    (bytes, expected)
}

/// An OFX element: an aggregate with children, or a leaf with a value.
enum El {
    Agg(&'static str, Vec<El>),
    Leaf(&'static str, String),
}

fn leaf(tag: &'static str, v: impl Into<String>) -> El {
    El::Leaf(tag, v.into())
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn write_el(el: &El, spec: &OfxSpec, xml: bool, depth: usize, out: &mut Vec<String>) {
    let indent = if spec.line_per_element {
        "  ".repeat(depth)
    } else {
        String::new()
    };
    match el {
        El::Agg(tag, kids) => {
            out.push(format!("{indent}<{tag}>"));
            for k in kids {
                write_el(k, spec, xml, depth + 1, out);
            }
            out.push(format!("{indent}</{tag}>"));
        }
        El::Leaf(tag, v) => {
            if xml || spec.close_leaf_tags {
                out.push(format!("{indent}<{tag}>{}</{tag}>", escape(v)));
            } else {
                out.push(format!("{indent}<{tag}>{}", escape(v)));
            }
        }
    }
}

fn ofx_dt(d: Date, spec: &OfxSpec) -> String {
    if spec.tz.is_empty() && !spec.date_only_postings {
        format!("{}120000", fmt_date(d, "YYYYMMDD"))
    } else if spec.tz.is_empty() {
        fmt_date(d, "YYYYMMDD")
    } else {
        format!("{}120000.000{}", fmt_date(d, "YYYYMMDD"), spec.tz)
    }
}

fn ofx_amount(minor: i64) -> String {
    fmt_amount(minor, &crate::spec::PLAIN)
}

fn status(spec: &OfxSpec) -> El {
    El::Agg(
        "STATUS",
        vec![leaf("CODE", "0"), leaf("SEVERITY", spec.severity)],
    )
}

/// Posting dates: date only when the bank sends them that way.
fn ofx_posting(d: Date, spec: &OfxSpec) -> String {
    if spec.date_only_postings {
        fmt_date(d, "YYYYMMDD")
    } else {
        ofx_dt(d, spec)
    }
}

fn render_ofx(
    layout: &Layout,
    spec: &OfxSpec,
    ctx: &FileCtx,
    rows: &[Row],
) -> (Vec<u8>, Vec<ExpectedTx>, ExpectedAccount) {
    let xml = spec.version.starts_with('2');
    let card = ctx.account.kind == AccountKind::CreditCard;
    let mut sonrs = vec![
        status(spec),
        leaf("DTSERVER", ofx_dt(ctx.as_of, spec)),
        leaf("LANGUAGE", spec.language),
        El::Agg("FI", vec![leaf("ORG", spec.org), leaf("FID", spec.fid)]),
    ];
    if let Some(bid) = spec.intu_bid {
        sonrs.push(leaf("INTU.BID", bid));
    }
    let dtstart = spec
        .dtstart
        .map_or_else(|| ofx_dt(ctx.start, spec), str::to_string);
    let mut trns = vec![
        leaf("DTSTART", dtstart),
        leaf("DTEND", ofx_dt(ctx.end, spec)),
    ];
    let mut expected = Vec::new();
    for row in rows {
        assert!(!row.pending, "OFX exports carry posted transactions only");
        let fitid = row.external_id.clone().expect("posted rows have an id");
        // OFX element values don't keep trailing spaces (some descriptions end in one
        // where a bank cut them short), so neither do NAME and MEMO.
        let full = row.description.trim_end();
        let name: String = full
            .chars()
            .take(spec.name_max)
            .collect::<String>()
            .trim_end()
            .to_string();
        let cut = name.chars().count() < full.chars().count();
        let (name, memo) = match spec.text {
            "memo_only" => (None, Some(full.to_string())),
            "name_and_memo" => (Some(name.clone()), Some(name)),
            _ => (
                Some(name),
                (spec.memo == "overflow" && cut).then(|| full.to_string()),
            ),
        };
        let mut t = vec![
            leaf("TRNTYPE", (layout.type_of)(row.kind, row)),
            leaf("DTPOSTED", ofx_posting(row.posted_date, spec)),
        ];
        if spec.dtuser
            && let Some(d) = row.transaction_date
        {
            t.push(leaf("DTUSER", ofx_posting(d, spec)));
        }
        t.push(leaf("TRNAMT", ofx_amount(row.amount)));
        t.push(leaf("FITID", fitid.clone()));
        if let Some(n) = &row.check_number {
            t.push(leaf("CHECKNUM", n.clone()));
        }
        if let Some(n) = &name {
            t.push(leaf("NAME", n.clone()));
        }
        if let Some(m) = &memo {
            t.push(leaf("MEMO", m.clone()));
        }
        trns.push(El::Agg("STMTTRN", t));
        expected.push(ExpectedTx {
            truth_id: row.truth_id,
            status: TransactionStatus::Posted,
            posted_date: row.posted_date,
            transaction_date: if spec.dtuser {
                row.transaction_date
            } else {
                None
            },
            amount: money(row.amount),
            description: name.clone().or_else(|| memo.clone()).expect("NAME or MEMO"),
            payee: None,
            memo,
            external_id: Some(fitid),
            check_number: row.check_number.clone(),
            balance: None,
            bank_category: None,
        });
    }
    let ledgerbal = El::Agg(
        "LEDGERBAL",
        vec![
            leaf("BALAMT", ofx_amount(ctx.balance_as_of)),
            leaf("DTASOF", ofx_dt(ctx.as_of, spec)),
        ],
    );
    let tranlist = El::Agg("BANKTRANLIST", trns);
    let msgs = if card && !spec.card_as_creditline {
        El::Agg(
            "CREDITCARDMSGSRSV1",
            vec![El::Agg(
                "CCSTMTTRNRS",
                vec![
                    leaf("TRNUID", "1"),
                    status(spec),
                    El::Agg(
                        "CCSTMTRS",
                        vec![
                            leaf("CURDEF", "USD"),
                            El::Agg("CCACCTFROM", vec![leaf("ACCTID", ctx.number.clone())]),
                            tranlist,
                            ledgerbal,
                        ],
                    ),
                ],
            )],
        )
    } else {
        let acct_type = match ctx.account.kind {
            AccountKind::Savings => "SAVINGS",
            AccountKind::CreditCard => "CREDITLINE",
            _ => "CHECKING",
        };
        El::Agg(
            "BANKMSGSRSV1",
            vec![El::Agg(
                "STMTTRNRS",
                vec![
                    leaf("TRNUID", "1"),
                    status(spec),
                    El::Agg(
                        "STMTRS",
                        vec![
                            leaf("CURDEF", "USD"),
                            El::Agg(
                                "BANKACCTFROM",
                                vec![
                                    leaf("BANKID", ctx.routing.clone()),
                                    leaf("ACCTID", ctx.number.clone()),
                                    leaf("ACCTTYPE", acct_type),
                                ],
                            ),
                            tranlist,
                            ledgerbal,
                        ],
                    ),
                ],
            )],
        )
    };
    let doc = El::Agg(
        "OFX",
        vec![
            El::Agg("SIGNONMSGSRSV1", vec![El::Agg("SONRS", sonrs)]),
            msgs,
        ],
    );
    let mut parts = Vec::new();
    write_el(&doc, spec, xml, 0, &mut parts);
    let (header, eol) = if xml {
        (
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\"?>\n<?OFX OFXHEADER=\"200\" VERSION=\"{}\" SECURITY=\"NONE\" OLDFILEUID=\"NONE\" NEWFILEUID=\"NONE\"?>\n",
                spec.version
            ),
            "\n",
        )
    } else {
        let fileuids = if spec.header_fileuids {
            "OLDFILEUID:NONE\r\nNEWFILEUID:NONE\r\n"
        } else {
            ""
        };
        (
            format!(
                "{}OFXHEADER:100\r\nDATA:OFXSGML\r\nVERSION:{}\r\nSECURITY:NONE\r\nENCODING:USASCII\r\nCHARSET:1252\r\nCOMPRESSION:NONE\r\n{fileuids}\r\n",
                if spec.leading_blank_line { "\r\n" } else { "" },
                spec.version
            ),
            "\r\n",
        )
    };
    let body = if spec.line_per_element {
        parts.join(eol)
    } else {
        parts.join("")
    };
    let bytes = format!("{header}{body}{eol}").into_bytes();
    let account = ExpectedAccount {
        kind: kind_name(ctx.account.kind).to_string(),
        currency: "USD".to_string(),
        number: Some(ctx.number.clone()),
        routing: (!card || spec.card_as_creditline).then(|| ctx.routing.clone()),
        ledger_balance: Some(money(ctx.balance_as_of)),
    };
    (bytes, expected, account)
}

/// Renders one file and its expected output.
pub fn render(layout: &Layout, ctx: &FileCtx, rows: &[Row]) -> (Vec<u8>, ExpectedFile) {
    let mut rows = rows.to_vec();
    if layout.newest_first {
        rows.reverse();
    }
    let (bytes, transactions, account) = match &layout.format {
        Format::Csv(spec) => {
            let (b, t) = render_csv(layout, spec, ctx, &rows);
            let account = ExpectedAccount {
                kind: kind_name(ctx.account.kind).to_string(),
                currency: "USD".to_string(),
                number: None,
                routing: None,
                ledger_balance: None,
            };
            (b, t, account)
        }
        Format::Ofx(spec) => render_ofx(layout, spec, ctx, &rows),
    };
    let expected = ExpectedFile {
        file: ctx.file.clone(),
        layout: layout.id.to_string(),
        institution: layout.institution.to_string(),
        source: SourceAccount {
            persona: ctx.persona.clone(),
            account_id: ctx.account.id,
        },
        as_of: ctx.as_of,
        start: ctx.start,
        end: ctx.end,
        account,
        transactions,
    };
    (bytes, expected)
}
