# Explanations and the number check (`fm_llm::explain`)

SPEC constraint 6: every number in an LLM-written explanation is extracted and checked against the computed values. On a mismatch the app asks again, or falls back to a template.

This module has no dependency on fm-core: the caller (the Ask flow) passes the computed values in as `Fact`s.

```rust
let facts = vec![Fact { name: "total".into(), value: Value::Money(Money { minor: 123456, currency: "USD".into(), minor_digits: 2 }), evidence: Some(12) }];
let e = explain("How much did I spend?", &facts, &[], 2, |prompt| session.generate(prompt));
// e.text, e.source (Model or Template), e.rejected (the checks that failed)
```

## Facts
- **`Fact { name, value, evidence }`.**
  - `value` is one of `Money { minor, currency, minor_digits }`, `Count`, `Percent { basis_points }`, `Date`, `Month`, `Year`, `Text` or `Texts`.
  - `evidence` is the number of transactions behind the value, which the text may state too ("from 12 transactions").
- **The caller passes everything the text may state:** the answer's values, the evidence counts, and the question's period (as `Month` or `Year` facts).
- **Any other number is rejected.** That includes numbers that would be true: constraint 6 as written.
- **`Derivation`s** list the numbers a text may compute from the facts: `Difference(a, b)`, `Sum(names)`, `Ratio(a, b)` as a percent, and `PercentChange(a, b)`. Nothing derived is allowed unless listed.

## What counts as a number (`extract.rs`)
- **Money:**
  - `$1,234.56`, `-$12.50`, `−$12.50`;
  - `$1.2k`, `$3M`, `$2.5 million`, `3bn`;
  - `€12.30`, `£5`, `¥500`, `USD 1,234`, `7.50 EUR`;
  - `20 dollars|euros|pounds|yen`.
- **Percents:** `12%`, `12.3 percent`, `4 percentage points`, `7 per cent`.
- **Plain numbers:** `1,234` and `12.5`, and written-out numbers: zero to nineteen, the tens (`twenty-five`), `a dozen`, `a hundred`, `two hundred`, `a thousand`.
- **Dates:** `2026-03-03`, `03/03/2026` (US order), `March 3, 2026`, `3 March 2026`, `Mar 7th 2026`, and `March 3` without a year.
- **Months:** `September 2025`, `Sept. 2025`, `2025-09`.
- **Years:** a bare four-digit number from 1900 to 2100.
- **Not numbers:**
  - ordinals (`3rd`) and digits inside words (`Q4`, `W2`);
  - `one-off`, `one-time`, and "may" without a day or a year;
  - anything inside a fact's name, or inside a fact's own text values. This is why a merchant called "IN-N-OUT 592" or a label like "Q4 2025" isn't read as numbers.

## Matching (`check.rs`)
- **Money matches money only**, in a currency the mark can mean: `$` covers USD and the other dollars, `€` is EUR, and a code is that code.
  - Values are compared without their sign, so "spent $12.50" states −1,250 cents.
  - A plain number never matches money, and a percent never matches money.
- **Percents match percents only:** facts, or listed ratios and changes.
- **Plain numbers** match a count, an evidence count, or a listed sum or difference of counts.
- **A bare year** matches a `Year` fact, the year of any `Date` or `Month` fact, or a count. If it matches none of them, it is caught.
- **Dates** match a `Date` fact exactly. A date without a year matches its month and day. A month matches a `Month` fact or the month of a `Date` fact.

**Rounding** follows the precision the text shows:
- **Plain.** The fact rounded to as many decimals as the text shows must equal the text's number. For 1,234.56:
  - "$1,234.56" passes;
  - "$1,235" passes (rounded to dollars);
  - "$1.2k" passes (rounded to tenths of a thousand);
  - "$1,234" and "$1,200" fail.
- **Hedged.** The hedges are `about`, `around`, `roughly`, `approximately`, `approx`, `nearly`, `almost` and `~`. A hedged number may be rounded to the significant digits it shows, as long as it shows at least two and is within 5%. **Decision:** for 1,234.56:
  - "about $1,200" **passes** (two significant digits, 2.8% off);
  - "about $1,000" fails (one significant digit);
  - "about $1,300" fails (it rounds to 1,200).
- **Bounds aren't hedges.** "over", "under" and "more than" state a bound, not the value, so the number after one must match exactly. "over $1,200" fails for 1,234.56.

## The template (`template.rs`)
- One sentence per fact, written only from the facts, for example: `Total spent: $1,234.56, from 42 transactions.`
- Formats:
  - Money uses the currency's symbol (USD, EUR, GBP, JPY) or its code, thousands commas, and exactly `minor_digits` decimals.
  - Percents are `28%` or `12.34%`.
  - Dates are `January 20, 2026` and months `September 2025`.
  - Texts are quoted, and lists are joined with commas.
- Every template passes its own check. The tests hold it to that on every TD.3 answer.

## The flow (`mod.rs`)
- **`prompt(question, facts, derivations)`** holds the instructions, the user's question, the allowed derivations, and one line per fact as the template writes it. Nothing else from the ledger goes in: no transaction descriptions beyond the facts' own text values.
- **`explain(...)`** calls the caller's generator up to `attempts` times and keeps the first text that passes. It falls back to the template after the last attempt, or at once when the generator returns `None` (no model).
- **No logging.** A `Report` holds byte offsets and kinds, never the numbers.
- **What's left:** the model wording plugs in through T2.4's session, and into the Ask flow with T4.2.

## Tests (`tests.rs`, all on fixed texts)
- **Every form above** is read, and so are the non-numbers.
- **Every matching rule**, including the hedge decision. Money and percents are kept apart, and a bare year matching neither a year nor a count is caught.
- **Derivations:** each is allowed only when listed, and an unlisted percent is reported on its own.
- **The template** handles every kind, and the flow handles retry, fallback, no model and an empty text.
- **TD.3** (done-when): each of the 100 questions' answers becomes facts.
  - A month written as text becomes a `Month` fact.
  - Its template states **275 numbers with 0 unchecked**.
  - For every question, **one injected wrong number is caught (100/100):**
    - a money value off by one cent (84);
    - a count off by one (6);
    - a date off by a day (4);
    - a month not among the answer's months (4);
    - a percent off by 0.1 (1);
    - a number invented into a text-only answer (1).
  - **A right number in the wrong unit is caught (98):** a percent written as dollars, money written as a percent, or a count written as dollars.

## Limits
- **The check is by value, not by position.** A number that equals another fact's value passes, even if it is attached to the wrong label. For example, "January" written where "March" belongs passes when both months are in the answer. Checking which value a number is attached to would need the model to tag its numbers. It doesn't, by design: the template already gives that guarantee.
- **US conventions.** Dates are read month first (`03/04/2026` is March 4). A comma is a thousands separator, not a decimal one.
