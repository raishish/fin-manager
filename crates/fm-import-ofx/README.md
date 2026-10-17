# fm-import-ofx

This crate reads OFX and QFX bank downloads into the shared, normalised records of [`fm-import-types`](../fm-import-types). It is a standalone Apache-2.0 library with no dependency on fm-core; the import pipeline (T1.3) maps its output into the ledger.

```rust
let file = fm_import_ofx::parse(&bytes)?;
for statement in &file.statements {          // one per account in the file
    for record in &statement.records {       // in file order
        // record.amount_minor: signed minor units, negative = money out
    }
}
```

## What it reads
- **Formats:**
  - OFX 1.x SGML (`DATA:OFXSGML`, version 102 and similar), with leaf elements closed or unclosed, one element per line or the whole document on one line.
  - OFX 2.x XML (`<?OFX ... VERSION="202"?>`, 2.0.2 through 2.2), with comments, CDATA, processing instructions and `<TAG/>`.
  - QFX is OFX with Quicken's `INTU.BID`.
  - Tag names are compared in upper case.
  - Truncated files are closed at the end, and stray closing tags are ignored.
- **Statements:** bank statements (`BANKMSGSRSV1/STMTTRNRS/STMTRS`) and card statements (`CREDITCARDMSGSRSV1/CCSTMTTRNRS/CCSTMTRS`), any number per file. Record `source_index` counts across the whole file. Investment and other message sets are skipped. A transaction set that holds only an error status returns `StatementFailed { code }`.
- **Text encoding:** UTF-8 (a BOM is dropped) when the bytes are valid UTF-8, otherwise Windows-1252, which is what `CHARSET:1252` files really are.

## Field mapping

| Record field | OFX |
|---|---|
| `posted_date` | `DTPOSTED`: the first 8 characters, as the bank wrote them |
| `transaction_date` | `DTUSER` |
| `amount_minor` | `TRNAMT`, in the statement's `CURDEF` currency. The sign is kept: OFX already uses negative for money out, cards included |
| `description` | `NAME`; if that is empty, `PAYEE/NAME`; if that is missing, `MEMO`; if all are missing, an empty string |
| `memo` | `MEMO` |
| `external_id` | `FITID` |
| `check_number` | `CHECKNUM` |
| `trn_type` | `TRNTYPE`, upper-cased |
| `status` | Always `Posted`: OFX statements list posted transactions |
| `payee`, `balance_minor`, `bank_category`, `original_*` | Not set: see "Not handled" |

| StatementInfo field | OFX |
|---|---|
| `account_kind` | `CCSTMTRS` → `CreditCard`; otherwise `ACCTTYPE`: `CHECKING`, `SAVINGS`, `MONEYMRKT`, `CREDITLINE` → `CreditCard` (see Citi below), anything else → `Other` |
| `currency` | `CURDEF`, upper-cased. Required, and must be a known ISO 4217 code |
| `account_number`, `routing` | `ACCTID`, `BANKID` |
| `ledger_balance_minor`, `ledger_balance_date`, `available_balance_minor` | `LEDGERBAL/BALAMT`, `LEDGERBAL/DTASOF`, `AVAILBAL/BALAMT` |
| `period_start`, `period_end` | `BANKTRANLIST/DTSTART`, `DTEND`. 1970 and earlier → `None` |
| `institution` | `SONRS/FI/ORG`, `FI/FID`, `SONRS/INTU.BID` |

**Values:** surrounding spaces are trimmed. Entities are decoded: `&amp; &lt; &gt; &quot; &apos; &nbsp;` and numeric references. A bare `&` that doesn't start an entity (`AT&T` in SGML) is kept as written.

**Amounts:** exact, with no floats.
- A decimal separator is `.` or `,` (OFX allows both). Thousands grouping isn't allowed.
- A missing leading zero (`-.50`) and fewer decimals than the currency has (`88.0`) are fine.
- More precision than the currency has is an error, unless the extra digits are zeros.

**Dates:** taken as the bank wrote them. The time and the `[-8:PST]` zone are ignored and never converted to UTC, because converting would move evening transactions onto the next day.

## Bank quirks (TD.2 `data/layouts.json`)
Every one of these is in the corpus test (`tests/corpus.rs`) or in a unit test (`src/tests.rs`):

| Quirk | Seen at | Handling |
|---|---|---|
| SGML without closing tags | Chase, Wells Fargo, Citi, U.S. Bank, PNC, Amex, Apple, Barclays, Synchrony | A tag followed by text is a leaf |
| SGML with closing tags | Capital One card, Ally (an SGML header over an XML-style body) | The closing tag is consumed with its leaf |
| XML 2.0.2 / 2.2 | Capital One 360, Navy Federal | The version comes from the `<?OFX?>` instruction |
| Whole document on one line | Amex, Apple Card, Ally, Barclays | The tokenizer doesn't depend on lines |
| Blank line before the header; no FILEUID lines | Chase; Ally | The header is scanned for `OFXHEADER`, not parsed positionally |
| `SEVERITY` written `Info`; `LANGUAGE` written `eng` | Ally; Barclays | These values aren't interpreted |
| `MEMO` only, no `NAME` | Capital One 360 | The description falls back to `MEMO` |
| `NAME` cut at 32 characters with the full text in `MEMO` | Wells Fargo, Fifth Third, Citizens, KeyBank, Huntington, Regions | `NAME` is the description; `MEMO` is kept as the memo |
| `NAME` and `MEMO` both set | Chase checking, Capital One card, Navy Federal | As above |
| Empty `NAME` | Synchrony (real files) | Treated as missing; the description falls back to `MEMO` |
| Padded `NAME` | U.S. Bank (real files) | Trimmed |
| A card as a bank `CREDITLINE` statement | Citi | Reported as `AccountKind::CreditCard` |
| Epoch `DTSTART` (1970-01-01) | PNC | `period_start` is `None` |
| Date-only `DTPOSTED`/`DTUSER` | Synchrony | Only the first 8 characters are read |
| `[0:GMT]`, `[-4:EDT]`, `[-7:MST]`, `[-8:PST]`, or no zone | various | Ignored; the date is as written |
| One-decimal amounts (`88.0`) | PNC (real files) | Accepted |
| `&amp;` escaping | Citi, Amex, Barclays, Chase card | Decoded (`AT&T`) |
| Proprietary tags (`ORIGIN.ID`, `REFNUM`), `CCACCTTO` inside `STMTTRN` | Amex, Capital One (real files) | Unknown elements are ignored |

## Not handled (on purpose)
- **Foreign currency.** An `STMTTRN/CURRENCY` aggregate in another currency means `TRNAMT` isn't in the account's currency. Converting it at `CURRATE` would need rounding, so the file is refused with `ForeignAmount`. `ORIGCURRENCY` gives only a rate, not the original amount, so `original_amount_minor` stays `None` rather than being computed.
- **Pending lists, investment, loan and bill-pay message sets** are skipped.
- `payee`, `balance_minor` and `bank_category` aren't per-transaction fields in OFX.

## Errors
`Error` says what is wrong and where (`MissingField { field: "DTPOSTED", record: 12 }`). It never includes amounts, descriptions or account numbers from the file (SPEC §7).
