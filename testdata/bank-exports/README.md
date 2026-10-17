# Bank export corpus (TD.2)

These are the files US banks and card issuers let customers download (CSV in each institution's own layout, OFX and QFX), rendered from TD.1's synthetic ledgers (`testdata/synthetic`). Every file comes with the output an importer should produce from it. The CSV importer (T1.1), the OFX/QFX importer (T1.2) and the import pipeline (T1.3) are measured against this corpus.

**23 institutions, 41 layouts, 50 files (30 CSV, 19 QFX, 1 OFX), 8,487 rows.**

Everything is synthetic. The transactions are TD.1's invented households. Account and routing numbers are fake: account numbers start with `9999`, and routing numbers start with `999999`, which no real ABA routing number does. Institution identifiers (OFX `ORG`, `FID`, `INTU.BID`) are the public ones, because importers use them to recognise the bank.

## Running it

This folder is its own Cargo workspace, like `testdata/synthetic`. Run everything from here:

```sh
cargo run --release              # regenerate data/ from TD.1's committed ledgers
cargo run --release -- --check   # exit 1 if data/ differs from a fresh run
cargo test --release             # the checks below
```

The corpus is rendered from TD.1's committed files (`../synthetic/data`), not from its generator. `manifest.json` records the hash of TD.1's manifest that it was built from. If TD.1's data changes, run `cargo run --release` here too.

## Files

```
data/
  layouts.json                         the spec of every layout (below)
  manifest.json                        every file: layout, institution, format, persona, row count, SHA-256
  <institution>/<name>.<csv|qfx|ofx>   the download
  <institution>/<name>.<ext>.expected.json   what an importer should produce from it
```

### `layouts.json`: the spec

Each layout says where it came from and exactly how its files are built. Importers can treat it as their spec.

- `provenance`: the public `sources` the layout was checked against and a `confidence`:
  - `high`: matches real downloads or the bank's own documentation.
  - `medium`: matches importer code or an older sample.
  - `reconstructed`: no public sample was found (see below).
  - `notes` lists quirks, and variants that are known but not rendered.
- `newest_first`, `includes_pending`: the row order, and whether pending transactions are listed.
- `format.csv`:
  - Delimiter and quoting: `quote_all`, `quote_header`, `quote_empty`, and per-column `quoted`.
  - `line_ending`, `utf8_bom`.
  - `skip_lines` (lines before the header) and `has_header`.
  - `date_format`.
  - `amount_format`: thousands separators, `$`, negative style.
  - `skip_rows_without_amount` (summary rows such as Bank of America's opening balance).
  - `trailing_delimiter`: `rows` means data rows have one more field than the header.
  - `trim`: fields are space-padded, so readers must trim.
  - `columns`: each column's header text and **role** (below).
- `format.ofx`:
  - `version` (`102` is 1.0.2 SGML; `202` and `220` are XML) and `qfx` (Quicken Web Connect, adds `INTU.BID`).
  - Identifiers: `org`, `fid`, `intu_bid`.
  - Tag layout: `close_leaf_tags`, `line_per_element`.
  - Dates: the time-zone suffix (`tz`), `date_only_postings`, `dtuser`, and a fixed `dtstart` where a bank always sends one.
  - Text: `name_max` (NAME length), how text is split between `NAME` and `MEMO` (`text`, `memo`).
  - Header quirks: `leading_blank_line`, `header_fileuids`.
  - Other quirks: `card_as_creditline`, `language`, `severity`.

Column roles map to fields of the expected output:

| Role | Field |
|---|---|
| `posted_date` | `posted_date` |
| `transaction_date` | `transaction_date`. If a layout has only a transaction-date column, `posted_date` is that date too. |
| `description` | `description`, exactly as written (after trimming, if the layout says `trim`) |
| `payee` | `payee`: a bank's cleaned merchant name (USAA's Description, Apple's and Truist's Merchant) |
| `memo` | `memo` |
| `amount` | `amount`. `money_out: positive` means purchases are positive (Amex, Discover, Apple Card), so negate them. |
| `debit` / `credit` | Money out and money in. Take the absolute value: Citi prints credits as negative numbers. |
| `abs_amount` + `indicator` | An unsigned amount plus a direction column (Capital One 360, Navy Federal) |
| `balance` | `balance` |
| `check_number` | `check_number` |
| `reference` | `external_id`, without the `wrapped_in` characters (Amex wraps it in `'`) |
| `status` | `status`: `pending` when the cell holds the layout's pending value |
| `bank_category` | `bank_category` |
| `type`, `bank_subcategory`, `account_number`, `fixed`, `extra` | Informational; not in the expected output |

For OFX/QFX, the fields map like this:
- `posted_date` ← `DTPOSTED`; `transaction_date` ← `DTUSER`; `amount` ← `TRNAMT`.
- `external_id` ← `FITID`; `check_number` ← `CHECKNUM`.
- `description` ← `NAME`, or `MEMO` when there is no `NAME` (Capital One 360); `memo` ← `MEMO`.
- At account level: `number` ← `ACCTID`, `routing` ← `BANKID`, `ledger_balance` ← `LEDGERBAL/BALAMT`.
- Values are unescaped (`&amp;` becomes `&`) and have no trailing spaces.

### `<file>.expected.json`

```json
{"file": "citi/card_2025_06_16.csv", "layout": "citi_card_csv", "institution": "Citi",
 "source": {"persona": "renter", "account_id": 4},
 "as_of": "2025-06-16", "start": "2025-05-17", "end": "2025-06-16",
 "account": {"kind": "credit_card", "currency": "USD", "number": null, "routing": null, "ledger_balance": null},
 "transactions": [
   {"truth_id": 739, "status": "pending", "posted_date": "2025-06-16", "transaction_date": "2025-06-16",
    "amount": {"minor": -639, "currency": "USD"}, "description": "SQ *DAILY GRIND      SAN FRANCISCO       CA",
    "payee": null, "memo": null, "external_id": null, "check_number": null, "balance": null, "bank_category": null},
   …
 ]}
```

- Amounts follow fm-core's rule: signed cents, **negative means money out of the account**, for cards too, whatever sign the file uses.
- Transactions are listed in file order.
- `truth_id` is the TD.1 transaction the row came from. Join it with `../synthetic/data/<persona>/truth.json` for its category, merchant, transfer pair and recurring series. It is not in the file; importers don't produce it.
- `account.number`, `routing` and `ledger_balance` are set when the file states them (OFX).

## Institutions and layouts

| Institution | Layout | Format | Confidence | Files |
|---|---|---|---|---|
| Chase | `chase_card_csv` | CSV | high | 3 |
| Chase | `chase_checking_csv` | CSV | high | 1 |
| Chase | `chase_card_qfx` | QFX 1.0.2 SGML | high | 1 |
| Chase | `chase_checking_qfx` | QFX 1.0.2 SGML | high | 1 |
| Bank of America | `bofa_checking_csv` | CSV | high | 2 |
| Bank of America | `bofa_card_csv` | CSV | high | 1 |
| Wells Fargo | `wells_fargo_legacy_csv` | CSV (no header) | high | 1 |
| Wells Fargo | `wells_fargo_csv` | CSV | high | 1 |
| Wells Fargo | `wells_fargo_qfx` | QFX 1.0.2 SGML | high | 2 |
| Citi | `citi_card_csv` | CSV | high | 4 |
| Citi | `citi_qfx` | QFX 1.0.2 SGML | medium | 1 |
| Capital One | `capital_one_card_csv` | CSV | high | 1 |
| Capital One | `capital_one_360_csv` | CSV | high | 1 |
| Capital One | `capital_one_card_qfx` | QFX 1.0.2 SGML | medium | 1 |
| Capital One | `capital_one_360_qfx` | QFX 2.0.2 XML | high | 1 |
| U.S. Bank | `us_bank_csv` | CSV | medium | 1 |
| U.S. Bank | `us_bank_qfx` | QFX 1.0.2 SGML | high | 1 |
| PNC | `pnc_csv` | CSV | medium | 1 |
| PNC | `pnc_qfx` | QFX 1.0.2 SGML | high | 1 |
| Truist | `truist_csv` | CSV | medium | 1 |
| American Express | `amex_csv` | CSV | high | 1 |
| American Express | `amex_extended_csv` | CSV | high | 1 |
| American Express | `amex_qfx` | QFX 1.0.2 SGML | high | 1 |
| Discover | `discover_csv` | CSV | high | 1 |
| Apple Card | `apple_card_csv` | CSV | medium | 1 |
| Apple Card | `apple_card_qfx` | QFX 1.0.2 SGML | high | 1 |
| Ally Bank | `ally_csv` | CSV | high | 1 |
| Ally Bank | `ally_qfx` | QFX 1.0.2 SGML | high | 1 |
| Charles Schwab Bank | `schwab_checking_csv` | CSV | high | 2 |
| USAA | `usaa_csv` | CSV | high | 2 |
| Navy Federal Credit Union | `navy_federal_csv` | CSV | high | 1 |
| Navy Federal Credit Union | `navy_federal_ofx` | OFX 2.2 XML | high | 1 |
| TD Bank | `td_bank_csv` | CSV | medium | 1 |
| Barclays | `barclays_csv` | CSV | high | 1 |
| Barclays | `barclays_qfx` | QFX 1.0.2 SGML | high | 1 |
| Synchrony | `synchrony_qfx` | QFX 1.0.2 SGML | medium | 1 |
| Fifth Third | `fifth_third_qfx` | QFX 1.0.2 SGML | reconstructed | 1 |
| Citizens | `citizens_qfx` | QFX 1.0.2 SGML | reconstructed | 1 |
| KeyBank | `keybank_qfx` | QFX 1.0.2 SGML | reconstructed | 1 |
| Huntington | `huntington_qfx` | QFX 1.0.2 SGML | reconstructed | 1 |
| Regions | `regions_qfx` | QFX 1.0.2 SGML | reconstructed | 1 |

**Confirmed vs reconstructed.** 18 institutions are backed by real downloads or importer code. For the other five (Fifth Third, Citizens, KeyBank, Huntington, Regions) no public sample or CSV layout was found. The Quicken FI directory confirms they offer a Web Connect download, so they get a QFX in the common Web Connect shape, marked `reconstructed`. Don't treat those five as evidence of the banks' real quirks.

**Not in the corpus, on purpose:**
- **M&T Bank** offers Quicken Direct Connect only, with no file to download.
- **Bank of America** ended QFX/OFX downloads on 2025-09-30 (CSV only here).
- **Discover** (about 2022) and **USAA** dropped QFX.
- **Synchrony** has no public CSV sample.

## What the corpus exercises

- **Sign conventions.** Purchases negative (most), purchases positive (Amex, Discover, Apple Card), separate debit and credit columns (Capital One, PNC, Schwab, TD), Citi's negative credits, unsigned amounts with an indicator column (Capital One 360, Navy Federal).
- **File shapes.** No header (Wells Fargo legacy); summary blocks before the header (Bank of America, Barclays); a header with leading spaces (Ally); a trailing comma on data rows only (Chase checking).
- **Quoting and encoding.** Every field quoted (Wells, U.S. Bank, Schwab, PNC); line breaks inside quoted fields and `'`-wrapped references (Amex extended); a UTF-8 BOM (Navy Federal); padded fields (Bank of America card, TD Bank).
- **Formats.** Dates as `MM/DD/YYYY`, `MM/DD/YY` and ISO. Amounts plain, with thousands separators, and with `$`.
- **OFX variants.**
  - Formats: SGML with and without closing tags; XML 2.0.2 and 2.2; single-line and one-tag-per-line.
  - Time zones: `[0:GMT]`, `[-4:EDT]`, `[-7:MST]`, `[-8:PST]`, or none. Some banks send dates with no time.
  - Text: NAME cut at 32 characters with MEMO overflow; MEMO-only; NAME and MEMO both set.
  - Statement shape: a card sent as a bank `CREDITLINE` statement (Citi); an epoch `DTSTART` (PNC); `&amp;` escaping (`AT&T`).
- **Overlap and pending (for T1.3).**
  - Overlapping exports of the same account, as CSV against CSV and CSV against QFX:
    - Chase Freedom: H1 and Q2–Q3 CSVs, plus a full-year QFX.
    - Wells Fargo checking: the legacy CSV and a QFX.
  - Mid-cycle downloads that list pending purchases at their authorised amount (before the tip, or a $175 fuel hold), followed by a later download where the same purchase has posted at its final amount. These come in pairs: Citi ×2, Schwab, USAA. Some pending amounts differ from the posted ones (tips added).
  - Importing all of them must produce no duplicates.

## Simplifications

- **Descriptions come from TD.1.** Each row keeps the description text of its TD.1 account, which imitates that account's own bank. Only the file layout is the exporting institution's, because accounts are reused across institutions. Importers mustn't depend on description style.
- **Bank category names** (Chase, Capital One, Discover, Amex, Apple, USAA, Navy Federal) are each bank's vocabulary, mapped from our categories. They are plausible but not an exhaustive list. Truist's category columns use our own category names (reconstructed).
- **Balances** use an opening balance chosen so the account never drops below $500. They show how balance columns work and aren't meant to match anything.
- **Reference numbers and FITIDs** are TD.1's 16-character ids, not each bank's own id format.
- Quirks known but not rendered are listed in each layout's `notes`, for example U.S. Bank's padded NAME and Amex's proprietary OFX tags.

## Checks (`tests/corpus.rs`)

- **A reference reader.** A small CSV/OFX reader in the test parses every file using only its `layouts.json` entry and must reproduce the expected output field for field. This proves the files, the expected output and the spec agree, and that the spec is enough to write an importer from.
- **Every expected row matches TD.1.**
  - Posted rows have the same account, amount and date as the TD.1 transaction.
  - Pending rows are transactions that had not posted by `as_of`.
  - No posted transaction in the date range is missing.
  - No transaction appears twice in a file.
- **Coverage.** At least 20 institutions; CSV, OFX and QFX all present; every layout used; account and routing numbers fake.
- **Overlap and pending.** At least 3 overlapping pairs, and at least 5 distinct purchases pending in one download and posted in a later one.
- **Freshness.** The committed `data/` is byte-identical to a fresh render.

## Changing the corpus

1. Edit a layout in `src/layouts/` or the file list in `src/corpus.rs`.
2. Bump `CORPUS_VERSION` in `src/lib.rs` if the output changes.
3. Run `cargo run --release`, then `cargo test --release`.
4. Commit the code and `data/` together.

When a layout is confirmed or corrected against a new public sample, update its `provenance`.
