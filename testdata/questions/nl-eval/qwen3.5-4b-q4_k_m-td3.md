# nl-eval: qwen3.5-4b-q4_k_m

- Model file: `Qwen3.5-4B-Q4_K_M.gguf` (2740937888 bytes), SHA-256 `00fe7986ff5f6b463e62455821146049db6f9313603938a70800d1fb69ef11a4`
- Prompt `nlq-6`, greedy, as of 2026-10-01, answer lead "<think>\n\n</think>\n\n"
- Questions: TD.3

## Result

| | Correct | |
|---|---|---|
| All questions | 70/100 | 70.0% |
| Without the 9 proxies | 65/91 | 71.4% |
| comparisons | 6/15 | 40.0% |
| hard | 10/20 | 50.0% |
| merchants | 12/15 | 80.0% |
| recurring | 8/15 | 53.3% |
| time_ranges | 14/15 | 93.3% |
| totals | 20/20 | 100.0% |

## With answer completion (`nl_query::complete`)

| | Correct | |
|---|---|---|
| All questions | 84/100 | 84.0% |
| Without the proxies | 78/91 | 85.7% |

Extra items with completion: 30 of 100 questions.

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

- Wrong value 29, stray period 0, run error 1, no parse 0, no answer 0
- Extra items (more answers than expected): 10 of 100 questions
- Evidence ids agree with TD.3 on 70 of 70 questions whose values all matched
- Mean confidence: 3589 bp when correct, 2266 bp when wrong
- Time per question: mean 42763 ms, median 39003 ms, p95 66929 ms
- Items completion adds per answer: 0 added: 57, 1 added: 24, 2 added: 16, 3 added: 3 (of 100 answered questions)

## Every question

| Id | Type | Proxy | Outcome | Completed | Extra | Confidence | ms |
|---|---|---|---|---|---|---|---|
| q001 | totals |  | Correct | Correct |  | 7611 | 38631 |
| q002 | totals |  | Correct | Correct |  | 5264 | 42019 |
| q003 | totals |  | Correct | Correct |  | 2071 | 39450 |
| q004 | totals |  | Correct | Correct |  | 3736 | 38547 |
| q005 | totals |  | Correct | Correct | yes | 7 | 52899 |
| q006 | totals |  | Correct | Correct |  | 1469 | 45912 |
| q007 | totals |  | Correct | Correct |  | 4168 | 38923 |
| q008 | totals |  | Correct | Correct |  | 3809 | 38711 |
| q009 | totals |  | Correct | Correct |  | 1765 | 40094 |
| q010 | totals |  | Correct | Correct | yes | 109 | 44987 |
| q011 | totals |  | Correct | Correct | yes | 229 | 41177 |
| q012 | totals |  | Correct | Correct |  | 3912 | 36778 |
| q013 | totals |  | Correct | Correct |  | 1776 | 40285 |
| q014 | totals |  | Correct | Correct |  | 2935 | 42768 |
| q015 | totals |  | Correct | Correct |  | 7040 | 39944 |
| q016 | totals |  | Correct | Correct |  | 3012 | 38789 |
| q017 | totals |  | Correct | Correct |  | 6929 | 39230 |
| q018 | totals |  | Correct | Correct |  | 2455 | 39628 |
| q019 | totals |  | Correct | Correct |  | 986 | 40025 |
| q020 | totals |  | Correct | Correct |  | 3742 | 39999 |
| q021 | comparisons |  | Correct | Correct |  | 240 | 51253 |
| q022 | comparisons |  | Correct | Correct |  | 2753 | 52685 |
| q023 | comparisons |  | Correct | Correct |  | 1505 | 53194 |
| q024 | comparisons |  | WrongValue | WrongValue |  | 6507 | 35569 |
| q025 | comparisons |  | WrongValue | Correct |  | 5904 | 37682 |
| q026 | comparisons |  | WrongValue | Correct |  | 6725 | 39759 |
| q027 | comparisons |  | WrongValue | Correct |  | 5678 | 45006 |
| q028 | comparisons |  | WrongValue | Correct |  | 2059 | 42650 |
| q029 | comparisons |  | Correct | Correct |  | 2989 | 40140 |
| q030 | comparisons |  | WrongValue | Correct |  | 6527 | 40890 |
| q031 | comparisons |  | Correct | Correct |  | 3270 | 42985 |
| q032 | comparisons |  | WrongValue | WrongValue | yes | 175 | 45409 |
| q033 | comparisons |  | Correct | Correct |  | 3328 | 45958 |
| q034 | comparisons |  | WrongValue | WrongValue |  | 4014 | 47360 |
| q035 | comparisons |  | WrongValue | WrongValue |  | 3096 | 37183 |
| q036 | merchants |  | Correct | Correct |  | 7704 | 37743 |
| q037 | merchants |  | WrongValue | Correct |  | 2658 | 38753 |
| q038 | merchants |  | Correct | Correct |  | 4281 | 37704 |
| q039 | merchants |  | Correct | Correct |  | 116 | 45291 |
| q040 | merchants |  | WrongValue | Correct |  | 3299 | 38507 |
| q041 | merchants |  | Correct | Correct |  | 1564 | 38100 |
| q042 | merchants |  | Correct | Correct |  | 2199 | 44595 |
| q043 | merchants |  | Correct | Correct |  | 9050 | 41382 |
| q044 | merchants | yes | Correct | Correct |  | 2108 | 42135 |
| q045 | merchants |  | Correct | Correct |  | 8032 | 37216 |
| q046 | merchants |  | WrongValue | WrongValue |  | 1394 | 37480 |
| q047 | merchants |  | Correct | Correct |  | 5966 | 36843 |
| q048 | merchants |  | Correct | Correct |  | 6943 | 37153 |
| q049 | merchants |  | Correct | Correct |  | 4269 | 38980 |
| q050 | merchants |  | Correct | Correct |  | 8479 | 36155 |
| q051 | recurring |  | Correct | Correct |  | 618 | 35880 |
| q052 | recurring |  | RunError | RunError |  | 227 | 42882 |
| q053 | recurring |  | Correct | Correct |  | 5614 | 42373 |
| q054 | recurring |  | Correct | Correct |  | 3943 | 38995 |
| q055 | recurring |  | Correct | Correct |  | 7093 | 37205 |
| q056 | recurring |  | WrongValue | WrongValue |  | 1597 | 40953 |
| q057 | recurring |  | WrongValue | Correct |  | 3541 | 39698 |
| q058 | recurring | yes | Correct | Correct | yes | 1320 | 38987 |
| q059 | recurring | yes | Correct | Correct |  | 3216 | 35864 |
| q060 | recurring | yes | WrongValue | Correct |  | 2633 | 40000 |
| q061 | recurring |  | WrongValue | WrongValue | yes | 90 | 39410 |
| q062 | recurring |  | WrongValue | Correct | yes | 2474 | 38886 |
| q063 | recurring |  | Correct | Correct |  | 165 | 36231 |
| q064 | recurring | yes | Correct | Correct |  | 7768 | 31727 |
| q065 | recurring |  | WrongValue | WrongValue |  | 955 | 33842 |
| q066 | time_ranges |  | Correct | Correct |  | 6207 | 35746 |
| q067 | time_ranges |  | Correct | Correct |  | 6182 | 36277 |
| q068 | time_ranges |  | Correct | Correct |  | 4535 | 36661 |
| q069 | time_ranges |  | Correct | Correct |  | 3552 | 36195 |
| q070 | time_ranges |  | Correct | Correct |  | 7155 | 35911 |
| q071 | time_ranges |  | Correct | Correct |  | 5228 | 36074 |
| q072 | time_ranges |  | Correct | Correct |  | 892 | 38210 |
| q073 | time_ranges |  | Correct | Correct |  | 4004 | 37723 |
| q074 | time_ranges |  | Correct | Correct |  | 5209 | 35608 |
| q075 | time_ranges |  | WrongValue | WrongValue |  | 1000 | 38111 |
| q076 | time_ranges |  | Correct | Correct |  | 6991 | 36528 |
| q077 | time_ranges |  | Correct | Correct | yes | 900 | 38745 |
| q078 | time_ranges |  | Correct | Correct |  | 3563 | 33273 |
| q079 | time_ranges |  | Correct | Correct |  | 4023 | 37340 |
| q080 | time_ranges |  | Correct | Correct |  | 1707 | 41004 |
| q081 | hard |  | Correct | Correct |  | 1369 | 38846 |
| q082 | hard |  | Correct | Correct |  | 3759 | 36698 |
| q083 | hard |  | Correct | Correct |  | 2856 | 36438 |
| q084 | hard |  | Correct | Correct |  | 2705 | 36988 |
| q085 | hard |  | WrongValue | WrongValue |  | 857 | 38166 |
| q086 | hard |  | WrongValue | Correct |  | 186 | 59598 |
| q087 | hard |  | Correct | Correct |  | 6893 | 51543 |
| q088 | hard |  | WrongValue | Correct |  | 2056 | 59677 |
| q089 | hard |  | WrongValue | WrongValue |  | 172 | 71900 |
| q090 | hard | yes | Correct | Correct |  | 2299 | 47118 |
| q091 | hard |  | Correct | Correct | yes | 109 | 38608 |
| q092 | hard |  | Correct | Correct |  | 2840 | 40181 |
| q093 | hard |  | WrongValue | WrongValue |  | 1473 | 39003 |
| q094 | hard | yes | WrongValue | WrongValue |  | 527 | 76368 |
| q095 | hard |  | Correct | Correct |  | 22 | 79678 |
| q096 | hard |  | WrongValue | Correct |  | 902 | 65090 |
| q097 | hard | yes | WrongValue | WrongValue |  | 76 | 66929 |
| q098 | hard |  | WrongValue | Correct |  | 1197 | 53993 |
| q099 | hard |  | Correct | Correct | yes | 696 | 72902 |
| q100 | hard | yes | WrongValue | WrongValue |  | 5 | 75697 |

## What went wrong

### q024 (WrongValue)

Money(8129956, "USD") not found

- earned: total income · 2025

```json
{"items":[{"name":"earned","ask":{"op":"sum","measure":"income","scope":{"period":{"year":2025}}}}]}
```

### q025 (WrongValue)

Money(1105845, "USD") not found

- month: the month with the highest spending (net of refunds) · 2025

```json
{"items":[{"name":"month","ask":{"op":"rank","by":"month","measure":"spending","order":"highest","output":"label","scope":{"period":{"year":2025}}}}]}
```

### q026 (WrongValue)

Money(663281, "USD") not found

- month: the month with the lowest income · 2025

```json
{"items":[{"name":"month","ask":{"op":"rank","by":"month","measure":"income","order":"lowest","output":"label","scope":{"period":{"year":2025}}}}]}
```

### q027 (WrongValue)

Text("coffee") not found

- coffee: total spending (net of refunds) · Coffee shops · 2025
- bars: total spending (net of refunds) · Bars & alcohol · 2025
- difference: coffee minus bars

```json
{"items":[{"name":"coffee","ask":{"op":"sum","measure":"spending","scope":{"period":{"year":2025},"categories":["food.coffee"]}}},{"name":"bars","ask":{"op":"sum","measure":"spending","scope":{"period":{"year":2025},"categories":["food.bars"]}}},{"name":"difference","ask":{"op":"derive","formula":"difference","of":["coffee","bars"]}}]}
```

### q028 (WrongValue)

Text("2025-07") not found

- highest: spending (net of refunds) of the month with the highest · Electricity · 2025
- lowest: spending (net of refunds) of the month with the lowest · Electricity · 2025

```json
{"items":[{"name":"highest","ask":{"op":"rank","by":"month","measure":"spending","order":"highest","output":"amount","scope":{"period":{"year":2025},"categories":["utilities.electricity"]}}},{"name":"lowest","ask":{"op":"rank","by":"month","measure":"spending","order":"lowest","output":"amount","scope":{"period":{"year":2025},"categories":["utilities.electricity"]}}}]}
```

### q030 (WrongValue)

Percent(2800) not found

- rent: total spending (net of refunds) · Rent · 2025
- total_spending: total spending (net of refunds) · 2025

```json
{"items":[{"name":"rent","ask":{"op":"sum","measure":"spending","scope":{"period":{"year":2025},"categories":["housing.rent"]}}},{"name":"total_spending","ask":{"op":"sum","measure":"spending","scope":{"period":{"year":2025}}}}]}
```

### q032 (WrongValue)

Money(110695, "USD") not found

- this_year: total spending (net of refunds) · Software & subscriptions · this year so far (2026-01-01 to 2026-10-01)
- last_year: total spending (net of refunds) · Software & subscriptions · 2025
- difference: this_year minus last_year
- change: change from last_year to this_year, in percent

```json
{"items":[{"name":"this_year","ask":{"op":"sum","measure":"spending","scope":{"period":"year_to_date","categories":["services.software"]}}},{"name":"last_year","ask":{"op":"sum","measure":"spending","scope":{"period":{"year":2025},"categories":["services.software"]}}},{"name":"difference","ask":{"op":"derive","formula":"difference","of":["this_year","last_year"]}},{"name":"change","ask":{"op":"derive","formula":"percent_change","of":["this_year","last_year"]}}]}
```

### q034 (WrongValue)

Money(-3148, "USD") not found

- august_2026: total spending (net of refunds) · Groceries · August 2026
- september_2026: total spending (net of refunds) · Groceries · September 2026
- difference: august_2026 minus september_2026

```json
{"items":[{"name":"august_2026","ask":{"op":"sum","measure":"spending","scope":{"period":{"month":"2026-08"},"categories":["food.groceries"]}}},{"name":"september_2026","ask":{"op":"sum","measure":"spending","scope":{"period":{"month":"2026-09"},"categories":["food.groceries"]}}},{"name":"difference","ask":{"op":"derive","formula":"difference","of":["august_2026","september_2026"]}}]}
```

### q035 (WrongValue)

Money(662804, "USD") not found

- average: average spending (net of refunds) per month · the last 6 months (April 2026 to September 2026)

```json
{"items":[{"name":"average","ask":{"op":"average","measure":"spending","per":"month","scope":{"period":{"last_months":6}}}}]}
```

### q037 (WrongValue)

Money(2276000, "USD") not found

- merchants: the 3 merchants with the highest spending (net of refunds) · 2025

```json
{"items":[{"name":"merchants","ask":{"op":"rank","by":"merchant","measure":"spending","n":3,"order":"highest","output":"labels","scope":{"period":{"year":2025}}}}]}
```

### q040 (WrongValue)

Money(284900, "USD") not found

- merchant: merchant of the largest of purchases (before refunds) · on credit card accounts · 2025

```json
{"items":[{"name":"merchant","ask":{"op":"pick","measure":"purchases","order":"largest","output":"merchant","scope":{"period":{"year":2025},"accounts":{"kinds":["credit_card"]}}}}]}
```

### q046 (WrongValue)

Text("Cosmic Cup Coffee") not found

- merchant: the 1 merchants with the highest purchases (before refunds) · 2025

```json
{"items":[{"name":"merchant","ask":{"op":"rank","by":"merchant","measure":"purchases","order":"highest","output":"labels","scope":{"period":{"year":2025}}}}]}
```

### q052 (RunError)

item "difference" combines values of the wrong kind

- from: date of the last price change of subscriptions named Adobe
- now: price after the last change of subscriptions named Adobe
- difference: from minus now

```json
{"items":[{"name":"from","ask":{"op":"recurring","output":"price_from","select":{"name":"Adobe","kinds":["subscription"]}}},{"name":"now","ask":{"op":"recurring","output":"price_after","select":{"name":"Adobe","kinds":["subscription"]}}},{"name":"difference","ask":{"op":"derive","formula":"difference","of":["from","now"]}}]}
```

### q056 (WrongValue)

Money(11988, "USD") not found

- subscriptions: names of still running annual subscriptions
- total: current prices added up for still running annual subscriptions

```json
{"items":[{"name":"subscriptions","ask":{"op":"recurring","output":"names","select":{"cadences":["annual"],"kinds":["subscription"],"status":"active"}}},{"name":"total","ask":{"op":"recurring","output":"latest_total","select":{"cadences":["annual"],"kinds":["subscription"],"status":"active"}}}]}
```

### q057 (WrongValue)

Money(187500, "USD") not found

- from: date of the last price change of recurring payments in Rent
- now: price after the last change of recurring payments in Rent

```json
{"items":[{"name":"from","ask":{"op":"recurring","output":"price_from","select":{"categories":["housing.rent"]}}},{"name":"now","ask":{"op":"recurring","output":"price_after","select":{"categories":["housing.rent"]}}}]}
```

### q060 (WrongValue)

Count(12) not found

- average: average spending (net of refunds) per month · Electricity · 2025
- total: total spending (net of refunds) · Electricity · 2025

```json
{"items":[{"name":"average","ask":{"op":"average","measure":"spending","per":"month","scope":{"period":{"year":2025},"categories":["utilities.electricity"]}}},{"name":"total","ask":{"op":"sum","measure":"spending","scope":{"period":{"year":2025},"categories":["utilities.electricity"]}}}]}
```

### q061 (WrongValue)

Money(29400, "USD") not found

- total: current prices added up for subscriptions named Coursera
- names: names of subscriptions named Coursera

```json
{"items":[{"name":"total","ask":{"op":"recurring","output":"latest_total","select":{"name":"Coursera","kinds":["subscription"]}}},{"name":"names","ask":{"op":"recurring","output":"names","select":{"name":"Coursera","kinds":["subscription"]}}}]}
```

### q062 (WrongValue)

Count(13) not found

- bills: how many still running bills
- subscriptions: how many still running subscriptions

```json
{"items":[{"name":"bills","ask":{"op":"recurring","output":"count","select":{"kinds":["bill"],"status":"active"}}},{"name":"subscriptions","ask":{"op":"recurring","output":"count","select":{"kinds":["subscription"],"status":"active"}}}]}
```

### q065 (WrongValue)

Count(4) not found

- count: number of purchases (before refunds) · Income tax · 2025
- total: total purchases (before refunds) · Income tax · 2025

```json
{"items":[{"name":"count","ask":{"op":"count","measure":"purchases","scope":{"period":{"year":2025},"categories":["taxes.income"]}}},{"name":"total","ask":{"op":"sum","measure":"purchases","scope":{"period":{"year":2025},"categories":["taxes.income"]}}}]}
```

### q075 (WrongValue)

Money(530083, "USD") not found

- total: total spending (net of refunds) · 2025-11-25 to 2025-12-25

```json
{"items":[{"name":"total","ask":{"op":"sum","measure":"spending","scope":{"period":{"range":{"from":"2025-11-25","to":"2025-12-25"}}}}}]}
```

### q085 (WrongValue)

Money(5117920, "USD") not found

- money_out: total all money out · on checking accounts · 2025

```json
{"items":[{"name":"money_out","ask":{"op":"sum","measure":"money_out","scope":{"period":{"year":2025},"accounts":{"kinds":["checking"]}}}}]}
```

### q086 (WrongValue)

Money(233452, "USD") not found

- total_spent: total purchases (before refunds) · at Amazon · 2025
- total_refunded: total refunds · at Amazon · 2025

```json
{"items":[{"name":"total_spent","ask":{"op":"sum","measure":"purchases","scope":{"period":{"year":2025},"merchant":"Amazon"}}},{"name":"total_refunded","ask":{"op":"sum","measure":"refunds","scope":{"period":{"year":2025},"merchant":"Amazon"}}}]}
```

### q088 (WrongValue)

Money(184813, "USD") not found

- total: total spending (net of refunds) · paid in EUR, in EUR · all time (2024-10-01 to 2026-10-01)

```json
{"items":[{"name":"total","ask":{"op":"sum","measure":"spending","scope":{"period":"all_time","amounts":"original","original_currency":"EUR"}}}]}
```

### q089 (WrongValue)

Money(301230, "USD") not found

- total: total spending (net of refunds) · Hotels & lodging or Flights or Car rental or Other travel · paid in EUR · all time (2024-10-01 to 2026-10-01)

```json
{"items":[{"name":"total","ask":{"op":"sum","measure":"spending","scope":{"period":"all_time","categories":["travel.lodging","travel.flights","travel.car_rental","travel.other"],"original_currency":"EUR"}}}]}
```

### q093 (WrongValue)

Money(7432764, "USD") not found

- total: total spending (net of refunds) · 2025

```json
{"items":[{"name":"total","ask":{"op":"sum","measure":"spending","scope":{"period":{"year":2025}}}}]}
```

### q094 (WrongValue)

Money(600000, "USD") not found

- retirement: total transfers out · on savings accounts · 2025

```json
{"items":[{"name":"retirement","ask":{"op":"sum","measure":"transfers_out","scope":{"period":{"year":2025},"accounts":{"kinds":["savings"]}}}}]}
```

### q096 (WrongValue)

Money(2178000, "USD") not found

- earned: total income · Q1 2026 (January 2026 to March 2026)
- checking_in: total all money in · on checking accounts · Q1 2026 (January 2026 to March 2026)

```json
{"items":[{"name":"earned","ask":{"op":"sum","measure":"income","scope":{"period":{"quarter":{"quarter":1,"year":2026}}}}},{"name":"checking_in","ask":{"op":"sum","measure":"money_in","scope":{"period":{"quarter":{"quarter":1,"year":2026}},"accounts":{"kinds":["checking"]}}}}]}
```

### q097 (WrongValue)

Money(56580, "EUR") not found

- lisbon_euros: total spending (net of refunds) · at Lone Star Smokehouse · paid in EUR · all time (2024-10-01 to 2026-10-01)
- lisbon_dollars: total spending (net of refunds) · at Lone Star Smokehouse · paid in EUR · all time (2024-10-01 to 2026-10-01)

```json
{"items":[{"name":"lisbon_euros","ask":{"op":"sum","measure":"spending","scope":{"period":"all_time","merchant":"Lone Star Smokehouse","original_currency":"EUR"}}},{"name":"lisbon_dollars","ask":{"op":"sum","measure":"spending","scope":{"period":"all_time","merchant":"Lone Star Smokehouse","original_currency":"EUR"}}}]}
```

### q098 (WrongValue)

Text("Amazon") not found

- refund: amount of the largest of refunds · 2025

```json
{"items":[{"name":"refund","ask":{"op":"pick","measure":"refunds","order":"largest","output":"amount","scope":{"period":{"year":2025}}}}]}
```

### q100 (WrongValue)

Money(2316200, "USD") not found

- set_aside: total transfers out · on checking accounts or savings accounts · to or from checking accounts or savings accounts · described as "tax" · 2025
- spending_effect: total spending (net of refunds) · on checking accounts or savings accounts · to or from checking accounts or savings accounts · described as "tax" · 2025

```json
{"items":[{"name":"set_aside","ask":{"op":"sum","measure":"transfers_out","scope":{"period":{"year":2025},"accounts":{"kinds":["checking","savings"]},"counterpart_accounts":{"kinds":["checking","savings"]},"description":"tax"}}},{"name":"spending_effect","ask":{"op":"sum","measure":"spending","scope":{"period":{"year":2025},"accounts":{"kinds":["checking","savings"]},"counterpart_accounts":{"kinds":["checking","savings"]},"description":"tax"}}}]}
```

