# Personal Finance Manager — Spec

Status: **MVP spec complete (decisions 1–34)**, built through a grill-me Q&A, 2026-10-11 to 2026-10-15 (restarted after the first session was lost). Remaining open items are listed at the end.
Each decision is recorded here as soon as it is made. "Open" means not decided yet.

## 0. Vision
People own their financial data, and the app adds an intelligence layer on top of it.
- Users bring data in through existing integrations and file formats. The app gives one interface that combines these formats into a single unified model.
- Analysis runs on **local AI models, always on the user's device**. Current decision models are good enough to run on moderately resourced personal devices.
- **Financial data never touches a central server.** Only account and login metadata may live centrally.

## 1. Problem, users and goals
- **Audience: a product for other people** (strangers install it and trust it with their bank data), not a personal-only tool.
  - Consequences: needs a trust story, user onboarding, support for many banks, and a defensible PII posture from day one.
- **Market: US only for v1.**

## 2. v1 scope (width and depth)
- **Build strategy: MVP first, prove it, then add features one at a time, each with proof that it works.** This applies to every scope decision below.
- **Account types: all of them eventually** (cash, credit, investments/brokerage, loans, and so on), added in increments. The data model must allow for them from the start, but the MVP starts narrow.
- Must-have: past-spend analysis, i.e. "where did the money go"
- **MVP features (decision 14):**
  1. Import, remove duplicates, and build one combined transaction record (*fixed logic*)
  2. Detect transfers between the user's own accounts, so spending isn't double-counted (*mostly fixed logic*)
  3. Clean up merchant names and categorise (*both*: rules and the user's corrections first, the LLM only for what's left; each correction becomes a rule)
  4. Spending views: by category, merchant and month; trends; month-over-month changes (*fixed logic*)
  5. Recurring charges and subscriptions (*mostly fixed logic*; the LLM labels them)
  6. Questions in plain English (*both*: LLM → structured query → numbers computed by fixed logic → LLM explains)
- **Order of increments after the MVP (decision 30):**
  1. **Planning v0, with manual inputs:** goals and scenarios using the MVP's real spending history as the baseline, plus balances, assets, income and assumptions the user types in. No investment sync and no automatic forecast needed. This is the founder's pressing problem.
     - **Scope (decision 31): (a) affordability and (b) savings goals.**
       - Affordability: "Can I afford a $X purchase or commitment, and what does it do to my runway?"
       - Savings goals: "Save $X by date Y: how much per month, and am I on track?"
     - **Shared engine:** monthly cash flow from the real spending baseline, plus manual inputs, projected forward.
     - **LLM role:** turns a plain-English scenario into parameters, which the user confirms; all arithmetic is fixed logic; every assumption is visible and editable.
     - **Not in v0:** scenario comparison, debt payoff, runway as its own feature, retirement, competing big goals.
     - **Engine defaults (decision 32):**
       - **Baseline spending:** median monthly spending over the last 6 months, excluding transfers. 3-, 6- and 12-month versions are shown side by side and the user can choose. The user can mark one-off transactions to exclude, and enter known upcoming changes.
       - **Income:** median of the last 6 months of Income transactions; the user can override it (for irregular income).
       - **Safety floor:** a minimum cash buffer the user sets; default is 3 months of baseline spending.
       - **Liquid cash:** checking and savings by default; the user can include or exclude accounts; investments are a manual figure until increment 4.
       - **Affordability verdict:** month-by-month projection → "Comfortable" / "Tight" / "Breaks the floor" (with the month). A new recurring commitment counts as an added monthly expense.
       - **Savings goal verdict:** required monthly amount vs actual average surplus → "On track" / "Behind by $X/month" / "Ahead". Several goals allowed; the app warns if together they exceed the surplus but doesn't prioritise between them.
       - **Setup:** import 12 months of CSV/OFX history on day one, because SimpleFIN only returns about 90 days; SimpleFIN keeps it current from then on.
  2. **Budgets and tracking.**
  3. **PDF statement parsing** (moved from "next" in decision 12).
  4. **Investments** (SnapTrade Personal).
  5. **Cash-flow forecasting** (recurring charges + budgets).
  6. **Planning v1:** planning is upgraded to use synced investments and the automatic forecast instead of manual inputs.
  7. **Then:** unusual-spending alerts, tags, split transactions, mobile, v2 device sync, region plug-ins, support for an existing Ollama install.
- The MVP itself stays features 1–6; nothing above moves into it.
- **Categories (decision 27):**
  - A **two-level default list the user can edit** (about 15 groups, about 60 categories), based on an openly published taxonomy such as Plaid's personal-finance categories (check the licence first) or our own similar list.
  - Users can rename, add, merge or hide categories; renaming and merging re-apply to past transactions.
  - Special buckets outside the category list: *Transfer*, *Income*, *Refund* (offsets the original category), *Needs review*.
  - The taxonomy is versioned, so evals record which version they were measured against.
  - **No split transactions in the MVP; tags for cross-cutting labels come later.** Both are later increments.
- **Counting rules for spending views and answers (decision 42):**
  1. A transaction belongs to the period of its **purchase date** when the bank gives one, otherwise its **posted date**.
  2. **Posted transactions only**; pending ones are shown separately and never included in totals (their amounts can change, e.g. tips and fuel holds).
  3. Category spending is **net of refunds and reimbursements** in that category.
  4. **Transfers never count** as spending or income: card payments, moves between own accounts, and money to or from own accounts outside the ledger. Income is the Income bucket only.
  5. Amounts are in the **account's currency**; questions about foreign purchases use the original amount the bank recorded.
- **Quality gate:** transfer detection and categorisation must meet a quality bar before the views and Q&A ship on top of them.
- **First run (decision 34):**
  1. Welcome: the promise ("your data never leaves this computer") and a link to the network log.
  2. Create the master password → show and confirm the recovery key → turn on Touch ID if available. **The model download starts in the background immediately.**
  3. Bring in data: drag-and-drop CSV/OFX (known bank formats auto-detected; unknown CSV layouts get a column-mapping screen saved per bank); optional SimpleFIN setup token, with a plain explanation that SimpleFIN and MX see the data.
  4. **First insight from fixed logic before the model is ready:** spending by month, top merchants, recurring charges, detected transfers. Categories fill in once the model is ready.
  5. Review queue: "Needs review" items in batches; each correction becomes a rule.
  6. Choose a backup folder: suggested at the end, not blocking, with reminders until set.
  7. Prompt to ask a first question ("Where did my money go last month?").
  - The **10-minute target is measured to step 4** (first insight), not to full categorisation.
- **MVP proof (decision 22):**
  - **Stage 1, technical proof: the founder is the only user, with their own real data.**
    - Pass bars (first guesses, to adjust after the first measurements):
      - The 8 hard constraints are never violated (automated checks on every run).
      - Imports succeed without manual fixes for ≥ 90% of files. Measured on the founder's own banks **plus a corpus of public and synthetic export files** covering the top ~20 US banks and card issuers, since one person's accounts can't cover them all.
      - Transfer detection reaches ≥ 95% precision.
      - ≤ 10% of model categorisations corrected after the first month.
      - ≥ 85% correct on a fixed set of 100 test questions against a synthetic dataset.
      - First useful insight within 10 minutes of install, on the 8 GB minimum machine.
    - **Overfitting guard:** with one user, tuning rules and prompts on the same months used for measurement inflates the scores. Keep at least one month held out, never used for tuning, and measure on it.
  - **Stage 2, user proof:** 15–30 US beta users who don't know the founder.
    - ≥ 60% still use it weekly after 4 weeks.
    - Most say it answered "where did the money go" better than what they used before.
    - At least a few say they would pay.
    - Metrics are computed on users' machines and shared only by choice, with a preview of what is sent.
  - **Fail rule:** if Stage 1 misses its bars, fix before Stage 2. If Stage 2 fails, change the product before adding features.

## 3. Future scope (explicitly not in v1)
- **Syncing between devices, done without a central server.** The protocol and method will be chosen in v2 after a proof of concept, judged by which best fits the user journey and experience.
- **Data persistence layer design** (raw retention, decision history, replay or rebuild) is deferred. It may become its own project: persistence and long-context memory for AI-assisted applications.
- **Formal model selection, fine-tuning, the data flywheel and the full eval pipeline** to improve analysis and assistant quality. This starts once v1 exists (the MVP only does the small comparison in decision 33).

## 4. Platforms and architecture
- **MVP runs on desktops and laptops only; mobile comes later** (decision 11). All AI runs on that machine.
- **Launch platforms: Apple Silicon macOS and Linux first.** The design must stay OS-agnostic, with Windows to follow.
- **Minimum hardware: 8 GB RAM.**
  - Implication: about 2–3 GB is realistically left for the model. That means roughly 3–4B-parameter models at 4-bit quantisation, or 7–8B models with tight memory and slower responses.
  - Linux machines with no GPU will run inference on the CPU.
- **Tech stack (decision 23): Tauri 2, with a Rust core and a Svelte + TypeScript UI.**
  - The **Rust core is a separate library** holding the ledger, import pipeline, contract checks, model calls (`llama-cpp-2`), encrypted storage (`rusqlite` with bundled SQLCipher) and the network allowlist. The UI is a thin layer on top.
  - The hard constraints are tested as ordinary Rust tests on the core.
  - The same core can later power mobile (Tauri 2 iOS/Android) and a command-line tool for the eval pipeline.
  - Chosen partly for low memory use, which matters on 8 GB machines.
- **Supply-chain pinning (decision 35): all CI and tooling is pinned to an exact, immutable reference.**
  - GitHub Actions: pinned by full commit SHA, with the version in a comment.
  - Rust toolchain: exact release in `rust-toolchain.toml` (rustup verifies its SHA-256 manifests; `rust-toolchain.toml` cannot take a commit hash).
  - Rust crates: `Cargo.lock` with checksums, built with `--locked`; any git dependency uses `rev = "<sha>"`.
  - JavaScript (Svelte UI): exact versions plus a lockfile with integrity hashes; installs use the frozen lockfile.
  - Model files: GGUF SHA-256 (decision 33).
  - **Known gap:** GitHub-hosted runner images (`macos-14`, `ubuntu-24.04`) can only be chosen by label, not by hash. Linux jobs could later run in a container image pinned by digest.
- **Commits (decision 36):**
  - Every commit is **cryptographically signed** (SSH signing key) and carries a **DCO `Signed-off-by`** trailer, from every session.
  - Merged branches and their worktrees are deleted right after merging.
- **Linear history on `main` (decision 38):** every task branch lands as **one squash commit** on `main` (signed, signed off, message naming the task). There are no merge commits, and the task branch is rebased onto `main` first so the squash applies cleanly.
- **Model format: GGUF. Runtime: llama.cpp embedded in the app** (decision 13). One install; the app downloads and pins exact model files (Metal on Apple Silicon; CPU, with optional CUDA/Vulkan, on Linux).
  - Using an Ollama install the user already has is a possible later increment, not the MVP.
- **Distribution: local-first app with an optional paid tier** (decision 2). The free app keeps data on the device.
  - Pricing is deferred; the hypotheses to test are under "Open tensions" (decision 25).
- **No central server, accounts or login in the MVP** (decision 21). A central server holding only account and login metadata (never financial data) arrives when sync (v2) or paid features need it.
- **Fully open source** (decision 18). Anyone can check the privacy claims.
- **Model weights we train or fine-tune are open** (decision 25), consistent with the fully open project.
- **Licence (decision 24): split.**
  - **AGPL-3.0** for the app and core, so no one can turn it into a closed or hosted product.
  - **Apache-2.0** for standalone reusable libraries (file parsers such as OFX/CSV/CAMT, format converters, the contract checker).
  - **Contributions under a DCO sign-off for now,** not a CLA. This means contributed code can't be re-licensed later; revisit if dual licensing becomes the business model.
- **Dependency licence policy (decision 37), enforced by `cargo-deny` in CI:**
  - **Allowed for any dependency:** permissive licences only (MIT, Apache-2.0 and Apache-2.0 WITH LLVM-exception, BSD-2-Clause, BSD-3-Clause, ISC, Zlib, Unicode-3.0, CDLA-Permissive-2.0).
  - **MPL-2.0 is allowed only as named per-crate exceptions,** each reviewed. It's file-level copyleft and compatible with AGPL-3.0; only changes to the MPL files themselves must be shared. The first exceptions are the five crates Tauri needs: `cssparser`, `cssparser-macros`, `selectors`, `dtoa-short`, `option-ext`.
  - **Any other licence, or any new MPL crate, fails CI** until it's reviewed and added here.

## 5. Data connectors / ingestion
- Hard rule: no connector may route financial data through our servers.
- Research for every region is done: `research/connectors.md` (synthesis) plus files per region.
  - Key finding: **on-device file and statement import is the only route that works in every region**, so it is the backbone.
  - Live sync is a per-region plug-in that uses each user's own keys (SimpleFIN, SnapTrade Personal, Enable Banking, Pluggy, broker APIs).
  - Mobile-only sources (FinanceKit, Android SMS) wait for mobile.
- **MVP connectors (decision 12):**
  - File import: CSV (with saved per-bank column mappings) and OFX/QFX.
  - **SimpleFIN** for live bank and card sync (US). The UI states plainly that SimpleFIN and MX see the data; we don't.
  - Unified data model built for multiple currencies and regions from day one, even though v1 is US-only.
- **PDF statement parsing with the local model** is the first proof of the AI layer. Now increment 3 (see decision 30).
- **Later increments, each needing proof first:** SnapTrade Personal (investments); region plug-ins (Enable Banking EEA, India CAS/AIS); users' own Teller/Plaid keys.
- **Ruled out:** licensed aggregators through our backend; OFX Direct Connect (dying); CFPB 1033 as a dependency.

## 6. LLM vs fixed-logic parts
- All model inference is local, with no cloud LLM calls on financial data.
- **Rule: the LLM never produces a number** (decision 15). Every figure comes from fixed logic over the stored transactions.
- **The LLM's jobs:** categorising transactions nothing else can classify, labelling recurring charges, turning questions into structured queries, and explaining results in plain language.
- **Reproducibility means keeping the contract, not byte-exact output** (decision 19). Given the same input data and user inputs, the app must produce output *within the constraints of the output contract* (decision 20). Re-running a model may phrase or decide things differently, but must never break the contract.
- **Output contract (decision 20):**
  - **Hard constraints** (checked in code; any violation is a bug):
    1. **Numbers are exact.** Every figure comes from fixed logic; the same ledger and decisions always give the same totals.
    2. **User inputs always win.** Corrections, rules and account designations are never overridden by a model re-run.
    3. **Models pick from a closed list.** A model chooses only from the user's category list or "uncategorised", enforced at generation time with a llama.cpp grammar or JSON schema.
    4. **Everything adds up.** Every transaction is in exactly one bucket (a category, a transfer, or "needs review"). Category totals plus transfers equal total money out. Matched transfers net to zero.
    5. **Admit uncertainty.** Below a confidence threshold, a model decision becomes "needs review", never a guess shown as fact.
    6. **Explanations match the numbers.** Every number in an LLM-written explanation is extracted and checked against computed values; on a mismatch the app regenerates or falls back to a template.
    7. **No silent changes.** Model decisions the user has already seen are never changed silently; re-run changes appear as suggestions to review. A decision counts as seen only once the user acts on it: confirms or corrects it, one at a time or with bulk approve/reject over several transactions (decision 46).
    8. **Every decision shows its source:** a rule, the user, or the model (with model identity and confidence).
  - **Soft targets** (measured; thresholds set from the MVP's baseline): re-run agreement, user correction rate, question-to-query accuracy, and time to categorise a typical import on the 8 GB minimum machine.
- **MVP model choice (decision 33): a small comparison during the MVP build** (about a week). It is the first version of the eval pipeline.
  - 3–4 small open-weight instruct models (about 3–4B parameters, 4-bit GGUF) run through the same synthetic test set: the 100 Stage 1 questions plus a few hundred synthetic transactions to categorise.
  - **Picked on:** accuracy; hard-constraint violations (expected zero thanks to grammar-constrained output); speed on an 8 GB Apple Silicon machine and on a Linux machine running on CPU; licence.
  - **Rules:** the licence must allow redistribution; pin the exact GGUF file hash; record why the winner won.
  - Specific candidates are chosen at build time, because small models change quickly.
  - Using different models for different jobs is a later optimisation.
- **What the AI categoriser may choose (decision 45):** the user's categories, **Income**, or **uncategorised** (Needs review). It never chooses **Transfer**: transfers come only from transfer detection, the user's rules and the user's own corrections, because a wrong "transfer" silently hides real spending. Money in on a chosen category is recorded as a refund of that category.
- **Training and eval data (decision 26):**
  - **Released (open) weights are trained only on synthetic and public data.** Never on real user data, the founder's included, because models can memorise and leak training data.
  - **Evaluation uses the founder's own data locally** (Stage 1) **plus federated evaluation:** consenting users' apps run test sets locally and share only scores.
  - **Opt-in data donations are deferred.** If ever added, they are recorded as an explicit exception to the "nothing leaves" principle, with a scrubbing preview and consent for each donation.
  - **Two flywheels:** user corrections improve that user's local rules immediately (private); the public model improves through synthetic data plus federated scores.
  - Federated learning is research territory, not v1.
- **What this makes measurable for later evals:** categorisation accuracy, whether questions become the right query, and whether explanations stay faithful to the computed numbers.

## 7. PII and security posture
- Financial data stays private to the user. It is never stored on a central server.
- **Encryption at rest (decision 16):**
  - A master password is required every time the app launches. Argon2id turns it into a key-encryption key, which unwraps a random **data key**, which encrypts the database (SQLCipher, AES-256).
  - The password is never stored anywhere. The data key lives only in memory while unlocked and is wiped on lock, close or idle timeout (auto-lock).
  - The database holds everything, including connector credentials such as the SimpleFIN access URL.
  - The app enforces a minimum password strength: **at least 12 characters and a zxcvbn score of 4 (of 4)** (decision 41). Failures explain why and suggest a passphrase.
  - **Auto-lock after 10 minutes idle by default** (choices: 1, 5, 10, 30, 60 minutes), and on quit (decision 41).
- **Touch ID on macOS:**
  - A copy of the data key is stored in the Keychain, locked to the Secure Enclave and requiring a *current* fingerprint match. Enrolling a new fingerprint invalidates it.
  - The password is still required after a restart, after 5 failed scans, and every 7 days.
  - Macs without Touch ID use the password.
- **Linux:** password only in the MVP. fprintd is only a software gate, so it isn't used; TPM-backed unlock may be looked at later.
- **Recovery key: required.**
  - Shown once at setup; the user must confirm they saved it (for example by typing part of it back).
  - It also unlocks the data key. Losing both the password and the recovery key means the data is gone permanently; we cannot recover it.
- **Central account login, when accounts exist (v2+): passkeys** (Touch ID or phone), so the account server has no passwords to leak.
- **Network policy (decision 21):**
  - **Strict allowlist enforced in one place:** a small open-source network module is the only code allowed to make network connections. Anything not on the list is blocked and logged.
  - **Allowed:**
    - SimpleFIN, user-initiated and only to the user's own bridge.
    - Model and app-update downloads with no identifiers or cookies, checked against published hashes and signatures.
    - **Model files come from Hugging Face (decision 40):** `https://huggingface.co` plus the Hugging Face CDN hosts its downloads redirect to, each as an exact origin. We don't host model weights. The CDN host can vary by region and storage backend, so the list lives in one constant in `fm-llm`; a blocked redirect names the host so the list can be extended deliberately. Every file is still checked against its pinned SHA-256.
  - **Crash reports:** off by default. When one occurs the user sees exactly what would be sent and sends it manually; transaction content, amounts and merchant names are scrubbed.
  - **"Sent manually" means the app never uploads crash reports (decision 39).** The preview offers *Copy report*, shows the report file's path (to attach to a GitHub issue or email) and *Delete*. No crash-report address is on the allowlist.
  - **No analytics in the MVP.** Product learning comes from opt-in interviews and feedback.
  - **Visible network log in the app:** every request, when, and to where.
- **Threat model:**
  - Protects against: stolen device or disk, other users on the same machine, a cloud service copying the app's folder, malware reading files while the app is locked.
  - Does not protect against: malware running while the app is unlocked, keyloggers, coercion.

## 8. Data storage and retention (local vs cloud)
- Financial data is stored locally on the user's devices only.
- **Backups (decision 17):**
  - Automatic encrypted backups from the app to a **folder the user chooses** (external drive, or a cloud-synced folder like iCloud Drive, Dropbox or Google Drive).
  - Taken daily and after each import; copies kept are set in decision 28 (7 daily, 4 weekly, 12 monthly).
  - Restore on a fresh install with the master password or the recovery key.
  - Backups are the already-encrypted database, so wherever they're stored they're unreadable without a key.
- **We never host backups, not even encrypted ones** (option c was rejected). Storing them in the user's own cloud is the user's choice and never passes through us.
- **Retention and deletion (decision 28):**
  - **Keep everything until the user deletes it,** including original files and SimpleFIN responses (SimpleFIN only gives about 90 days, so discarded history can't be fetched again).
  - **Selective deletion:** by import, account or date range. Everything derived from it goes too (transactions, model decisions, rules learned only from that data).
  - **"Delete everything" destroys the keys first:** the data key, Keychain entry and recovery-key copy are wiped, then the files are deleted. Leftover copies become permanently unreadable, and the user's saved recovery key no longer unlocks anything; the app says so.
  - **Backup copies kept:** the last 7 daily, 4 weekly and 12 monthly in the user's folder. Deleting data doesn't reach older backups, so the app warns and offers to delete the backups in that folder too. Copies the user moved elsewhere are outside the app's control, and the app says so.
  - **Deleting by account or date range (decision 43):** transactions in scope are deleted with everything derived from them. Original files entirely in scope are deleted too. Files only partly in scope are listed with an **opt-in** choice to delete them whole (showing the extra transactions that would remove). When a file is kept, the deleted rows are recorded so they never come back from that file, and the app says the original still contains them.
  - **Re-importing over a deleted period (decision 44):** a later file that overlaps a period the user deleted (a re-saved copy or an overlapping export) is imported in full, with a **warning** that it covers a deleted period. **TODO (later):** refine, e.g. skip those rows by default with "bring them back".
  - **No automatic expiry in the MVP;** a "delete data older than N years" setting may come later.

## 9. How it differs from existing apps (Actual Budget, etc.)
- **Positioning line (decision 29):** *"Ask anything about your money. The AI runs on your computer, your data never leaves it, and you can check that yourself."*
- **What makes it distinctive:** the only option combining (1) AI-driven analysis, (2) data that stays on the device with nothing to self-host, and (3) claims you can check (open code and weights, in-app network log, published output contract).
- **Comparison (from memory, 2026-10-15; verify before using publicly):**

  | | This app | Actual Budget | Firefly III | GnuCash / hledger / Beancount | Monarch / YNAB / Copilot / Rocket Money |
  |---|---|---|---|---|---|
  | Data lives | Device only | Device + self-hosted server for sync/bank links | Self-hosted server | Device | Company cloud |
  | AI | Local models, contract + evals | None built in | None built in | None | Cloud AI |
  | Main idea | Analysis and questions; budgeting/planning/forecasting to come | Envelope budgeting | Double-entry bookkeeping | Double-entry bookkeeping | Budgeting and tracking |
  | Licence | AGPL/Apache, open weights | MIT | AGPL | GPL / open | Closed |
- **Non-goals:** not bookkeeping software, not a cloud service, no self-hosting required.
- **On the roadmap, not non-goals:** budgeting, tracking, planning and forecasting are planned increments after the MVP. They are the **founder's own main use cases.**

## Open tensions to resolve
- ~~**Local AI vs cross-platform**~~ — resolved by decision 11: desktops only for the MVP.
- **Paid tier contents:** pricing deferred (decision 25). Hypotheses to test in Stage 2:
  - **(1) Paid convenience builds:** signed, auto-updating installers or app-store versions; building from source stays free.
  - **(2) Curated model packs.** Because the weights are open, these can only be sold as *convenience*: pre-packaged, eval-tested, auto-updated, and possibly available earlier than the public release.

  Sponsorship and donations run from day one.
- ~~**Data flywheel vs privacy**~~ — resolved by decision 26.
- ~~**Account recovery**~~ — resolved: a forgotten password is covered by the recovery key (decision 16); a lost device by user-chosen encrypted backups (decision 17).

## Still open (not blocking the MVP build)
- **Pricing:** test hypotheses (1) and (2) in Stage 2.
- **v2 device sync protocol:** chosen after a proof of concept.
- **Persistence-layer design:** possibly its own project.
- **Windows support:** timing.
- **Multi-currency details:** the data model allows it; v1 is USD only.
- **Release logistics:** macOS code signing and notarisation, Linux packaging (AppImage, Flatpak or .deb), auto-update signing.
- **Federated evaluation protocol for Stage 2:** what is shared, the preview UI, and how scores are sent without a central server holding financial data.
- **Checks before relying on them:**
  - licence of the category taxonomy source (e.g. Plaid's)
  - competitor facts in §9
  - for later regions: Enable Banking terms, India FIU partnership legality, FinTS licensing
- **Accessibility and localisation:** not yet discussed.

## Decision log
| # | Date | Decision | Why |
|---|------|----------|-----|
| 1 | 2026-10-11 | Audience: a product for other people (option c) | User's choice; the recommendation was household-first, but the user chose a product for others |
| 2 | 2026-10-11 | Local-first app with an optional paid tier | Strongest trust story; matches the data-ownership vision |
| 3 | 2026-10-11 | US only for v1 | Narrows which connectors are possible |
| 4 | 2026-10-11 | Financial data never stored on a central server; only login and account metadata is central | Core principle: users own their data |
| 5 | 2026-10-11 | Analysis uses local AI models only | Privacy; modern models run on personal devices |
| 6 | 2026-10-11 | Serverless device sync deferred to v2, chosen after a proof of concept | Pick the protocol based on user experience, not up front |
| 7 | 2026-10-11 | Model choice, data flywheel and eval pipeline come after v1 | Ship v1 first |
| 8 | 2026-10-11 | Build strategy: MVP, then prove it, then add features one at a time with proof | Lower risk; every feature has to earn its place |
| 9 | 2026-10-11 | Support all account types eventually, added in increments | Full picture of the user's money, without bloating the MVP |
| 10 | 2026-10-11 | Connector choice deferred until EU, India and global research is done | Want the global landscape before committing |
| 11 | 2026-10-11 | MVP on desktops/laptops only; Apple Silicon macOS and Linux first; OS-agnostic design; 8 GB RAM minimum; GGUF models (llama.cpp/Ollama family) | Prove local AI where memory and screen space exist; GGUF keeps models portable across OSes |
| 12 | 2026-10-11 | MVP connectors: CSV + OFX/QFX import and SimpleFIN; PDF parsing next; SnapTrade and region plug-ins later | File import is the only universal route; SimpleFIN gives live US sync without our servers |
| 13 | 2026-10-11 | Embed llama.cpp in the app instead of requiring Ollama | One-step onboarding for ordinary users; pinned model versions for reproducible evals |
| 14 | 2026-10-11 | MVP features 1–6 (import, transfers, categorisation, spending views, recurring, Q&A); budgets, forecasting and alerts later | The minimum needed to answer "where did the money go" correctly |
| 15 | 2026-10-11 | The LLM never produces a number; all figures come from fixed logic | Prevents made-up totals; gives clear things to measure |
| 16 | 2026-10-11 | Master password every launch (Argon2id → data key → SQLCipher); Touch ID on macOS through a Secure Enclave–gated key; password only on Linux; recovery key required; passkeys for the account login | User wants the strictest option plus fingerprint convenience; recovery key avoids permanent loss from a forgotten password |
| 17 | 2026-10-11 | Automatic encrypted backups to a folder the user chooses; we never host backups, not even encrypted | Covers device loss without our servers ever holding financial data |
| 18 | 2026-10-11 | Fully open source (licence still open) | Privacy claims can be checked, not just promised |
| 19 | 2026-10-13 | Reproducibility means keeping the output contract given the same inputs and user inputs, not byte-exact output; persistence-layer design deferred, possibly its own project | Model outputs aren't bit-reproducible; what matters is that outputs stay within defined constraints |
| 20 | 2026-10-13 | Output contract: 8 hard constraints checked in code, plus soft quality targets set from the MVP baseline | Defines what "reproducible" means; you can't set thresholds before measuring |
| 21 | 2026-10-14 | Strict network allowlist in one open module; crash reports opt-in with preview; no analytics; no central server or accounts in the MVP; in-app network log | Lets users check the "nothing leaves" claim; less to build and secure |
| 22 | 2026-10-14 | Two-stage MVP proof: Stage 1 = founder only (own data + public/synthetic file corpus + held-out month); Stage 2 = 15–30 beta users | Clear pass or fail before adding features; metrics stay on users' devices |
| 23 | 2026-10-14 | Tauri 2: separate Rust core library plus a Svelte + TypeScript UI | Low memory on 8 GB machines; mature Rust libraries for llama.cpp, SQLCipher, Keychain; core reusable on mobile and in a command-line tool |
| 24 | 2026-10-14 | Split licence: AGPL-3.0 for app and core, Apache-2.0 for reusable libraries; DCO for contributions | Blocks closed or hosted forks; libraries spread goodwill; DCO is contributor-friendly |
| 25 | 2026-10-15 | Defer pricing; test paid convenience builds and curated model packs in Stage 2; sponsorship from day one; trained model weights are open | Prove the MVP before pricing; open weights keep the project fully open |
| 26 | 2026-10-15 | Train released weights only on synthetic + public data; evaluate on founder's local data + federated scores; defer donations; private per-user flywheel through corrections | Open weights can leak training data; keeps the "nothing leaves" principle intact |
| 27 | 2026-10-15 | Two-level editable category list from an open taxonomy, versioned; special buckets; no splits in MVP; tags later | Good defaults help both users and the model; versioning keeps evals comparable |
| 28 | 2026-10-15 | Keep until the user deletes; selective deletion cascades; full delete destroys keys first; backups kept 7 daily / 4 weekly / 12 monthly with a warning on deletion; no auto-expiry in the MVP | Lost history can't be re-fetched; deletion must be real and honest about backup copies |
| 29 | 2026-10-15 | Positioning line and non-goals adopted; budgeting, tracking, planning and forecasting are planned increments (the founder's main use cases) | Clear identity without ruling out the founder's core needs |
| 30 | 2026-10-15 | Post-MVP order: planning v0 (manual inputs) → budgets → PDF parsing → investments → forecasting → planning v1 (automated inputs) → the rest | Planning is the founder's pressing problem; manual inputs remove the dependency on investments and forecasting |
| 31 | 2026-10-15 | Planning v0 scope: affordability + savings goals, on one cash-flow engine | The founder's pressing problems; small to build; retirement left for later |
| 32 | 2026-10-15 | Planning engine defaults: 6-month median baseline, 3-month safety floor, three-level verdicts, 12 months of file history imported at setup | Median resists one-off spikes; SimpleFIN's 90 days is too short for a baseline |
| 33 | 2026-10-15 | MVP model picked by a small comparison of 3–4 small open-weight models on the synthetic test set; licence must allow redistribution; pinned hash | Evidence-based pick that doubles as the first eval pipeline |
| 34 | 2026-10-15 | First-run flow: model downloads in the background during setup; first insight from fixed logic before the model is ready; backups suggested but not blocking, with reminders | Meets the 10-minute target without skipping security steps |
| 35 | 2026-10-16 | Pin all CI and tooling to immutable references: actions by commit SHA, exact toolchain, lockfiles with checksums, git deps by rev, models by hash | Supply-chain safety for a privacy product; runner images are the one known gap |
| 36 | 2026-10-16 | All commits signed (SSH) with a DCO sign-off, from every session; merged branches and worktrees deleted after merge | Verifiable history; matches the DCO policy; avoids branch clutter |
| 37 | 2026-10-16 | Dependency licences: permissive allow list; MPL-2.0 only as reviewed per-crate exceptions (first five: Tauri's cssparser, cssparser-macros, selectors, dtoa-short, option-ext); anything else fails CI | Tauri needs these crates; per-crate exceptions keep copyleft from spreading unnoticed |
| 38 | 2026-10-16 | Squash-merge task branches; keep `main` history linear (no merge commits) | One reviewable commit per task; simple, bisectable history |
| 39 | 2026-10-16 | Crash reports are never uploaded by the app: Copy report, file path shown, Delete; no crash-report origin on the allowlist | MVP has no server; keeps the "nothing leaves" promise checkable |
| 40 | 2026-10-16 | Model downloads come from Hugging Face: huggingface.co + its CDN hosts as exact allowlisted origins, kept in one fm-llm constant; no self-hosted weights | Hosting weights ourselves adds cost and a server for no privacy gain; the SHA-256 pin protects integrity |
| 41 | 2026-10-16 | Password policy: ≥ 12 characters and zxcvbn score 4; auto-lock after 10 min idle (1/5/10/30/60) and on quit; Touch ID stays a persistent Keychain item as written in §7 | Score 3 still admits passwords crackable offline in hours; passphrases reach 4 easily |
| 42 | 2026-10-17 | Counting rules: purchase date when present (else posted); posted only, pending shown separately; net of refunds; transfers never spending or income; account currency with original amounts for foreign questions | One definition of "correct" shared by views, answers and the test set |
| 43 | 2026-10-17 | Deleting by account or date range: delete in-scope transactions; delete fully-in-scope files; partly-in-scope files deleted whole only by opt-in; kept files record their deleted rows so they never come back | Deletion stays real without silently removing transactions the user didn't ask to delete |
| 44 | 2026-10-17 | A file overlapping a deleted period imports everything, with a warning; refining this is a TODO | Simple and transparent for the MVP |
| 45 | 2026-10-17 | The AI categoriser chooses from categories + Income + uncategorised; never Transfer (rules, detection and the user only) | A wrong transfer hides real spending; income in review would otherwise always be wrong |
| 46 | 2026-10-17 | A model decision counts as "seen" only when the user acts on it: confirms or corrects it; bulk approve/reject covers several transactions at once | Being shown a decision in a list isn't agreement; acting on it is, and bulk actions keep review fast |
