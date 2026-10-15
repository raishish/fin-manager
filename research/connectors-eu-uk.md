# Data connectors: EU/EEA and UK (research as of 2026-10-11)

Source: research agent run on 2026-10-11, using web sources (linked inline). Claims marked **unverified** need checking before we rely on them.

## Summary
- **Calling bank APIs yourself requires a licence.** Bank PSD2 (EU) and Open Banking (UK) APIs only accept callers presenting eIDAS QWAC/QSealC certificates, which are issued only to authorised firms. In the UK the caller must be FCA-authorised or registered. Neither the app nor the user can call them directly.
- **Fully on the device (Tier 1):**
  - File imports.
  - **FinTS/HBCI** in Germany: the bank's own protocol, called from the device with the user's own login.
  - A few banks that give users personal API keys: Starling, Monzo, bunq, Trading 212, and the IBKR Flex reporting service.
- **Best Tier 2 option:** each user registers their own **Enable Banking** application, free in "restricted mode" for personal, non-commercial use. This is how Actual Budget and Firefly III users in Europe connect banks today.
- **GoCardless Bank Account Data (formerly Nordigen):** stopped taking new signups in July 2025 and is now enterprise-only.
- **Investments and pensions:** no mandated API in the EU or UK. FiDA is stalled and the UK only has a roadmap, so this relies on CSV/PDF imports plus the few broker APIs.

## Tier 1: fully on the device, no intermediary
| Option | How it works | Vendor in path? | Coverage | Cost | Caveats |
|---|---|---|---|---|---|
| CSV / CAMT.053 / MT940 / OFX / QIF files | User downloads from online banking | No | Universal. CAMT/MT940 common in DE/NL/AT/Nordics; OFX/QIF mostly UK | Free | Manual; per-bank parsing; OFX rare in EU |
| PDF statements | Parsed on the device (local models or rules) | No | Universal, including brokers and pensions | Free | Fragile layouts. Portfolio Performance shows it is practical for brokers |
| FinTS/HBCI (Germany) | Device talks straight to the bank's FinTS server; PIN/TAN stays on the device | No | Most DE banks and savings banks | Free, but needs a product registration ID from Die Deutsche Kreditwirtschaft (days to 8+ weeks) | Hibiscus works this way apparently without a BaFin licence (**legal view unverified**). Serving other people's accounts may need PSD2/ZAG licensing ([Ledgers #57](https://github.com/Ledgers-sh/ledgers/issues/57), [drweb.de](https://www.drweb.de/banking-software-vergleich/)) |
| Bank personal tokens | User creates a token in their bank app; device calls the bank | No | Starling ([docs](https://developer.starlingbank.com/docs)); Monzo, personal use only, full history only within 5 minutes of login, then 90 days ([docs](https://docs.monzo.com/)); bunq Pro/Elite ([docs](https://doc.bunq.com/basics/authentication/api-keys)) | Free | Wise personal tokens cannot read statements for EU/UK users ([docs](https://docs.wise.com/api-docs/guides/personal-tokens)) |
| Broker APIs and reports | IBKR Flex Web Service (read-only XML); Trading 212 API key | No | IBKR; Trading 212 Invest and ISA only, no SIPP ([docs](https://docs.trading212.com/api)) | Free | The Trading 212 key can also place trades, so ask for least privilege |

## Tier 2: a third party sees the data, our servers don't
| Option | How it works | Vendor in path? | Coverage | Cost | Caveats |
|---|---|---|---|---|---|
| **Enable Banking, restricted mode** | User registers their own app and private key and links only their own accounts; the device signs requests and calls the API | Enable Banking (licensed AISP, supervised by the Finnish FSA); not us | 2,500–2,700 EEA banks. **UK not listed** | Free for own accounts ([Firefly docs](https://docs.firefly-iii.org/tutorials/data-importer/eb/), [EB knowledge base](https://enablebanking.com/knowledge-base)) | **Terms risk:** limited to personal, non-commercial use; unclear whether a commercial app guiding each user through setup is acceptable. Hard setup for users. Consent lasts up to 6 months. Browser/redirect support **unverified** |
| GoCardless Bank Account Data | Was free and self-serve | GoCardless | EEA + UK | n/a | **Closed to new signups since July 2025** ([GoCardless](https://bankaccountdata.gocardless.com/new-signups-disabled), [Actual docs](https://actualbudget.org/docs/advanced/bank-sync/)) |
| Lunch Flow | User's own subscription; routes to regional aggregators | Lunch Flow plus the underlying aggregator | 30+ countries including UK, EU and investments | About $2.92/month for 2 connections, $10 per extra (vendor site, Oct 2026) | Relays data through its own servers; licensing basis **unverified** ([lunchflow.app](https://www.lunchflow.app/)) |

## Tier 3: we would need a licence, or a backend contracted with an aggregator
- **Aggregators:** Tink (Visa), TrueLayer, Yapily, Salt Edge, Plaid EU, Powens (formerly Budget Insight), finAPI (owned by Fabrick), and Enable Banking in full production.
- **Pricing:** free sandboxes; production prices come from sales. One developer's comparison (which may be biased) suggests TrueLayer about £150–300/month, Yapily about £200–500/month, and Tink usually four figures ([dev.to](https://dev.to/johnfrandsen/the-cheapest-open-banking-apis-for-small-businesses-and-indie-builders-in-2026-5cab)).
- **Ways to be allowed:** register as an AISP ourselves (PSD2 Art. 33, with professional indemnity insurance), become a UK FCA RAISP, become an agent of a licensed provider, or use an aggregator's licence.
- **Possible hybrid:** our backend holds only the aggregator secret and passes the access token to the device, so data goes aggregator → device. We would still hold tokens and be the contracted party, so it is probably still Tier 3.

## Regulation as of Oct 2026
- **PSD3/PSR (EU):**
  - Political deal Nov 2025; ECON committee approved May 2026.
  - "Close to adoption" but not yet in the Official Journal ([EP tracker](https://www.europarl.europa.eu/legislative-train/theme-economic-and-monetary-affairs-econ/file-payment-services-and-electronic-money-services-(directive)), [FinTech Weekly](https://www.fintechweekly.com/magazine/articles/psd3-fida-clarity-ai-act-fintech-regulation-waiting-room-2026)).
  - Applies about 21 months after publication, so roughly 2028.
  - Gives consumers no do-it-yourself API access.
- **FiDA (EU open finance):**
  - Trilogue talks paused in early 2026 over the "gatekeeper" question. One source expects agreement by end of 2026, so treat the timing as **uncertain**.
  - Application 2029–2030 at the earliest.
- **UK:**
  - Open Banking Limited still runs the CMA Order regime; the FCA now leads, with a Future Entity to come ([FCA FS25/4](https://www.fca.org.uk/publication/feedback/fs25-4.pdf)).
  - An FCA consultation and a statutory instrument under the Data (Use and Access) Act are expected in Q3–Q4 2026.
  - Open Finance Roadmap (Apr 2026) targets 2030, starting with SME lending and mortgages ([FCA](https://www.fca.org.uk/publications/corporate-documents/open-finance-roadmap)).
  - Pensions dashboards: there is no third-party consumer API.
  - No personal-use access route.

## Best fit for our principle
1. **Default:** Tier 1 file import (CAMT.053, MT940, CSV, OFX/QIF, PDF parsed by the local model) for everything.
2. **Germany:** add FinTS on the device. Needs a DK product ID and a lawyer's view on licensing.
3. **Personal-token connectors:** Starling, Monzo, bunq, IBKR Flex, Trading 212.
4. **Optional Tier 2 "bring your own Enable Banking key"** for EEA live sync, after written confirmation from Enable Banking that it fits their terms.
5. **UK automated cross-bank sync is the gap.** Without a licence, the only route is a user-paid service like Lunch Flow, which passes data through a third party.
6. **Avoid Tier 3** unless we accept AISP registration or a backend contracted with an aggregator.
