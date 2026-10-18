# Model comparison (T2.4, SPEC decision 33)

Small open-weight models compared for the jobs the LLM does: categorising what the rules leave in review, and (later, T4.2) turning questions into queries. The recommendation is in `report.md`. The generated tables are in `results.md` and `summary.json`.

## Files
| Path | What | In git |
|---|---|---|
| `report.md` | The analysis and the recommended model | yes |
| `results.md`, `summary.json` | Tables built by `fm-cli eval report` from `runs/` | yes |
| `runs/<run>/` | Per-transaction answers (`*.jsonl`), the run summary (`meta.json`), the questions proxy, and `/usr/bin/time -l` logs | **no**: only aggregates are committed (rebuild with `run_all.sh`) |
| `run_all.sh` | Runs every candidate in turn | yes |
| `models/` | The GGUF files, ~2–3 GB each | **no** (`.gitignore`) |

## Reproducing
```sh
cargo run -p fm-cli --release -- models fetch      # the 4 candidates, through fm-netguard, SHA-256 checked
eval/run_all.sh                                    # hours: every transaction in review, per model
cargo run -p fm-cli --release -- eval report       # results.md, summary.json
```

One model, quickly: `fm-cli eval categorize qwen3-4b-2507 --limit 25` (25 per household). CPU only: add `--cpu`. Other options: `--variants categories+income,categories`, `--threads N`, `--out RUN`, and `--check-whole`, which checks every answer against the whole prompt evaluated at once. By default only the first three per household are checked; `--no-check` checks none, since each check evaluates the whole prompt again, which is slow on a CPU.

Each run's `meta.json` records `cpu_repack`: how the llama.cpp in that binary was built (`GGML_CPU_REPACK` from its CMake cache).

The candidates are pinned in `crates/fm-cli/src/candidates.rs`: repository, revision, file, size and SHA-256. Downloads go only to the origins in `fm_llm::DOWNLOAD_ORIGINS` (SPEC decision 40).

## What is measured
- **Starting state:** TD.1's four households after transfer detection (T1.4), the rules engine, and a simulated user's 20 corrections per household. That's T2.2's `T2.2-BASELINE+T1.4` at k = 20: 4,012 of 7,376 transactions correct, 30 wrong, 3,334 in Needs review. `fm-cli` rebuilds it and checks those numbers.
- **(a) Categorisation:**
  - Each model answers every transaction still in review. The choice set is SPEC decision 45's: the categories, `income` and `uncategorised`.
  - Money in on a category counts as a refund of it; income on money out stays in review.
  - Answers are greedy and grammar-constrained (`fm_llm::Session::choose`).
  - The prompt (`crates/fm-cli/src/prompt.rs`, version `t2.4-categorise-v1`) holds the category list, the labelling conventions and the household's own corrections as examples.
  - Reported:
    - accuracy on the review set;
    - the whole ledger at confidence thresholds;
    - calibration;
    - errors, contract violations, and agreement with a fresh context;
    - the categories-only variant on 250 per household, beside it.
- **(b) Questions:** `fm-cli eval queries NAME` turns each TD.3 question into a T4.1 query (JSON-constrained, the same 8 worked examples for every model), runs it on the truth-labelled ledger and compares the answers; `fm-cli eval queries --oracle` checks the scoring with TD.3's own queries. `fm-cli eval questions NAME` is a proxy that needs no schema: sorting the questions into their types (80 scored; `hard` is a difficulty bucket, so those 20 are only timed).
- **Speed and memory:** peak RSS per run, and prefix and generation throughput from llama.cpp's counters. Speeds from the accuracy runs were taken on a busy machine; the `-speed` (Metal) and `-cpu` (CPU only, `--cpu --limit 10 --variants categories+income`) runs in quiet windows.

## Linux CPU check (manual CI job)

For the chosen model, CPU only, with and without weight repacking, on the same 40 transactions as the second quiet window. The `--cpu` flag loads no GPU layers; on Linux without a GPU nothing is offloaded. Peak memory comes from GNU time (`-v`), and the load average is recorded around each run.

The app is built without repacking (SPEC decision 48): `.cargo/config.toml` sets `GGML_CPU_REPACK=OFF`. A value in the environment wins, so the commands below build both ways. Check `cpu_repack` in each run's `meta.json`.

```sh
cargo run -p fm-cli --release --locked -- models fetch qwen3.5-4b
for repack in ON OFF; do
  out=qwen3.5-4b-linux-cpu-repack-$repack
  mkdir -p eval/runs/$out
  uptime > eval/runs/$out/load.txt
  GGML_CPU_REPACK=$repack CARGO_TARGET_DIR=target/repack-$repack cargo build -p fm-cli --release --locked
  /usr/bin/time -v target/repack-$repack/release/fm-cli eval categorize qwen3.5-4b \
    --cpu --limit 10 --variants categories+income --out $out 2> eval/runs/$out/categorize.log
  uptime >> eval/runs/$out/load.txt
  grep "Maximum resident set size" eval/runs/$out/categorize.log
done
cargo run -p fm-cli --release --locked -- eval report
```

`results.md`'s speed table then has a row per build. `GGML_CPU_REPACK` is read by llama.cpp's CMake at build time (forwarded by `llama-cpp-sys-2`), so each setting needs its own target directory.

**A target directory from before `.cargo/config.toml` keeps repacking:** `llama-cpp-sys-2` only watches the `GGML_` variables set when it was last built, so the new setting doesn't trigger a rebuild. Run `cargo clean -p llama-cpp-sys-2` once. CI fails a build that repacks: it reads `GGML_CPU_REPACK:BOOL=` from llama.cpp's `CMakeCache.txt` under `target/`.
