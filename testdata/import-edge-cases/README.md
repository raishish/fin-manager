# Import edge cases (TD.4)

These are small, adversarial bank files for the import pipeline's matcher (T1.3). Each one is built so that a plausible matching rule gets it wrong. The TD.2 corpus checks that real-world layouts import; these check that the matcher decides correctly *which rows are the same transaction*.

- **17 cases over 29 files (14 CSV, 15 QFX), 172 rows.** Cases 01–07 came first (TD.4). Cases 08–13 (TD.5) mix formats for one account and put tricky pairs in the same file. 08 and 09 are imported in both orders: `a` and `b` are two cases over the same files.
- The files are written in the documented bank layouts (see `testdata/bank-exports/data/layouts.json`). They parse with `fm-import-csv` (each CSV is detected as the stated layout) and with `fm-import-ofx`.
- Everything is synthetic. Account numbers start with `9999`, routing numbers with `999999`, and the cardholder is the invented Sam Fakerson.

## How to use it

`expected.json` is the machine-readable answer key. **Each case is independent: import its files, in `import_order`, into an empty ledger.**

```json
{"version": 1, "cases": [{
  "id": "06_tipped_pending_same_price", "title": "…", "what_it_tests": "…",
  "target": "account",
  "accounts": [{"key": "card", "name": "Citi Custom Cash", "kind": "credit_card", "currency": "USD",
                "institution": "Citi", "last4": "6630"}],
  "files": [{"path": "cases/06_…/citi_card_2026_03_07.csv", "format": "csv", "layout": "citi_card_csv", "account": "card"}],
  "import_order": ["cases/06_…/citi_card_2026_03_07.csv", "cases/06_…/citi_card_2026_03_14.csv"],
  "transactions": [{"key": "dinner_mar06", "account": "card", "final_status": "posted", "amount_minor": -4800,
                    "transaction_date": "2026-03-06", "posted_date": "2026-03-06", "description": "TST* BLUE HERON BISTRO"}],
  "rows": [{"file": "cases/06_…/citi_card_2026_03_07.csv", "statement": 0, "row": 0, "transaction": "dinner_mar06",
            "outcome": "inserted", "amount_minor": -4000, "posted_date": "2026-03-06",
            "transaction_date": "2026-03-06", "status": "pending", "external_id": null}]
}]}
```

| Field | Meaning |
|---|---|
| `target` | `account`: import each file into its named account (`Target::Account`). `auto`: let the pipeline find or create the account (`Target::Auto`). Only case 04 uses `auto`. |
| `accounts` | The accounts the case's ledger must end with. Under `auto`, the files must resolve to exactly these. |
| `files[].account` | The account the file's rows belong to. |
| `transactions` | **The ground truth: the distinct transactions the ledger must end with**, each with its final status and amount. |
| `rows` | Every row of every file. `statement` and `row` are the positions in the parsed `Statement`s (the record's `source_index`). Each row also says which transaction it is. |
| `rows[].transaction` | Rows with the same key are the same transaction; rows with different keys are distinct. **This holds in any import order.** |
| `rows[].outcome` | The pipeline outcome when importing in `import_order`: `inserted`, `duplicate`, `posted_pending` (a posted row settles a pending one) or `pending_already_posted`. |
| `rows[].amount_minor`, dates, `status`, `external_id` | What the row parses to, so a test can find the row and see that a pending amount differs from the posted one. |

A useful T1.3 test, for each case:
1. Import in `import_order` and check every outcome.
2. Check the ledger holds exactly `transactions`.
3. Import again in reverse order into a fresh ledger and check the same transactions come out.

## The cases

| Case | Files | The trap | Rule it aims at (T1.3 `pipeline/README.md`) |
|---|---|---|---|
| `01_month_boundary_transit` | Chase card CSV, January and February | A daily $2.90 fare. Fares post one or two days after the ride, so neighbouring rides share a date. The 01/30 and 01/31 rides both post 02/01 and appear in both downloads (true duplicates). The 02/01 ride posts 02/02 and appears only in February. A round trip on 02/05 is two fares on the same day. | "Dates are shared when the date sets overlap": the 02/01 ride shares 02/01 with the existing 01/31 ride. |
| `02_generic_memos` | Credit-union QFX, a mid-month download and a full-month re-download | NAME is just `POS PURCHASE` or `ACH DEBIT`; the merchant is in MEMO. Three different merchants share $23.45 on 03/03, and three ACH debits share $110.00 on 03/05. The re-download **regenerates every FITID**, which some credit unions do, so duplicates can only be found by MEMO text. The third merchant on each day is new and is listed first. | "Descriptions and memos are both tried": equal generic NAMEs look compatible. |
| `03a_reused_fitid` | U.S. Bank QFX, February and an overlapping March | The bank reuses short FITIDs. `4471` is a $12.00 lunch on 02/10 and later a $45.50 dinner on 03/12. `4479` is a $20.00 fuel purchase and, on the same day in the re-download, a $25.00 parking charge. `4480` really is the same grocery row. | "Same `external_id` → duplicate" without checking amount or date. |
| `03b_nonunique_reference` | Bank of America card CSV, two downloads | A refund carries its purchase's Reference Number (the $199.99 TV and its return). Two $5.25 coffees on the same day at the same store have different references. A later coffee on 04/22 reuses an earlier reference. | A reference column treated as a unique id. |
| `04_same_last4_two_banks` | Chase QFX, Capital One QFX, Capital One CSV | Two cards that both end in 4821, at different banks, each with a Spotify charge of the same amount on the same day. The Capital One CSV prints only the last four. | `Target::Auto`'s "same last four at the same institution". The two QFXs must create two accounts; the CSV must join the Capital One one and duplicate its rows. |
| `05_two_statements_one_account` | PNC QFX holding two statements | One file with an April and a May statement for the same account. They overlap on 04/30 and repeat that row with the same FITID. Two $20.00 ATM withdrawals on 05/02 are two transactions. | "Rows of the same file never match each other": the 04/30 row is the same transaction twice, inside one file. |
| `06_tipped_pending_same_price` | Citi card CSV, two downloads | A dinner shows pending at $40.00 on 03/06 and settles at $48.00 with the tip. On 03/09 the same restaurant charges exactly $40.00 for takeout, and that row comes first in the second file. A $40.00 lunch on 03/03 is in both downloads. | "Ranked by exact amount first": the pending dinner must settle into the $48.00 row with its purchase date, not the exact-amount takeout. |
| `07a_purchase_date_only_with_dtuser` | Barclays QFX, then Barclays CSV of the same month | The CSV has only purchase dates. The QFX has posted dates and DTUSER. Two $36.50 ramen dinners: the 06/10 one posts on 06/12, which is the other's purchase date. | The CSV rows must match their QFX twins by purchase date, without crossing over on the shared 06/12. |
| `07b_purchase_date_only_posted_twin` | Barclays QFX without DTUSER, then Barclays CSV | A constructed variant: the QFX has posted dates only, two days after each purchase. No two rows share amount and merchant, so the pairing is unambiguous. | The CSV purchase-date rows should still match their posted twins. A shared-date rule alone finds no match here. |
| `08a/08b_mixed_formats_boundary` | Chase QFX for February (posted dates only, as Chase sends it), and a Chase CSV with purchase and posted dates from posted 02-27. Both orders. | A daily $2.90 fare. The windows overlap on posted 02-27 and 02-28: the two 02-26 rides and a payment are in both files. The 02-27 and 02-28 rides both post 03-01 and are only in the CSV, and their purchase dates fall on the posted dates of the QFX's 02-26 rides. | "Dates are shared": a CSV row's {purchase, posted} overlaps a QFX row's {posted}, so different rides merge. |
| `09a/09b_purchase_only_vs_posted_only` | Amex QFX (posted dates only), then the Amex CSV (one date column, the purchase date). Both orders. | A $4.75 coffee every day, posting two days later. The QFX ends at posted 04-15 and the CSV starts at purchase 04-14, so no coffee is in both files, though rows a day apart are within three days of each other. The only true twin is a $23.88 Amazon order (bought 04-14, posted 04-15), the one row within ±3 days with no lookalike. | A date window without uniqueness: only a unique twin within ±3 days may match. |
| `10_pending_and_posted_same_file` | One Citi CSV | The export lists a dinner twice: pending at $40.00 and cleared at $48.00. One transaction, inside one file. An Uber ride still pending stays pending. | "Rows of the same file never match each other." |
| `11_checks_same_amount_day` | Wells QFX (CHECKNUM), then the Wells CSV (CHECK # column) | Checks 1043 and 1044, both $150.00, both described `CHECK`, on 05-12: two transactions. The CSV repeats both and adds check 1045 with the same amount and day. | Matching on amount, date and text while ignoring the check number. |
| `12_recycled_fitid_atm_zelle` | Chase checking QFX: a 3-day download, then the month | A $20.00 ATM withdrawal (03-02) and a $20.00 Zelle payment (03-04) share FITID `77`, both inside the month file. Two transactions; the ATM row is in both downloads. | "Same `external_id` → duplicate", even within one file. |
| `13_generic_name_vs_csv_text` | PNC QFX, then PNC CSV | The QFX has NAME `POS PURCHASE` and the merchant in MEMO; the CSV has `POS PURCHASE <merchant>` with no memo. Shell (QFX) and Chevron (CSV), both $40.00 on 03-10, are different purchases. H-E-B $61.20 on 03-12 is in both (CSV text = NAME + MEMO). | A generic NAME compared to the CSV text alone. |

Notes on realism:
- The regenerated FITIDs in case 02 and the recycled ones in 03a are known bank behaviours, but the files are constructed rather than copied from samples.
- 07b is a constructed variant; Barclays' real QFX carries DTUSER.
- Cases 08 and 09 use only real behaviour: Chase's and Amex's QFX have no DTUSER, and Amex's basic CSV has a single date column.
- 10 (both rows in one export), 11 (three same-day checks) and 12 (FITID `77` reused) are constructed situations in documented layouts.
- Every file follows its bank's documented layout.

## Checks (`tests/fixtures.rs`)

The folder is a tiny standalone Cargo workspace that only tests the fixtures. Run `cargo test --release` here.

- **Every file parses.**
  - CSVs go through `fm_import_csv::import` with the built-in profiles and must detect the stated layout.
  - QFXs go through `fm_import_ofx::parse`.
  - Each must yield exactly the statements and rows `expected.json` lists, with the same amount, dates, status and bank id per row (and check number where a row states one).
  - Account and routing numbers are fake.
- **`expected.json` is consistent with itself.**
  - `import_order` lists every file.
  - Every row names a known transaction in the right account.
  - Posted rows carry their transaction's final amount.
  - The outcomes follow from the order: a transaction's first row is `inserted`; a later posted row is `duplicate`, or `posted_pending` if the earlier one was pending.
  - Every transaction has a row, and its final status matches.
- **Coverage.**
  - All 17 cases are present.
  - Every file under `cases/` is listed by some case.
  - Case 04 is the one with two accounts sharing a last four.
- **Both orders.** 08a/08b and 09a/09b share files and ground truth, and their import orders are reversed. Every row is the same transaction in both.

Failure messages name the file, row and field, never the values.
