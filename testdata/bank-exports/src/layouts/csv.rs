//! CSV layouts, one per institution and account type. Each records the public
//! samples or importer code it was checked against.

use super::*;
use crate::render::fmt_amount;
use crate::spec::{AmountFormat, Pad, Role, col, qcol};

const DOLLARS: AmountFormat = AmountFormat {
    thousands_separator: true,
    currency_symbol: true,
    negative: "minus",
};

const THOUSANDS: AmountFormat = AmountFormat {
    thousands_separator: true,
    currency_symbol: false,
    negative: "minus",
};

const OUT_NEGATIVE: Role = Role::Amount {
    money_out: "negative",
};
const OUT_POSITIVE: Role = Role::Amount {
    money_out: "positive",
};

pub fn layouts() -> Vec<Layout> {
    vec![
        chase_card(),
        chase_checking(),
        bofa_checking(),
        bofa_card(),
        wells_legacy(),
        wells_2026(),
        citi_card(),
        capone_card(),
        capone_360(),
        us_bank(),
        pnc(),
        truist(),
        amex_basic(),
        amex_extended(),
        discover(),
        apple_card(),
        ally(),
        schwab(),
        usaa(),
        navy_federal(),
        td_bank(),
        barclays(),
    ]
}

fn chase_card() -> Layout {
    let spec = csv_spec(
        vec![
            col("Transaction Date", Role::TransactionDate),
            col("Post Date", Role::PostedDate),
            col("Description", Role::Description),
            col("Category", Role::BankCategory),
            col("Type", Role::Type),
            col("Amount", OUT_NEGATIVE),
            col("Memo", Role::Memo),
        ],
        "MM/DD/YYYY",
    );
    let mut l = base(
        "chase_card_csv",
        "Chase",
        &["credit_card"],
        "csv",
        prov(
            "high",
            &[
                "https://github.com/hoffmanaiden/finances/blob/HEAD/csv/joint/ChaseJoint_2026-07.csv",
                "https://github.com/avibanerjee/aiPersonalFinance/blob/HEAD/ChaseStatements/SaphirePreferred.CSV",
                "https://github.com/bdmckean/code_off/blob/HEAD/data/chase_transactions.csv",
                "https://github.com/ReadableCode/Cash_Flow_Commander/blob/HEAD/src/providers/chase.py",
            ],
            "Real downloads. Purchases negative, payments positive. Memo is usually empty, so lines end in a comma. \
             Variants not rendered: a leading Card column with 2-digit years, and the 2012-2018 \
             `Type,Trans Date,Post Date,Description,Amount` layout. Chase caps a download at 1,000 rows. \
             Category values are Chase's names, mapped from our categories.",
        ),
        Format::Csv(spec),
    );
    l.type_of = |k, _| match k {
        Kind::Purchase => "Sale",
        Kind::Refund => "Return",
        Kind::CardPaymentIn => "Payment",
        Kind::Fee | Kind::InterestCharged => "Fee",
        _ => "Adjustment",
    };
    l.category_of = chase_category;
    l
}

fn chase_checking() -> Layout {
    let spec = CsvSpec {
        line_ending: "crlf",
        trailing_delimiter: "rows",
        ..csv_spec(
            vec![
                col("Details", Role::Extra { what: "details" }),
                col("Posting Date", Role::PostedDate),
                qcol("Description", Role::Description),
                col("Amount", OUT_NEGATIVE),
                col("Type", Role::Type),
                col("Balance", Role::Balance),
                col("Check or Slip #", Role::CheckNumber),
            ],
            "MM/DD/YYYY",
        )
    };
    let mut l = base(
        "chase_checking_csv",
        "Chase",
        &["checking", "savings"],
        "csv",
        prov(
            "high",
            &[
                "https://github.com/kanema20/primeorca/blob/HEAD/Chase7573_Activity_20230828.CSV",
                "https://github.com/andrewkatz/ynar/blob/HEAD/spec/support/fixtures/transactions.csv",
                "https://github.com/firefly-iii/import-configurations/blob/main/us/chase/default.json",
            ],
            "Real downloads. Data rows have one more field than the 7-column header (a trailing comma); \
             the header doesn't. CRLF. Description always quoted. Details is DEBIT/CREDIT/CHECK/DSLIP.",
        ),
        Format::Csv(spec),
    );
    l.extra_of = extras;
    l.type_of = |k, _| match k {
        Kind::AchDebit => "ACH_DEBIT",
        Kind::AchCredit => "ACH_CREDIT",
        Kind::CardPaymentOut => "LOAN_PMT",
        Kind::TransferIn | Kind::TransferOut => "ACCT_XFER",
        Kind::P2pOut => "QUICKPAY_DEBIT",
        Kind::P2pIn => "QUICKPAY_CREDIT",
        Kind::Purchase => "DEBIT_CARD",
        Kind::Check => "CHECK_PAID",
        Kind::Atm => "ATM",
        Kind::Fee => "FEE_TRANSACTION",
        _ => "MISC_CREDIT",
    };
    l
}

/// Bank of America's summary block: quoted amounts with thousands separators.
fn bofa_amount(minor: i64) -> String {
    format!("\"{}\"", fmt_amount(minor, &THOUSANDS))
}

fn bofa_checking() -> Layout {
    let spec = CsvSpec {
        line_ending: "crlf",
        skip_lines: 6,
        amount_format: THOUSANDS,
        skip_rows_without_amount: true,
        ..csv_spec(
            vec![
                col("Date", Role::PostedDate),
                qcol("Description", Role::Description),
                qcol("Amount", OUT_NEGATIVE),
                qcol("Running Bal.", Role::Balance),
            ],
            "MM/DD/YYYY",
        )
    };
    let mut l = base(
        "bofa_checking_csv",
        "Bank of America",
        &["checking", "savings"],
        "csv",
        prov(
            "high",
            &[
                "https://github.com/VishruthR/Pennyful/blob/HEAD/data/bankofamericatransactions.csv",
                "https://github.com/enettolima/es-document-indexer/blob/HEAD/es_docs/Projetcs/stmt.csv",
                "https://github.com/averatec0773/yomi/blob/HEAD/guides/import-boa.md",
            ],
            "Real downloads. A 5-line summary and a blank line come before the header; the first row is a \
             'Beginning balance' line with no amount. Amounts quoted with thousands separators. CRLF, oldest \
             first. Bank of America ended QFX/OFX downloads on 2025-09-30, so only CSV is rendered. Not \
             rendered: one 2025 sample has an unescaped quote inside a Zelle description.",
        ),
        Format::Csv(spec),
    );
    l.newest_first = false;
    l.preamble = |rows, ctx| {
        let credits: i64 = rows.iter().filter(|r| r.amount > 0).map(|r| r.amount).sum();
        let debits: i64 = rows.iter().filter(|r| r.amount < 0).map(|r| r.amount).sum();
        let ending = ctx.balance_before + credits + debits;
        let d = |d| crate::render::fmt_date(d, "MM/DD/YYYY");
        vec![
            "Description,,Summary Amt.".to_string(),
            format!(
                "Beginning balance as of {},,{}",
                d(ctx.start),
                bofa_amount(ctx.balance_before)
            ),
            format!("Total credits,,{}", bofa_amount(credits)),
            format!("Total debits,,{}", bofa_amount(debits)),
            format!(
                "Ending balance as of {},,{}",
                d(ctx.end),
                bofa_amount(ending)
            ),
            String::new(),
        ]
    };
    l.lead_rows = |_, ctx| {
        let d = crate::render::fmt_date(ctx.start, "MM/DD/YYYY");
        vec![format!(
            "{d},Beginning balance as of {d},,{}",
            bofa_amount(ctx.balance_before)
        )]
    };
    l
}

fn bofa_card() -> Layout {
    let spec = CsvSpec {
        line_ending: "crlf",
        trim: true,
        ..csv_spec(
            vec![
                col("Posted Date", Role::PostedDate),
                col("Reference Number", Role::Reference { wrapped_in: "" }),
                crate::spec::Column {
                    header: "Payee",
                    quoted: true,
                    pad: Pad::Width(25),
                    role: Role::Description,
                },
                crate::spec::Column {
                    header: "Address",
                    quoted: true,
                    pad: Pad::Width(18),
                    role: Role::Extra { what: "address" },
                },
                col("Amount", OUT_NEGATIVE),
            ],
            "MM/DD/YYYY",
        )
    };
    let mut l = base(
        "bofa_card_csv",
        "Bank of America",
        &["credit_card"],
        "csv",
        prov(
            "high",
            &[
                "https://github.com/christoomey/vim-kata-meetup/blob/HEAD/_csv_cleanup/before.txt",
                "https://github.com/ryokather/FinanceTracker/blob/HEAD/sampleTransactions/account2_mayTrans.csv",
                "https://github.com/mschilli/go-ynabler/blob/HEAD/bofacc.go",
            ],
            "Real downloads. Payee and Address quoted and space-padded, so readers must trim. Real reference \
             numbers are 17-23 digits; here they are TD.1's ids. Purchases negative.",
        ),
        Format::Csv(spec),
    );
    l.extra_of = extras;
    l
}

fn wells_legacy() -> Layout {
    let spec = CsvSpec {
        quote_all: true,
        quote_empty: true,
        has_header: false,
        line_ending: "crlf",
        ..csv_spec(
            vec![
                col("", Role::PostedDate),
                col("", OUT_NEGATIVE),
                col("", Role::Fixed { value: "*" }),
                col("", Role::CheckNumber),
                col("", Role::Description),
            ],
            "MM/DD/YYYY",
        )
    };
    base(
        "wells_fargo_legacy_csv",
        "Wells Fargo",
        &["checking", "savings"],
        "csv",
        prov(
            "high",
            &[
                "https://github.com/simonmichael/hledger/blob/master/examples/csv/banking/wellsfargo-checking.csv.rules",
                "https://github.com/firefly-iii/import-configurations/blob/main/us/wellsfargo/default.json",
                "https://github.com/diegosol127/FinanceTools/tree/HEAD/Statements/WellsFargo/2024",
            ],
            "The layout until about 2025: no header; every field quoted; date, signed amount, `*`, check \
             number, description. Line ending not confirmed (CRLF used).",
        ),
        Format::Csv(spec),
    )
}

fn wells_2026() -> Layout {
    let spec = CsvSpec {
        quote_all: true,
        quote_header: true,
        line_ending: "crlf",
        ..csv_spec(
            vec![
                col("DATE", Role::PostedDate),
                col("DESCRIPTION", Role::Description),
                col("AMOUNT", OUT_NEGATIVE),
                col("CHECK #", Role::CheckNumber),
                col(
                    "STATUS",
                    Role::Status {
                        pending: "Pending",
                        posted: "Posted",
                    },
                ),
            ],
            "MM/DD/YYYY",
        )
    };
    base(
        "wells_fargo_csv",
        "Wells Fargo",
        &["checking", "savings"],
        "csv",
        prov(
            "high",
            &[
                "https://github.com/ddgarrett/load_statements/blob/HEAD/notes/v000.000.001_chg_wells_fargo_download_headings.md",
                "https://github.com/pdeepanraj/expensefiles/blob/HEAD/files/2026/May/WellsFargo_May_old.csv",
                "https://www.wellsfargo.com/help/online-banking/activity-faqs/",
            ],
            "The 2026 layout: a quoted header row; values quoted except empty ones (an empty check number is \
             `,,`). STATUS seen as `Posted`; the pending value is assumed, so this corpus lists posted rows only.",
        ),
        Format::Csv(spec),
    )
}

fn citi_card() -> Layout {
    let spec = csv_spec(
        vec![
            col(
                "Status",
                Role::Status {
                    pending: "Pending",
                    posted: "Cleared",
                },
            ),
            col("Date", Role::TransactionDate),
            qcol("Description", Role::Description),
            col("Debit", Role::Debit),
            col(
                "Credit",
                Role::Credit {
                    negative_printed: true,
                },
            ),
            col(
                "Member Name",
                Role::Extra {
                    what: "holder_upper",
                },
            ),
        ],
        "MM/DD/YYYY",
    );
    let mut l = base(
        "citi_card_csv",
        "Citi",
        &["credit_card"],
        "csv",
        prov(
            "high",
            &[
                "https://github.com/ste900811/creditCardParser/blob/HEAD/statements/stevenLu/Citi/strataElite/251119.CSV",
                "https://github.com/pdeepanraj/expensefiles/blob/HEAD/files/2025/Citi_Costco_OCT25.CSV",
                "https://github.com/firefly-iii/import-configurations/blob/main/us/citi/default.json",
            ],
            "Real downloads. Purchases are positive in Debit; credits are printed as NEGATIVE numbers in the \
             Credit column. Status is Cleared or Pending, and pending rows are included. One date column. \
             Older files have no Member Name column.",
        ),
        Format::Csv(spec),
    );
    l.includes_pending = true;
    l.extra_of = extras;
    l
}

fn capone_card() -> Layout {
    let spec = CsvSpec {
        line_ending: "crlf",
        ..csv_spec(
            vec![
                col("Transaction Date", Role::TransactionDate),
                col("Posted Date", Role::PostedDate),
                col("Card No.", Role::AccountNumber),
                col("Description", Role::Description),
                col("Category", Role::BankCategory),
                col("Debit", Role::Debit),
                col(
                    "Credit",
                    Role::Credit {
                        negative_printed: false,
                    },
                ),
            ],
            "YYYY-MM-DD",
        )
    };
    let mut l = base(
        "capital_one_card_csv",
        "Capital One",
        &["credit_card"],
        "csv",
        prov(
            "high",
            &[
                "https://github.com/simonmichael/hledger/blob/master/examples/csv/banking/capital-one.csv.rules",
                "https://github.com/reubano/csv2ofx/blob/master/csv2ofx/mappings/capitalone.py",
                "https://github.com/pythoninoffice/tutorials/blob/HEAD/sample_data/cc_statement_short.csv",
            ],
            "ISO dates, unsigned Debit and Credit columns, the card's last 4 digits. CRLF. Category values are \
             Capital One's names, mapped from our categories.",
        ),
        Format::Csv(spec),
    );
    l.category_of = capone_category;
    l
}

fn capone_360() -> Layout {
    let spec = csv_spec(
        vec![
            col("Account Number", Role::AccountNumber),
            col("Transaction Description", Role::Description),
            col("Transaction Date", Role::PostedDate),
            col(
                "Transaction Type",
                Role::Indicator {
                    debit: "Debit",
                    credit: "Credit",
                },
            ),
            col("Transaction Amount", Role::AbsAmount),
            col("Balance", Role::Balance),
        ],
        "MM/DD/YY",
    );
    base(
        "capital_one_360_csv",
        "Capital One",
        &["checking", "savings"],
        "csv",
        prov(
            "high",
            &[
                "https://github.com/Mantheous/a_fish_in_sea/blob/HEAD/lib/data/2025-10-11_AshtonChecking...9371.csv",
                "https://github.com/tomthehare/moneypit/blob/HEAD/parsers/parsers.py",
            ],
            "Capital One 360 checking and savings: 2-digit years; unsigned amounts with the direction in \
             Transaction Type (Debit or Credit).",
        ),
        Format::Csv(spec),
    )
}

fn us_bank() -> Layout {
    let spec = CsvSpec {
        quote_all: true,
        quote_header: true,
        quote_empty: true,
        ..csv_spec(
            vec![
                col("Date", Role::PostedDate),
                col(
                    "Transaction",
                    Role::Extra {
                        what: "usbank_transaction",
                    },
                ),
                col("Name", Role::Description),
                col("Memo", Role::Memo),
                col("Amount", OUT_NEGATIVE),
            ],
            "YYYY-MM-DD",
        )
    };
    let mut l = base(
        "us_bank_csv",
        "U.S. Bank",
        &["checking", "savings", "credit_card"],
        "csv",
        prov(
            "medium",
            &[
                "https://github.com/daquino/usbank-es-loader/blob/HEAD/doc/sample.csv",
                "https://github.com/Ledger-Software/Ledger/blob/HEAD/src/test/resources/USBankSample.csv",
                "https://github.com/dyer440/dyerempire/blob/HEAD/lib/bank-import.ts",
                "https://www.usbank.com/customer-service/knowledge-base/KB0069323.html",
            ],
            "Every field quoted, header included. Transaction is DEBIT, CREDIT or the check number. This is \
             the newer era (ISO dates, 2 decimals) reported by importer projects in 2026; older files use \
             M/D/YYYY and 4-decimal amounts and put a reference in Memo.",
        ),
        Format::Csv(spec),
    );
    l.extra_of = |what, row, ctx| match (what, row.kind) {
        ("usbank_transaction", Kind::Check) => row.check_number.clone().unwrap_or_default(),
        ("usbank_transaction", _) => if row.amount < 0 { "DEBIT" } else { "CREDIT" }.to_string(),
        _ => extras(what, row, ctx),
    };
    l
}

fn pnc() -> Layout {
    let spec = CsvSpec {
        quote_all: true,
        line_ending: "crlf",
        amount_format: DOLLARS,
        ..csv_spec(
            vec![
                col("Date", Role::PostedDate),
                col("Description", Role::Description),
                col("Withdrawals", Role::Debit),
                col(
                    "Deposits",
                    Role::Credit {
                        negative_printed: false,
                    },
                ),
                col("Balance", Role::Balance),
            ],
            "MM/DD/YYYY",
        )
    };
    base(
        "pnc_csv",
        "PNC",
        &["checking", "savings"],
        "csv",
        prov(
            "medium",
            &[
                "https://github.com/johannesgerer/buchhaltung/blob/HEAD/examples_configurations/nick/pnc.csv",
            ],
            "From a 2017 sample: unquoted header, every data field quoted, amounts with `$` and thousands \
             separators, withdrawals positive. Not confirmed for the current site.",
        ),
        Format::Csv(spec),
    )
}

fn truist() -> Layout {
    let spec = csv_spec(
        vec![
            col("Posted Date", Role::PostedDate),
            col("Transaction Date", Role::TransactionDate),
            col("Transaction Type", Role::Type),
            col("Check/Serial #", Role::CheckNumber),
            col("Full description", Role::Description),
            col("Merchant name", Role::Payee),
            col("Category name", Role::BankCategory),
            col("Sub-category name", Role::BankSubcategory),
            col("Amount", OUT_NEGATIVE),
            col("Daily Posted Balance", Role::Balance),
        ],
        "MM/DD/YYYY",
    );
    let mut l = base(
        "truist_csv",
        "Truist",
        &["checking", "savings"],
        "csv",
        prov(
            "medium",
            &[
                "https://github.com/Derekdog689/thrive-stability-platform/blob/HEAD/scripts/dry_run_january_2025_ingestion.py",
                "https://excel-checkbook.com/exporting-transactions-from-a-truist-bank-account-to-import-load-into-the-excel-checkbook-register/",
            ],
            "The layout Truist introduced at the end of 2025 (column names from parser code; quoting not \
             confirmed). Merchant name is the clean merchant. Category values are reconstructed from our own \
             category names. Daily Posted Balance is rendered as the running balance after each row.",
        ),
        Format::Csv(spec),
    );
    l.type_of = |k, _| match k {
        Kind::Purchase => "Debit Card",
        Kind::Refund => "Debit Card Credit",
        Kind::AchDebit | Kind::CardPaymentOut | Kind::P2pOut => "ACH Debit",
        Kind::AchCredit | Kind::P2pIn | Kind::InterestEarned => "ACH Credit",
        Kind::Check => "Check",
        Kind::TransferIn | Kind::TransferOut => "Transfer",
        Kind::Atm => "ATM Withdrawal",
        _ => "Fee",
    };
    l.category_of = truist_category;
    l.extra_of = extras;
    l
}

fn amex_basic() -> Layout {
    let spec = csv_spec(
        vec![
            col("Date", Role::TransactionDate),
            col("Description", Role::Description),
            col("Amount", OUT_POSITIVE),
        ],
        "MM/DD/YYYY",
    );
    base(
        "amex_csv",
        "American Express",
        &["credit_card"],
        "csv",
        prov(
            "high",
            &[
                "https://github.com/saifeemustafaq/creditspendanalyser/blob/HEAD/Amex%20may.csv",
                "https://github.com/mschilli/go-ynabler/blob/HEAD/README.md",
                "https://github.com/laleck/beancount_examples/blob/HEAD/importers/amex.py",
            ],
            "The basic download. Charges are POSITIVE; payments and credits negative. Descriptions keep Amex's \
             internal space padding.",
        ),
        Format::Csv(spec),
    )
}

fn amex_extended() -> Layout {
    let spec = csv_spec(
        vec![
            col("Date", Role::TransactionDate),
            col("Description", Role::Description),
            col(
                "Card Member",
                Role::Extra {
                    what: "holder_upper",
                },
            ),
            col(
                "Account #",
                Role::Extra {
                    what: "account_dash",
                },
            ),
            col("Amount", OUT_POSITIVE),
            col(
                "Extended Details",
                Role::Extra {
                    what: "extended_details",
                },
            ),
            col(
                "Appears On Your Statement As",
                Role::Extra {
                    what: "statement_as",
                },
            ),
            col("Address", Role::Extra { what: "street" }),
            col("City/State", Role::Extra { what: "city_state" }),
            col("Zip Code", Role::Extra { what: "zip" }),
            col("Country", Role::Extra { what: "country" }),
            col("Reference", Role::Reference { wrapped_in: "'" }),
            col("Category", Role::BankCategory),
        ],
        "MM/DD/YYYY",
    );
    let mut l = base(
        "amex_extended_csv",
        "American Express",
        &["credit_card"],
        "csv",
        prov(
            "high",
            &[
                "https://github.com/jmcameron/amex2qif",
                "https://github.com/laleck/beancount_examples/blob/HEAD/importers/amex.py",
            ],
            "The 'include additional details' download. Extended Details, Address and City/State contain \
             line breaks inside quotes. Reference is wrapped in single quotes. Charges positive. Category is \
             `Group-Subgroup` (mapped from our categories).",
        ),
        Format::Csv(spec),
    );
    l.extra_of = |what, row, ctx| match what {
        "statement_as" => row.description.clone(),
        _ => extras(what, row, ctx),
    };
    l.category_of = amex_category;
    l
}

fn discover() -> Layout {
    let spec = CsvSpec {
        line_ending: "crlf",
        ..csv_spec(
            vec![
                col("Trans. Date", Role::TransactionDate),
                col("Post Date", Role::PostedDate),
                qcol("Description", Role::Description),
                col("Amount", OUT_POSITIVE),
                qcol("Category", Role::BankCategory),
            ],
            "MM/DD/YYYY",
        )
    };
    let mut l = base(
        "discover_csv",
        "Discover",
        &["credit_card"],
        "csv",
        prov(
            "high",
            &[
                "https://github.com/amkchari/ExpenseManager/blob/HEAD/statements/discover-new-2024.csv",
                "https://github.com/redstreet/beancount_reds_importers/blob/main/beancount_reds_importers/importers/discover/__init__.py",
            ],
            "Real download. Purchases are POSITIVE, payments negative (some articles say otherwise; real \
             files don't). CRLF; Description and Category quoted. Discover dropped QFX downloads around 2022.",
        ),
        Format::Csv(spec),
    );
    l.category_of = discover_category;
    l
}

fn apple_card() -> Layout {
    let spec = csv_spec(
        vec![
            col("Transaction Date", Role::TransactionDate),
            col("Clearing Date", Role::PostedDate),
            qcol("Description", Role::Description),
            qcol("Merchant", Role::Payee),
            qcol("Category", Role::BankCategory),
            qcol("Type", Role::Type),
            qcol("Amount (USD)", OUT_POSITIVE),
            qcol("Purchased By", Role::Extra { what: "holder" }),
        ],
        "MM/DD/YYYY",
    );
    let mut l = base(
        "apple_card_csv",
        "Apple Card",
        &["credit_card"],
        "csv",
        prov(
            "medium",
            &[
                "https://github.com/ek68794998/tally-budgeting/blob/HEAD/packages/utilities/src/dataProviders/__mocks__/appleCardStatementMock.csv",
                "https://support.apple.com/en-lb/102284",
                "https://www.macrumors.com/how-to/export-apple-card-data-to-spreadsheet/",
            ],
            "Issued by Goldman Sachs. Dates unquoted, every other field quoted, the amount included. \
             Purchases positive. Merchant is the clean merchant name. Exported per monthly statement or for a \
             date range.",
        ),
        Format::Csv(spec),
    );
    l.type_of = |k, _| match k {
        Kind::Purchase => "Purchase",
        Kind::CardPaymentIn => "Payment",
        Kind::Refund => "Credit",
        Kind::InterestCharged => "Interest",
        _ => "Debit",
    };
    l.category_of = apple_category;
    l.extra_of = extras;
    l
}

fn ally() -> Layout {
    let spec = csv_spec(
        vec![
            col("Date", Role::PostedDate),
            col(" Time", Role::Extra { what: "time" }),
            col(" Amount", OUT_NEGATIVE),
            col(" Type", Role::Type),
            col(" Description", Role::Description),
        ],
        "YYYY-MM-DD",
    );
    let mut l = base(
        "ally_csv",
        "Ally Bank",
        &["checking", "savings"],
        "csv",
        prov(
            "high",
            &["https://github.com/davydog187/finances/blob/HEAD/exports/ally_spending.csv"],
            "Real download. The header has a space after each comma (so the names start with a space); data \
             rows don't. ISO date plus a time column.",
        ),
        Format::Csv(spec),
    );
    l.type_of = |_, row| {
        if row.amount < 0 {
            "Withdrawal"
        } else {
            "Deposit"
        }
    };
    l.extra_of = extras;
    l
}

fn schwab() -> Layout {
    let spec = CsvSpec {
        quote_all: true,
        quote_header: true,
        quote_empty: true,
        amount_format: DOLLARS,
        ..csv_spec(
            vec![
                col("Date", Role::PostedDate),
                col(
                    "Status",
                    Role::Status {
                        pending: "Pending",
                        posted: "Posted",
                    },
                ),
                col("Type", Role::Type),
                col("CheckNumber", Role::CheckNumber),
                col("Description", Role::Description),
                col("Withdrawal", Role::Debit),
                col(
                    "Deposit",
                    Role::Credit {
                        negative_printed: false,
                    },
                ),
                col("RunningBalance", Role::Balance),
            ],
            "MM/DD/YYYY",
        )
    };
    let mut l = base(
        "schwab_checking_csv",
        "Charles Schwab Bank",
        &["checking", "savings"],
        "csv",
        prov(
            "high",
            &[
                "https://github.com/redstreet/beancount_reds_importers/blob/main/beancount_reds_importers/importers/schwab/schwab_csv_checking.py",
                "https://github.com/reubano/csv2ofx/blob/master/csv2ofx/mappings/schwabchecking.py",
            ],
            "Every field quoted, amounts with `$`. Status is Posted or Pending, and pending rows are included \
             (no running balance yet). Type values ACH, TRANSFER and INTADJUST are confirmed; the others are \
             assumed. Older files use Credits/Debits/TransactionType and `#` comment lines.",
        ),
        Format::Csv(spec),
    );
    l.includes_pending = true;
    l.type_of = |k, _| match k {
        Kind::AchDebit | Kind::AchCredit | Kind::CardPaymentOut | Kind::P2pIn | Kind::P2pOut => {
            "ACH"
        }
        Kind::TransferIn | Kind::TransferOut => "TRANSFER",
        Kind::InterestEarned => "INTADJUST",
        Kind::Check => "CHECK",
        Kind::Atm => "ATM",
        Kind::Fee => "FEE",
        _ => "VISA",
    };
    l
}

fn usaa() -> Layout {
    let spec = csv_spec(
        vec![
            col("Date", Role::PostedDate),
            qcol("Description", Role::Payee),
            qcol("Original Description", Role::Description),
            col("Category", Role::BankCategory),
            col("Amount", OUT_NEGATIVE),
            col(
                "Status",
                Role::Status {
                    pending: "Pending",
                    posted: "Posted",
                },
            ),
        ],
        "YYYY-MM-DD",
    );
    let mut l = base(
        "usaa_csv",
        "USAA",
        &["checking", "savings"],
        "csv",
        prov(
            "high",
            &[
                "https://github.com/Topicet/LiquiLens/blob/HEAD/Data/September/bk_download(25).csv",
                "https://community.quicken.com/discussion/7929128/usaa-no-longer-uses-qfx-files-and-i-can-find-no-way-at-all-to-download-past-transactions",
            ],
            "Real download. Description is USAA's cleaned name (our payee); Original Description is the raw \
             text. Status is Posted or Pending, and pending rows are included. USAA no longer offers QFX. \
             Category values are USAA-style names mapped from our categories.",
        ),
        Format::Csv(spec),
    );
    l.includes_pending = true;
    l.category_of = usaa_category;
    l
}

fn navy_federal() -> Layout {
    let spec = CsvSpec {
        utf8_bom: true,
        ..csv_spec(
            vec![
                col("Posting Date", Role::PostedDate),
                col("Transaction Date", Role::TransactionDate),
                col("Amount", Role::AbsAmount),
                col(
                    "Credit Debit Indicator",
                    Role::Indicator {
                        debit: "Debit",
                        credit: "Credit",
                    },
                ),
                col("type", Role::Type),
                col("Type Group", Role::Extra { what: "nfcu_group" }),
                col("Reference", Role::Reference { wrapped_in: "" }),
                col("Instructed Currency", Role::Fixed { value: "" }),
                col("Currency Exchange Rate", Role::Fixed { value: "" }),
                col("Instructed Amount", Role::Fixed { value: "" }),
                col("Description", Role::Description),
                col("Category", Role::BankCategory),
                col("Check Serial Number", Role::CheckNumber),
                col(
                    "Card Ending",
                    Role::Extra {
                        what: "card_ending",
                    },
                ),
                col("Rewards Total", Role::Fixed { value: "" }),
                col("Rewards Type", Role::Fixed { value: "" }),
            ],
            "MM/DD/YYYY",
        )
    };
    let mut l = base(
        "navy_federal_csv",
        "Navy Federal Credit Union",
        &["checking", "savings", "credit_card"],
        "csv",
        prov(
            "high",
            &[
                "https://github.com/imid12/ImanHaamid_Solo_ITAI2376/blob/HEAD/Docs/Historical%20Spending%20-%20transactions.csv",
                "https://github.com/channerlbok/Home-Expense-Visualizer/blob/HEAD/home-expense-visualizer/expenses.csv",
                "https://github.com/tacticalpacket/purser/blob/HEAD/tests/test_fixture_provenance.py",
            ],
            "The 2026 layout: UTF-8 BOM, Rewards Total and Rewards Type at the end. Amount is unsigned; Credit \
             Debit Indicator gives the direction. Note the lowercase `type` header. Type, Type Group and \
             Category values are assumed.",
        ),
        Format::Csv(spec),
    );
    l.type_of = |k, _| match k {
        Kind::Purchase | Kind::Refund => "Debit Card",
        Kind::Check => "Check",
        Kind::TransferIn | Kind::TransferOut => "Transfer",
        Kind::Atm => "ATM",
        Kind::Fee => "Fee",
        Kind::InterestEarned => "Dividend",
        Kind::P2pIn | Kind::P2pOut => "Zelle",
        _ => "ACH",
    };
    l.category_of = nfcu_category;
    l.extra_of = extras;
    l
}

fn td_bank() -> Layout {
    let spec = CsvSpec {
        trim: true,
        ..csv_spec(
            vec![
                col("Date", Role::PostedDate),
                col("Bank RTN", Role::Extra { what: "routing" }),
                col("Account Number", Role::Extra { what: "number" }),
                col("Transaction Type", Role::Type),
                crate::spec::Column {
                    header: "Description",
                    quoted: false,
                    pad: Pad::Trailing(1),
                    role: Role::Description,
                },
                col("Debit", Role::Debit),
                col(
                    "Credit",
                    Role::Credit {
                        negative_printed: false,
                    },
                ),
                col("Check Number", Role::CheckNumber),
                col("Account Running Balance", Role::Balance),
            ],
            "YYYY-MM-DD",
        )
    };
    let mut l = base(
        "td_bank_csv",
        "TD Bank",
        &["checking", "savings"],
        "csv",
        prov(
            "medium",
            &[
                "https://github.com/lonelytango/swiss-finance/blob/HEAD/personal_finance/input/checking/td/1_2024.csv",
            ],
            "ISO dates, unsigned Debit and Credit, the routing and account number on every row (fake here). \
             Descriptions end with a trailing space, so readers must trim. Transaction Type values are assumed.",
        ),
        Format::Csv(spec),
    );
    l.type_of = |k, _| match k {
        Kind::Purchase => "DEBIT POS",
        Kind::Refund => "CREDIT POS",
        Kind::AchCredit | Kind::P2pIn | Kind::InterestEarned => "ACH DEPOSIT",
        Kind::Check => "CHECK",
        Kind::TransferIn | Kind::TransferOut => "TRANSFER",
        Kind::Atm => "ATM WITHDRAWAL",
        Kind::Fee => "SERVICE CHARGE",
        _ => "ACH DEBIT",
    };
    l.extra_of = extras;
    l
}

fn barclays() -> Layout {
    let spec = CsvSpec {
        skip_lines: 4,
        ..csv_spec(
            vec![
                col("Transaction Date", Role::TransactionDate),
                qcol("Description", Role::Description),
                qcol("Category", Role::Type),
                col("Amount", OUT_NEGATIVE),
            ],
            "MM/DD/YYYY",
        )
    };
    let mut l = base(
        "barclays_csv",
        "Barclays",
        &["credit_card"],
        "csv",
        prov(
            "high",
            &[
                "https://github.com/johannesgerer/buchhaltung/blob/HEAD/examples_configurations/nick/barclaycardus.csv",
                "https://github.com/velicanu/expenses/blob/HEAD/tests/test_common.py",
                "https://github.com/taimoorq/expense_tracker/blob/HEAD/test/fixtures/files/account_activity/preamble_card_activity.csv",
            ],
            "Barclays US cards. Four lines before the header (bank name, masked account, balance, a line with \
             a single space). Its Category column is really DEBIT or CREDIT. Purchases negative. A possible \
             2026 variant adds card digits and 'Purchased by' (not rendered).",
        ),
        Format::Csv(spec),
    );
    l.type_of = |_, row| if row.amount < 0 { "DEBIT" } else { "CREDIT" };
    l.preamble = |_, ctx| {
        let d = ctx.as_of;
        const MONTHS: [&str; 12] = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ];
        vec![
            "Barclays Bank Delaware".to_string(),
            format!(
                "Account Number: XXXXXXXXXXXX{}",
                ctx.account.masked_number.as_deref().unwrap_or("")
            ),
            format!(
                "Account Balance as of {} {:02} {}:    ${}",
                MONTHS[usize::from(d.month()) - 1],
                d.day(),
                d.year(),
                fmt_amount(-ctx.balance_as_of, &PLAIN)
            ),
            " ".to_string(),
        ]
    };
    l.newest_first = false;
    l
}
