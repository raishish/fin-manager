# Data connectors: cross-region synthesis (2026-10-11)

Detail files: `connectors-us-global.md`, `connectors-eu-uk.md`, `connectors-india.md`.
Principle being tested: **financial data never passes through our servers.**

## The pattern is the same everywhere
Every region breaks down into the same four routes:

| Route | Examples | Region-specific? | Fits the desktop MVP? |
|---|---|---|---|
| **1. File and statement import, parsed on the device** | CSV, OFX/QFX/QIF (US/UK), CAMT.053/MT940 (EU), PDF statements, India CAS/AIS/26AS/EPF | Formats vary, but **works in every country** | Yes |
| **2. The user's own keys to a third-party service, called from the device** | SimpleFIN, SnapTrade Personal, Teller/Plaid with the user's own keys (US); Enable Banking with the user's own key (EEA); Pluggy (Brazil); broker personal APIs (India Kite/Upstox, IBKR Flex, Trading 212) | Yes, one adapter per service | Yes |
| **3. Direct-to-bank protocols or personal tokens** | FinTS/HBCI (DE), Starling/Monzo/bunq tokens (UK/EU), OFX Direct Connect (US, dying) | Yes, narrow coverage | Yes |
| **4. On-device OS data sources** | Apple FinanceKit (US/UK iOS), Android SMS parsing (India) | Yes | **No; mobile only, so later** |
| ~~5. Licensed aggregator through our backend~~ | Plaid/MX/Tink/TrueLayer/AA-as-FIU | — | **Ruled out by principle** |

## Implications for the design
- **The universal backbone is route 1 plus one unified data model.** It is the only route that works in every region, and it is also the "combine data formats" layer in the vision. A PDF parser that uses the local model is the strongest showcase for the AI layer across regions.
- **Live sync is a set of per-region plug-ins (routes 2 and 3).** Each is a thin adapter into the same pipeline.
- **The trade-off with route 2:** the user signs up with a third party, and that third party (and its upstream, for example MX) sees the data. That matches "users own their data" but is not "nobody but the user sees it". The UI should say this honestly.
- **Investments everywhere lean on brokers' own exports or APIs.** No region (EU FiDA stalled, UK aiming for 2030, US 1033 enjoined) mandates investment data access soon.
- **Regulatory watch list:** US 1033 rewrite; EU PSD3/FiDA; UK Data (Use and Access) Act rules; India's 31 Dec 2026 change adding bank deposits to the depository CAS; Canada's final consumer-driven-banking rules.
