# Data connectors: US fact-check and other regions (research as of 2026-10-11)

Source: research agent run on 2026-10-11, using web sources (linked inline). Claims marked **unverified** need checking.

## Part A: US, verified status

1. **SimpleFIN Bridge**
   - **Price:** $1.50/month or $15/year, covering up to 25 institutions and 25 apps ([bridge](https://beta-bridge.simplefin.org/)).
   - **Flow:** the user creates a one-time Setup Token; the app claims an Access URL once, then reads with Basic Auth. No developer registration is needed ([dev guide](https://beta-bridge.simplefin.org/info/developers)).
   - **MX is the underlying provider** ([privacy](https://beta-bridge.simplefin.org/info/privacy)). SimpleFIN and MX see the data; we don't.
   - **Limits:**
     - At most 90 days per request; some banks give only 30–60.
     - Refreshes about every 24 hours.
     - 24 requests a day or fewer; going over repeatedly gets the token disabled.
     - No investment holdings and no categories, only a raw description ([comparison, 2026-10-03](https://bankbridge.money/guides/bankbridge-vs-simplefin)).
   - **Actual Budget** uses it as its North American provider ([Actual docs](https://actualbudget.org/docs/advanced/bank-sync/)).
2. **Teller**
   - mTLS client certificate required outside the sandbox. The docs say never to embed the private key in an app ([docs](https://teller.io/docs/api/authentication)).
   - Free developer tier: 100 live enrollments; paid use about $0.30 per enrollment per month plus per-call fees.
   - **Individuals can sign up,** so "each user brings their own certificate and key" works without our servers, but setup is clunky.
3. **Plaid**
   - Limited Production ended 2026-04-15 and was replaced by a free **Trial** plan: real production data, up to 10 Items, auto-approved ([billing docs](https://plaid.com/docs/account/billing/)).
   - Pricing: one-time fees for Auth/Identity, per-Item monthly fees for Transactions/Investments/Liabilities, per-call fees for Balance.
   - Expects the secret to stay off the client. A user's own Trial keys kept on their own device is possible.
   - **Unverified:** whether the Trial terms allow ongoing personal use, and whether OAuth banks (Chase etc.) work without a full review.
4. **Apple FinanceKit**
   - US: iOS 17.4+, Apple Card, Apple Cash and Savings only.
   - UK: iOS 18.4+, open-banking-linked Wallet accounts work (Barclays, HSBC, Lloyds, Monzo, Nationwide, NatWest, Santander) ([Apple](https://developer.apple.com/financekit)).
   - Needs a per-app entitlement, and the app must be in the Finance category.
   - On-device only, and Apple never sees the data. **iOS only, so not usable in the desktop MVP.**
5. **OFX Direct Connect:** declining fast.
   - Chase, Schwab, Discover and PayPal are off; Bank of America ended all OFX around 2025-09-30.
   - Small institutions are still dropping it ([Quicken community](https://community.quicken.com/discussion/7966609/9-30-25-bank-of-america-ofx-discontinued)).
   - Don't plan around it. Manual OFX/QFX file import is still worthwhile.
6. **CFPB Section 1033:** finalised Oct 2024 but **enjoined** (*Forcht Bank v. CFPB*, 2025-10-29) while the CFPB reconsiders it.
   - The 2026-04-01 compliance date passed without enforcement.
   - A rewrite went to White House review (OIRA) in Aug 2026 and was unpublished as of 2026-09-28. It is expected to allow fees and narrow who can access data on a user's behalf ([Cozen](https://www.cozen.com/news-resources/publications/2026/section-1033-compliance-date-open-banking-rule-enjoined-and-under-reconsideration), [Ballard](https://www.consumerfinancemonitor.com/2026/08/06/cfpb-sends-new-section-1033-open-banking-proposal-to-oira-for-review/)).
   - JPMorgan already charges aggregators. FDX is recognised as a standard-setter until 2030.
   - **Not a v1 dependency.**
7. **Other US options**
   - **SnapTrade Personal:** free for personal use, up to 20 brokerage connections. The user gets their own client ID and key, which can live on their device ([docs](https://docs.snaptrade.com/docs/personal-vs-commercial)). **Best fit for investment accounts.**
   - **Akoya:** FDX-based but needs a network membership agreement; not for individuals.
   - **Quiltt:** from $100/month, backend-based.
   - **MX direct, Finicity:** enterprise contracts that assume a backend.
   - **Local scraping** (for example jbms/finance-dl): fragile and may breach bank terms.

## Part B: Other regions
| Region | Regime and status | Licensed intermediary needed? | Routes compatible with "vendor never sees data" | Notes |
|---|---|---|---|---|
| Canada | Consumer-Driven Banking Act: Royal Assent 2026-03-26; draft regulations 2026-06-27; final rules expected late 2026 or early 2027; Bank of Canada oversees it ([Gazette](https://gazette.gc.ca/rp-pr/p1/2026/2026-06-27/html/reg3-eng.html)) | Yes (Bank of Canada accreditation) | SimpleFIN (MX covers North America); statement import | Screen scraping (Flinks, Plaid CA) still common |
| Brazil | Open Finance fully live, 128M consents (Jan 2026) | Yes (only BCB-authorised institutions) | A connection the user holds themselves through Pluggy (Actual supports it) | Probably the strongest regime worldwide |
| Australia | CDR live for banking; non-bank lenders added in 2026; required history cut from 7 years to 2; other sectors paused in 2024 ([ACCC](https://www.accc.gov.au/media-release/non-bank-lenders-join-consumer-data-right-as-next-stage-commences)) | Yes (ADR accreditation or CDR representative) | Statement import only | Screen-scraping ban announced 2024; whether it is in force is **unverified** |
| Singapore | SGFinDex via Singpass to banks, insurers, CDP, CPF, HDB ([MAS](https://www.mas.gov.sg/development/fintech/sgfindex)) | Only participating institutions or government apps | Not open to third-party apps (**unverified** for 2026) | Market-led |
| Japan | 2018 Banking Act: registered EPSPs get bank-API access; 130+ banks | Yes (FSA registration and contracts with each bank) | CSV import only (**unverified**) | Money Forward and Moneytree are cloud-based |
| Saudi Arabia | SAMA started licensing AIS/PIS providers in Mar 2026 | Yes | None found | Early stage. The agent's source link pointed to a .gov.sg domain, which looks wrong; **re-check** |
| UAE | CBUAE Open Finance Regulation (Jul 2025); Al Tareq platform live | Yes | None found | Banks still being added |
| India | Account Aggregator (see `connectors-india.md`) | Yes (the receiving FIU must be regulated) | Statement and CAS import, Android SMS | — |
| Mexico | Fintech Law Art. 76 rules for transaction data still unpublished (about 6 years late) | n/a | Import only | Stalled |
| Nigeria | CBN framework; phased go-live through mid-2026 | Yes | None | Live status **unverified** |
| Indonesia / China | Indonesia's SNAP is a payments standard; China has PIPL portability only | n/a | Import only | — |

**Takeaway:** outside the US, live data access almost always needs a licensed intermediary. "The vendor never sees the data" usually means statement import, or a third party the user holds a relationship with directly (Pluggy, SimpleFIN, Enable Banking).
