# Model comparison (T2.4, SPEC decision 33)

**Recommendation.** Use **Qwen3.5-4B** for categorising and for questions, and apply its categories only when it is at least 80% confident; everything else waits in the review queue. Of the four small open models we tried, it gets the most right on both jobs: with it, 82.6% of a household's transactions end up correctly categorised and 2.8% wrong, against 54.4% and 0.4% with your rules and corrections alone. Its confidence also means what it says, which is what makes the 80% cut-off work; the runner-up (Qwen3-4B-2507) is about as accurate but sure of itself even when it's wrong. It takes under a second per transaction on a Mac and fits in 8 GB there. On a computer without a graphics chip it needed 6.3 GB, too much for 8 GB; switching off one llama.cpp optimisation (CPU weight repacking) brings that to 3.8 GB with the same answers, at a cost in speed: categorising a 300-transaction import takes about 6–7 minutes in the background, instead of about 3. On a real 4-core Linux machine without a GPU (T5.14), the build as shipped peaked at 3.4 GiB and took 3.6 s per transaction (about 8 minutes for that import, in the background); repacking would have used 4.7 GiB for only about 5% more speed, so switching it off costs little there. Licence: Apache-2.0.

| | Qwen3.5-4B | Qwen3-4B-2507 | Ministral-3-3B | Phi-4-mini |
|---|---|---|---|---|
| Whole ledger correct / wrong at ≥ 0.80 (rules alone: 54.4% / 0.4%) | **82.6% / 2.8%** | 83.4% / 9.6% | 71.3% / 3.3% | 68.2% / 8.7% |
| Confidence can be trusted | **yes** | no (overconfident) | yes | no |
| Questions → queries, all answers exact (a floor) | **37.5%** | 33.0% | 10.2% | 3.4% |
| Per transaction, Apple Silicon / CPU only (as built) | 0.66 s / 2.72 s | 0.52 s / 2.48 s | 0.50 s / not re-measured | 0.49 s / not re-measured |
| Peak memory, Apple Silicon / CPU only (as built) | 3.6 / 3.8 GB | 3.5 / 4.0 GB | 2.9 GB / not re-measured | 3.4 GB / not re-measured |
| Licence | Apache-2.0 | Apache-2.0 | Apache-2.0 | MIT |

**Decided:** the user chose Qwen3.5-4B at ≥ 0.80, built without CPU weight repacking (SPEC decision 48). Details below.

Pinned file: `https://huggingface.co/unsloth/Qwen3.5-4B-GGUF/resolve/e87f176479d0855a907a41277aca2f8ee7a09523/Qwen3.5-4B-Q4_K_M.gguf`, 2,740,937,888 bytes, SHA-256 `00fe7986ff5f6b463e62455821146049db6f9313603938a70800d1fb69ef11a4`.

How the numbers were made: `README.md`. Every table: `results.md` (generated). Raw per-transaction outputs stay local (only aggregates are committed).

## The candidates

| Model | Licence | Parameters | File | Peak memory |
|---|---|---|---|---|
| Qwen3.5-4B (Qwen, 2026-03) | Apache-2.0 | 4.7B | 2.74 GB | 3.44 GB |
| Qwen3-4B-Instruct-2507 (Qwen, 2025-08) | Apache-2.0 | 4.0B | 2.50 GB | 3.48 GB |
| Ministral-3-3B-Instruct-2512 (Mistral, 2025-12) | Apache-2.0 | 3.8B | 2.15 GB | 2.86 GB |
| Phi-4-mini-instruct (Microsoft, 2025-02) | MIT | 3.8B | 2.49 GB | 3.23 GB |

All four allow redistribution with no extra terms. Left out: Llama 3.2 3B (Llama licence, custom terms), Gemma 3 4B (Gemma terms), Qwen2.5-3B (non-commercial licence), Gemma 4 E4B (Apache-2.0, but 8B parameters in total: a ~5 GB file, too big for 8 GB). The Qwen and Phi GGUFs are unsloth's; the official repositories don't publish them openly. The SHA-256 pin makes the source irrelevant to what runs.

Peak memory is the whole process on Metal: the weights (memory-mapped) plus the cache for a ~2,000-token prompt. All four leave room on an 8 GB machine, measured here on a 16 GB M2.

## (a) Categorising what the rules leave in review

Starting state: TD.1's four households after transfer detection, the rules engine and 20 user corrections each (T2.2's baseline): 54.4% of 7,376 transactions correct, 0.4% wrong, 3,334 in review. Each model answers each of the 3,334, from SPEC decision 45's list (categories, income, uncategorised), greedy and grammar-constrained.

**Whole ledger at confidence ≥ 0.80** (answers below stay in review):

| Model | Correct | Wrong | In review |
|---|---|---|---|
| Rules alone | 54.4% | 0.4% | 45.2% |
| **Qwen3.5-4B** | **82.6%** | **2.8%** | 14.6% |
| Qwen3-4B-2507 | 83.4% | 9.6% | 7.0% |
| Ministral-3-3B | 71.3% | 3.3% | 25.5% |
| Phi-4-mini | 68.2% | 8.7% | 23.1% |

**Why confidence matters more than raw accuracy.** The app shows a model answer as fact only above the threshold (constraint 5). That only works if confidence means something:

| Answers given at confidence | Qwen3.5-4B right | Qwen3-4B-2507 right | Ministral-3-3B right | Phi-4-mini right |
|---|---|---|---|---|
| 0.80–0.95 | 87.7% | 35.8% | 79.3% | 48.4% |
| 0.95–0.99 | 92.9% | 54.4% | 85.0% | 63.8% |
| ≥ 0.99 | 98.7% | 81.0% | 98.6% | 79.7% |

Qwen3-4B-2507 answers almost everything at ≥ 0.99 and is wrong 19% of the time there, so no threshold brings its errors down (6.5% wrong even at ≥ 0.99). Qwen3.5-4B and Ministral-3-3B are calibrated; Qwen3.5-4B is also right more often. Phi-4-mini is wrong on more than half of what it answers and rarely says "uncategorised".

Threshold choices for Qwen3.5-4B (whole ledger, correct / wrong): ≥ 0.70 → 83.9% / 4.1%; **≥ 0.80 → 82.6% / 2.8%**; ≥ 0.90 → 79.5% / 2.1%; ≥ 0.99 → 59.4% / 0.5%. 0.80 is `FALLBACK_THRESHOLD_BP` in T2.5; a stricter setting trades coverage for fewer wrong answers.

All four: 0 errors and 0 contract violations. Offering Income (decision 45) adds 1–2 points correct and removes 1–4 points wrong for every model, compared with categories only.

### Does reusing the prompt change the answers?

Every number here comes from `fm_llm::Session`, which is also what the app uses: the shared part of the prompt (instructions, category list, examples) is evaluated once per household, and each transaction only adds its own text. An answer should be the same as evaluating the whole prompt at once (`Model::choose`).

- **Metal (all the numbers above): the same.** On 40 transactions with every answer checked against the whole prompt, Qwen3.5-4B's answers are identical, confidences included. Qwen3-4B-2507's are within 0.05 points of confidence.
- **Ministral-3-3B was re-run.** Its tokenizer puts a space token before a word at the start of a text, so the session gave it one token more than the whole prompt has. Sessions now tokenise the whole prompt and reuse only the cached tokens it starts with. Re-run: 142 of 3,334 answers changed category; at ≥ 0.80, 71.4% / 3.2% became 71.3% / 3.3%, and questions → queries went from 15 to 9 exact. All its numbers here are the re-run's. The other models' tokens were already identical.
- **CPU only:**
  - Ordinary models give exactly the whole prompt's answer since llama.cpp's flash attention is off on CPU-only machines. With it on, how the prompt was batched moved the CI test model's confidences by up to 5.5 points.
  - Qwen3.5-4B's recurrent layers still make its confidences depend on batching. On 40 transactions on pure CPU (llama.cpp built without Metal), the session's answers against the whole prompt have the same categories, but confidences differ by a median of 0.09 points and at most 13.9 points.
  - Against the Mac's Metal answers for the same 40, CPU-only gives 1 different category (below 0.80 on both, so it stays in review either way). 1 correct answer is at 0.84 on Metal and 0.74 on CPU, so it waits in review instead of being applied.
- **What it means:** a machine always gives the same answer for the same transaction and prompt. A CPU-only machine's confidences can differ from a Mac's, so answers near 0.80 can land on the other side of it: 1 in 40 here. The Linux CI run (T5.14) will show how a real Linux machine compares.

## (b) Question → query on TD.3

**A floor, not T4.2's number.** Output was constrained only to valid JSON; T4.2 constrains it to T4.1's query schema, which removes the most common failure (a query that doesn't fit the schema). Same prompt, same 8 worked examples for every model, chosen before any model ran and not scored (q002, q022, q035, q037, q040, q041, q051, q068). 88 questions scored; T4.1's two known gaps (q062, q065) listed apart. Checked by running TD.3's own queries through the same scoring: 88/88.

| Model | Valid query | Ran | All answers exact |
|---|---|---|---|
| **Qwen3.5-4B** | 70.0% | 66.7% | **33 (37.5%)** |
| Qwen3-4B-2507 | 63.3% | 57.8% | 29 (33.0%) |
| Ministral-3-3B | 25.6% | 22.2% | 9 (10.2%) |
| Phi-4-mini | 25.6% | 17.8% | 3 (3.4%) |

On a proxy that needs no schema (sorting the questions into TD.3's five types), Qwen3.5-4B is also first: 59 of 80, against 49–52 for the others.

## Speed

Measured in quiet windows (no other builds or runs; load average recorded per run) on an Apple M2 with 16 GB. 100 transactions per model on Metal; 40 on the CPU only. One evaluated prompt per household (~2,150 tokens: instructions, the category list, 20 examples) is reused for every transaction; the time to evaluate it is shown separately.

CPU only means what a computer without a graphics chip does: no GPU layers, and nothing sent to the GPU (llama.cpp otherwise hands large batches to this Mac's GPU even then). It uses the build the app ships: no weight repacking (decision 48), and no flash attention on the CPU (see "Does reusing the prompt change the answers?").

| Model | Per transaction, Metal (median) | 100 transactions, Metal | Per transaction, CPU only | 100 transactions, CPU only | Prompt once, Metal / CPU only | Peak memory, Metal / CPU only |
|---|---|---|---|---|---|---|
| **Qwen3.5-4B** | 0.66 s | 68 s | 2.72 s | 283 s | 12 s / 76 s | 3.6 / 3.8 GB |
| Qwen3-4B-2507 | 0.52 s | 62 s | 2.48 s | 262 s | 9 s / 72 s | 3.5 / 4.0 GB |
| Ministral-3-3B | 0.50 s | 54 s | not re-measured | | 9 s / | 2.9 GB / |
| Phi-4-mini | 0.49 s | 55 s | not re-measured | | 7 s / | 3.4 GB / |

Ministral-3-3B and Phi-4-mini were measured on the CPU only on an earlier build that repacked the weights (and sent large batches to the GPU): 1.67 s and 1.40 s per transaction, 5.2 and 5.9 GB. They weren't re-measured once the choice was made.

- **What this means for an import:** rules and transfer detection settle most transactions first; in TD.1 the model sees about 45%. A 300-transaction import is then about 135 model answers: about 1.5 minutes with Qwen3.5-4B on Apple Silicon; on a computer without a graphics chip, about 6–7 minutes. It runs in the background after the first fixed-logic results are shown (SPEC decision 34).
- **Qwen3.5-4B is the slowest on Metal**, by about 25% (its hybrid design and larger size); the difference is small in absolute terms.
- **The CPU-only cost is evaluating prompts**, not writing answers (17 tokens/s generated either way). Each transaction adds ~70 tokens of its own to the shared prompt, and without a GPU those take most of the time.
- **On a real Linux machine** (T5.14: GitHub's ubuntu-24.04 runner, 4 cores, 15 GB, no GPU; the same 40 transactions), the build as shipped took **3.60 s** per transaction (median; mean 3.68 s) and peaked at **3.43 GiB**. With repacking it took 3.44 s and peaked at 4.70 GiB. So on Linux, switching repacking off costs about 5% in speed for 1.3 GiB less memory, much less than the 2× this Mac's CPU showed. A 300-transaction import's ~135 model answers take about 8 minutes there, in the background. The runner was busy during the runs (load average 3.5–5.4 on 4 cores), so these timings are on the slow side and carry some noise.
- Answering questions (part b) is slower, 16–28 s per question on Metal with the generic JSON grammar; that grammar checks the whole vocabulary at every step. T4.2's schema grammar will be tighter.

## Machines without a GPU (8 GB): options

With repacking (llama.cpp's default), the CPU backend copies the weights into ordinary memory, rearranged for faster prompt evaluation. That nearly doubles the memory:

| Qwen3.5-4B, CPU only, same 40 transactions | Peak memory | Per transaction (median) | Prompt once | Generated tokens/s |
|---|---|---|---|---|
| Repacking on (llama.cpp's default) | 6.3 GB | 1.31 s | 33 s | 18.4 |
| **Repacking off (as built, decision 48)** | **3.8 GB** | 2.72 s | 76 s | 17.3 |
| Linux, 4 cores, no GPU (T5.14): repacking on | 4.70 GiB | 3.44 s | not reported | not reported |
| **Linux, 4 cores, no GPU (T5.14): repacking off (as built)** | **3.43 GiB** | 3.60 s | not reported | not reported |

On an 8 GB machine, 6.3 GB leaves too little for the app and the operating system. The options were:

| Option | Peak memory | Speed (per transaction) | Accuracy | Cost |
|---|---|---|---|---|
| **A. Don't repack the weights** (build llama.cpp with `GGML_CPU_REPACK=OFF`; weights stay memory-mapped) | **3.8 GB** | 2.72 s (about 2× slower prompts) | Same answers (39 of 40 identical in an earlier comparison) | None in code; a build setting |
| B. Smaller quant of the same model (same pinned revision) | Q4_K_S 2.59 GB file, Q3_K_M 2.29 GB (against 2.74 GB); still roughly doubled by repacking | about the same as repacking | not measured; usually a little lower | A second model file to pin and evaluate |
| C. A smaller model on CPU-only machines (Ministral-3-3B) | 5.2 GB with repacking | 1.67 s | 71.3% correct / 3.3% wrong (against 82.6% / 2.8%); weak at questions | Two models to support |
| D. Keep repacking, and raise the CPU-only minimum to 12 GB | 6.3 GB | 1.31 s | unchanged | Fewer machines supported |

llama.cpp also has a load-time switch for repacking, which would let the app choose per machine (repack when memory allows). llama-cpp-2 doesn't expose it yet, so today it is a build-time choice.

**Recommendation: A**, confirmed by the Linux CI run before release; D if its speed cost turns out too high there. B and C cost accuracy for less memory than A saves.

**Chosen: A** (SPEC decision 48). `.cargo/config.toml` sets `GGML_CPU_REPACK=OFF` for every build (dev, CI and release; with Metal nothing changes). CI and the release builds fail if llama.cpp's CMake cache under `target/` says otherwise, and each eval run records the setting (`cpu_repack` in its `meta.json`). The Linux CI timing run is T5.14.

## Reasons for the pick, and the risks

- **Most accurate on both jobs**, and calibrated: a 0.80 threshold gives 82.6% of the whole ledger right with 2.8% wrong, against 54.4% / 0.4% for rules alone.
- **Licence:** Apache-2.0, no extra terms.
- **Fast enough:** 0.66 s per transaction on Apple Silicon (about 1.5 minutes for a typical import's model share); 2.7 s on CPU only (6–7 minutes, in the background).
- **Fits 8 GB:** 3.6 GB peak on Apple Silicon, 3.8 GB on CPU only as built (6.3 GB with repacking).
- Risks:
  - It is the newest (2026-03) and a hybrid design (some recurrent layers). fm-llm restores a state snapshot for it instead of trimming the cache; on Metal its answers are identical to the whole prompt's, but it is the less-travelled path in llama.cpp, and on CPU only its confidences depend on batching (see "Does reusing the prompt change the answers?").
  - It is a thinking model; we give it an empty thinking block (its own "thinking off" form) before the answer.
  - The GGUF comes from unsloth, pinned by hash.
- **Fallback choice:** Ministral-3-3B (smallest: 2.9 GB on Metal; well calibrated, cautious) if Qwen3.5-4B turns out too slow on Linux machines without a GPU, at the cost of coverage (71.3% correct against 82.6%) and much weaker queries (10% exact against 37.5%).
- **Linux CPU confirmed (T5.14):** 3.43 GiB peak and 3.60 s per transaction as shipped on a 4-core Linux machine without a GPU, within the 8 GB budget; repacking would add 1.3 GiB for about 5% more speed.
