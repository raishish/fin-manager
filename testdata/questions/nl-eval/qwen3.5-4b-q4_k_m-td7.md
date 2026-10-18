# nl-eval: qwen3.5-4b-q4_k_m

- Model file: `Qwen3.5-4B-Q4_K_M.gguf` (2740937888 bytes), SHA-256 `00fe7986ff5f6b463e62455821146049db6f9313603938a70800d1fb69ef11a4`
- Prompt `nlq-6`, greedy, as of 2026-10-01, answer lead "<think>\n\n</think>\n\n"
- Questions: questions.json

## Result

| | Correct | |
|---|---|---|
| All questions | 15/25 | 60.0% |
| Without the 1 proxies | 15/24 | 62.5% |
| comparisons | 0/4 | 0.0% |
| hard | 4/4 | 100.0% |
| merchants | 3/4 | 75.0% |
| recurring | 0/4 | 0.0% |
| time_ranges | 4/4 | 100.0% |
| totals | 4/5 | 80.0% |

## With answer completion (`nl_query::complete`)

| | Correct | |
|---|---|---|
| All questions | 18/25 | 72.0% |
| Without the proxies | 18/24 | 75.0% |

Extra items with completion: 6 of 25 questions.

The completion rules (each only adds items):

| The model wrote | Added | Rule |
|---|---|---|
| a ranked label | the same rank's amount (or count, if ranked by count) | `rank-amount` |
| a ranked amount or count | the same rank's label | `rank-label` |
| the top n labels | the amount (or count) at each position | `top-amounts` |
| one output of a picked transaction | the other two of merchant, amount and date | `pick-others` |
| two money sums of one measure | their difference and percent change if nothing is derived from them yet, and the larger | `pair-compare` |
| a difference | the percent change of the same two items | `difference-change` |
| a sum within another sum's scope (same measure and period, fewer filters) | its share of the larger (instead of a difference) | `part-share` |
| a recurring price before, after or date of change | the other two | `price-steps` |
| recurring names | their latest total | `names-total` |
| a recurring latest total | the names behind it | `total-names` |
| an average | the sum and the count (of purchases, for spending) | `average-parts` |
| two counts, nothing derived from them | their sum | `counts-sum` |
| a count | the sum | `count-sum` |
| a sum in the original currency | the same in the account's currency | `original-charged` |
| all money in or out | the part of it that is income or spending, for the same accounts and period | `money-split` |

## Breakdown (without completion)

- Wrong value 7, stray period 0, run error 3, no parse 0, no answer 0
- Extra items (more answers than expected): 1 of 25 questions
- Evidence ids agree with TD.3 on 13 of 15 questions whose values all matched
- Mean confidence: 3828 bp when correct, 1543 bp when wrong
- Time per question: mean 74940 ms, median 72959 ms, p95 85185 ms
- Items completion adds per answer: 0 added: 13, 1 added: 8, 2 added: 4 (of 25 answered questions)

## Every question

| Id | Type | Proxy | Outcome | Completed | Extra | Confidence | ms |
|---|---|---|---|---|---|---|---|
| h01 | totals |  | Correct | Correct |  | 2488 | 72959 |
| h02 | totals |  | Correct | Correct |  | 3845 | 71061 |
| h03 | totals |  | WrongValue | WrongValue |  | 1180 | 71678 |
| h04 | totals |  | Correct | Correct |  | 2658 | 70766 |
| h05 | totals |  | Correct | Correct |  | 2796 | 73702 |
| h06 | comparisons |  | WrongValue | Correct |  | 5297 | 85185 |
| h07 | comparisons |  | WrongValue | WrongValue | yes | 190 | 91203 |
| h08 | comparisons |  | WrongValue | Correct |  | 2127 | 71964 |
| h09 | comparisons |  | WrongValue | Correct |  | 3956 | 77591 |
| h10 | merchants |  | Correct | Correct |  | 8070 | 70843 |
| h11 | merchants |  | Correct | Correct |  | 4937 | 74104 |
| h12 | merchants |  | Correct | Correct |  | 4251 | 80283 |
| h13 | merchants |  | WrongValue | WrongValue |  | 138 | 72924 |
| h14 | recurring |  | RunError | RunError |  | 10 | 76589 |
| h15 | recurring |  | RunError | RunError |  | 1037 | 77828 |
| h16 | recurring |  | RunError | RunError |  | 135 | 83210 |
| h17 | recurring | yes | WrongValue | WrongValue |  | 1361 | 71065 |
| h18 | time_ranges |  | Correct | Correct |  | 5735 | 69342 |
| h19 | time_ranges |  | Correct | Correct |  | 3872 | 73984 |
| h20 | time_ranges |  | Correct | Correct |  | 2983 | 71252 |
| h21 | time_ranges |  | Correct | Correct |  | 4734 | 71080 |
| h22 | hard |  | Correct | Correct |  | 2100 | 71684 |
| h23 | hard |  | Correct | Correct |  | 4472 | 75296 |
| h24 | hard |  | Correct | Correct |  | 2500 | 74983 |
| h25 | hard |  | Correct | Correct |  | 1991 | 72935 |

## What went wrong

### h03 (WrongValue)

Money(24013, "USD") not found

- total: total spending (net of refunds) · Shipping & postage · 2025

```json
{"items":[{"name":"total","ask":{"op":"sum","measure":"spending","scope":{"period":{"year":2025},"categories":["services.shipping"]}}}]}
```

### h06 (WrongValue)

Text("coffee") not found

- fast_food: total spending (net of refunds) · Fast food · this year so far (2026-01-01 to 2026-10-01)
- coffee: total spending (net of refunds) · Coffee shops · this year so far (2026-01-01 to 2026-10-01)
- difference: fast_food minus coffee

```json
{"items":[{"name":"fast_food","ask":{"op":"sum","measure":"spending","scope":{"period":"year_to_date","categories":["food.fast_food"]}}},{"name":"coffee","ask":{"op":"sum","measure":"spending","scope":{"period":"year_to_date","categories":["food.coffee"]}}},{"name":"difference","ask":{"op":"derive","formula":"difference","of":["fast_food","coffee"]}}]}
```

### h07 (WrongValue)

Money(39282, "USD") not found

- this_year: total spending (net of refunds) · Rideshare & taxis · 2026
- last_year: total spending (net of refunds) · Rideshare & taxis · 2025
- difference: this_year minus last_year
- change: change from last_year to this_year, in percent

```json
{"items":[{"name":"this_year","ask":{"op":"sum","measure":"spending","scope":{"period":{"year":2026},"categories":["transport.rideshare"]}}},{"name":"last_year","ask":{"op":"sum","measure":"spending","scope":{"period":{"year":2025},"categories":["transport.rideshare"]}}},{"name":"difference","ask":{"op":"derive","formula":"difference","of":["this_year","last_year"]}},{"name":"change","ask":{"op":"derive","formula":"percent_change","of":["this_year","last_year"]}}]}
```

### h08 (WrongValue)

Money(472439, "USD") not found

- month: the month with the lowest spending (net of refunds) · this year so far (2026-01-01 to 2026-10-01)

```json
{"items":[{"name":"month","ask":{"op":"rank","by":"month","measure":"spending","order":"lowest","output":"label","scope":{"period":"year_to_date"}}}]}
```

### h09 (WrongValue)

Percent(169) not found

- software: total spending (net of refunds) · Software & subscriptions · 2025
- total_spending: total spending (net of refunds) · 2025

```json
{"items":[{"name":"software","ask":{"op":"sum","measure":"spending","scope":{"period":{"year":2025},"categories":["services.software"]}}},{"name":"total_spending","ask":{"op":"sum","measure":"spending","scope":{"period":{"year":2025}}}}]}
```

### h13 (WrongValue)

Texts(["IRS", "Riverside Flats"]) not found

- top_1: the 1 merchants with the highest spending (net of refunds) · last month (September 2026)

```json
{"items":[{"name":"top_1","ask":{"op":"rank","by":"merchant","measure":"spending","order":"highest","output":"labels","scope":{"period":{"last_months":1}}}}]}
```

### h14 (RunError)

item "latest" found nothing

- monthly_average: average spending (net of refunds) per month · Phone · last quarter (July 2026 to September 2026)
- latest: current price of still running subscriptions in Phone

```json
{"items":[{"name":"monthly_average","ask":{"op":"average","measure":"spending","per":"month","scope":{"period":"last_quarter","categories":["utilities.phone"]}}},{"name":"latest","ask":{"op":"recurring","output":"latest","select":{"categories":["utilities.phone"],"kinds":["subscription"],"status":"active"}}}]}
```

### h15 (RunError)

item "before" found nothing

- before: date of the last price change of still running subscriptions in Health insurance
- now: price after the last change of still running subscriptions in Health insurance

```json
{"items":[{"name":"before","ask":{"op":"recurring","output":"price_from","select":{"categories":["insurance.health"],"kinds":["subscription"],"status":"active"}}},{"name":"now","ask":{"op":"recurring","output":"price_after","select":{"categories":["insurance.health"],"kinds":["subscription"],"status":"active"}}}]}
```

### h16 (RunError)

item "price_before" found nothing

- price_before: price before the last change of still running subscriptions in Software & subscriptions
- price_after: price after the last change of still running subscriptions in Software & subscriptions
- difference: price_before minus price_after

```json
{"items":[{"name":"price_before","ask":{"op":"recurring","output":"price_before","select":{"categories":["services.software"],"kinds":["subscription"],"status":"active"}}},{"name":"price_after","ask":{"op":"recurring","output":"price_after","select":{"categories":["services.software"],"kinds":["subscription"],"status":"active"}}},{"name":"difference","ask":{"op":"derive","formula":"difference","of":["price_before","price_after"]}}]}
```

### h17 (WrongValue)

Money(25000, "USD") not found

- cost: current prices added up for still running subscriptions in Software & subscriptions

```json
{"items":[{"name":"cost","ask":{"op":"recurring","output":"latest_total","select":{"categories":["services.software"],"kinds":["subscription"],"status":"active"}}}]}
```

