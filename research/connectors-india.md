# Data connectors: India (research as of 2026-10-11)

Source: research agent run on 2026-10-11, using web sources (listed at the end). Claims marked **unverified** need checking before we rely on them.

## Summary
- **Almost every data type can be covered on the device, with nothing reaching our servers.** Users download password-protected statements and files, and the app parses them locally. The gap is bank and credit-card transactions, which need statement PDFs or Android SMS parsing.
- **The Account Aggregator (AA) network is closed to an unlicensed app.** Under the RBI directions, a "Financial Information User" (FIU, the party that receives the data) must be "an entity registered with and regulated by any financial sector regulator". An unregulated app cannot be an FIU, and neither can the user.
- **The directions do let the AA show data to the customer.** AA apps (OneMoney, Finvu, CAMSfinserv, Saafe) offer a "self-consent" view of your own data. Whether any of them lets you export it is **unverified**.
- **AA data is encrypted end-to-end, but only as far as the FIU.** The FIU creates a one-time key per data fetch (ECDHE Curve25519, AES-256-GCM). That key could in principle be created on the user's device, so a server only relays unreadable data. But the FIU must still be a regulated entity that signs requests with its registered key. A regulator has never ruled on an FIU that never decrypts; **get a legal opinion**.

## Tier 1: on the device, no intermediary
| Option | How it works | Vendor in path? | Coverage | Cost | Caveats |
|---|---|---|---|---|---|
| Bank and credit-card statements (PDF, XLS, CSV) | User downloads from net banking or email; app decrypts and parses locally | No | All banks and cards; history as far back as the bank allows | Free | Each bank needs its own parser; password formats vary |
| Mutual-fund CAS from CAMS, KFintech or MF Central | Detailed CAS by email (user sets the password) or direct download from MF Central (PAN + OTP) | No | All mutual funds, with full transaction history | Free | Open-source parser exists: `casparser` (MIT; handles CAMS, KFin, NSDL, CDSL). Re-printed PDFs not supported |
| NSDL or CDSL CAS | Arrives by email monthly or half-yearly; password is PAN + date of birth | No | Demat equities, ETFs, bonds and MFs held in demat | Free | RBI (7 Oct 2026) will let depositories add **bank deposit data to the CAS** by 31 Dec 2026 |
| Form 26AS, AIS, TIS | Downloaded from the income-tax portal; AIS as PDF, JSON or CSV | No | TDS, interest, dividends, securities trades, high-value transactions | Free | AIS PDF password is lowercase PAN + date of birth (DDMMYYYY); 26AS password is date of birth |
| EPF passbook | PDF from passbook.epfindia.gov.in or UMANG | No | EPF | Free | Needs an activated UAN; UMANG now uses face authentication |
| NPS transaction statement | PDF from the CRA portals (Protean, KFin, CAMS) | No | NPS | Free | Format not verified in detail |
| GST returns | User downloads GSTR-1/3B JSON or Excel from the GST portal | No | GST | Free | Pulling via the GSTN API needs a GSP (Tier 3) |
| Insurance | Policy PDFs or an e-Insurance Account statement | No | Fragmented | Free | Bima Sugam rolling out in phases during 2026; status **unverified** |
| Credit report | Free annual PDF from each credit bureau | No | Credit report | Free | Format differs by bureau |
| SMS alerts (Android only) | Read and parse SMS on the device | No | Near real-time bank, card and UPI debits | Free | Google Play allows READ_SMS for "SMS-based money management… apps that track and manage budget", subject to a declaration form. Not possible on iOS |

## Tier 2: a third party sees the data, our servers don't
| Option | How it works | Vendor in path? | Coverage | Cost | Caveats |
|---|---|---|---|---|---|
| Email statement ingestion | IMAP or Gmail API called from the device with the user's OAuth token | No (the mail provider already holds the data) | CAS, card and bank statements arriving by email | Free | Gmail read scopes are "restricted"; whether client-side-only apps avoid the CASA security assessment is **unverified**, so plan for CASA |
| Broker APIs (Kite Connect Personal, Upstox, Angel SmartAPI) | User's own API key, called from the device | No | Holdings and positions at that broker | Kite Personal free since Mar 2025; full Kite Connect ₹500/month | One broker at a time; access tokens expire daily |
| AA app "self-consent" view | User views own data in an AA app; our app imports any export | No | Bank, MF, equities, NPS, insurance, GST | Free | Export ability **unverified**; whether the AA can see the data during self-view is **unclear** |
| Screen-scraping with user credentials | Device logs in to net banking | No | Varies | Free | Breaks banks' terms of use, fragile, poor security. Not recommended |

## Tier 3: regulated vendor or a backend in the path
| Option | Notes |
|---|---|
| Become, or partner with, an FIU on the AA network | Requires a licence from RBI, SEBI, IRDAI or PFRDA (for example an NBFC licence or SEBI investment-adviser registration). About ₹0.01–₹25 per fetch through Setu AA; one vendor blog with an interest puts first-year cost at ₹5–25 lakh. AA history is about 2 years versus decades for CAS |
| GSP or ERI APIs | GST and income-tax APIs only through registered intermediaries |

## Account Aggregator framework
- **Roles:**
  - **FIPs** hold the data: banks, depositories, the CAMS/KFin registrars (RTAs), insurers, NPS record-keepers (CRAs), GSTN.
  - **FIUs** receive it and must be regulated entities.
  - **NBFC-AAs** carry it but are "data-blind": no financial data may sit with the AA.
- **Specs:** ReBIT v2.0.0. Consent artefacts are digitally signed. Data is encrypted between FIP and FIU.
- **Governance:** Sahamati recognised by RBI as the AA ecosystem's self-regulatory organisation on 5 Jun 2026.
- **Directions:** RBI's NBFC-AA Directions 2025 (28 Nov 2025) keep the regulated-FIU definition.
- **Live data types:** bank deposits, recurring and term deposits; equities, ETFs, REITs, InvITs and AIFs; mutual funds and SIPs; life and general insurance; NPS; GSTR-1/3B.
- **Still only proposed:** EPF, PPF, bonds, G-Secs.
- **Adoption (Jun/Jul 2026):** 2.88 billion accounts enabled; 1.97 billion consents raised, 493 million fulfilled; 295 million data shares a month; 1,076 FIUs, 176 FIPs, 17 AAs.
- **Coming:** RBI said on 7 Oct 2026 that AAs will become interoperable by 31 Dec 2026.
- **DEPA:** NITI Aayog's data-sharing architecture; it gives a user no right to act as an FIU.

## Regulation as of Oct 2026
- **DPDP Rules:**
  - Notified 13 Nov 2025, with an 18-month phased rollout.
  - Consent-manager provisions take effect 13 Nov 2026.
  - Full obligations apply from 13 May 2027: notice, consent, security safeguards, breach reporting, data-subject rights.
  - A Jan 2026 proposal to shorten this to Nov 2026 has no gazette notification yet.
- **What DPDP means for us:**
  - The personal-use exemption covers the user, not us.
  - Keeping financial data off our servers greatly reduces our duties as data fiduciary. Accounts, telemetry, crash logs and licensing data are still in scope.
  - Whether we count as a fiduciary for processing that only happens on the device is an open legal question.
- **Google Play:** SMS permission only under the money-management exception. The India personal-loan policy is stricter, so avoid any lending features.

## Best fit for our principle
1. **Core:** on-device parsers for CAS (CAMS/KFin/MF Central and NSDL/CDSL), bank and card statement PDFs, AIS/26AS, EPF, NPS and GST. This covers about 90% of net worth with nothing touching our servers.
2. **Android add-on:** SMS parsing on the device for near real-time transactions.
3. **Optional:** email import from the device (budget for a CASA assessment) and broker personal APIs.
4. **AA network:** only through a regulated partner FIU, with the device creating the encryption key. This needs legal and regulator sign-off. Until then, point users to the AA apps' self-view.

## Sources
- RBI NBFC-AA Directions 2025: https://taxguru.in/rbi/rbi-non-banking-financial-companies-account-aggregator-directions-2025.html
- Sahamati FAQ: https://sahamati.org.in/faq/ ; stats: https://sahamati.org.in/ ; data types: https://sahamati.org.in/data-fi-types-available-on-aa/
- Sahamati SRO recognition: https://sahamati.org.in/media-article/rbi-sro-recognition-account-aggregator-ecosystem/
- RBI 7 Oct 2026 announcement: https://www.outlookmoney.com/invest/rbi-account-aggregator-interoperability-bank-deposits-cas-financial-data
- Saafe end-to-end encryption: https://docs.saafe.in/fiu-module/end-to-end-encryption
- ReBIT AA API release notes: https://api.rebit.org.in/assets/ReleaseNotes.txt
- OneMoney self-consent: https://www.onemoney.in/consumers.html
- Setu AA pricing: https://setu-aa.com/pricing-policy
- casparser: https://github.com/codereverser/casparser ; blog: https://casparser.in/blog/state-of-account-aggregator-2026/
- DPDP Rules (PIB): https://www.pib.gov.in/PressReleasePage.aspx?PRID=2190014
- Google Play SMS policy: https://support.google.com/googleplay/android-developer/answer/10208820
- Zerodha personal APIs: https://zerodha.com/z-connect/updates/free-personal-apis-from-kite-connect
- AIS: https://cleartax.in/s/new-annual-information-statement-ais
