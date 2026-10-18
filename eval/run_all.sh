#!/bin/sh
# Runs the T2.4 comparison: every candidate, part (a) and the questions proxy, one model
# at a time (they share the GPU). Peak memory comes from /usr/bin/time -l.
# Usage: eval/run_all.sh [MODEL...]
set -eu
cd "$(dirname "$0")/.."
cargo build -q -p fm-cli --release --locked
cli=./target/release/fm-cli
models=${*:-"qwen3.5-4b qwen3-4b-2507 ministral-3-3b phi-4-mini"}
for m in $models; do
  dir=eval/runs/$m
  mkdir -p "$dir"
  # SPEC decision 45's choice set on every transaction in review.
  /usr/bin/time -l "$cli" eval categorize "$m" --variants categories+income \
    2> "$dir/categorize.log"
  # Categories only, on 250 per household, to show what Income adds.
  mkdir -p "eval/runs/$m-categories"
  /usr/bin/time -l "$cli" eval categorize "$m" --variants categories --limit 250 \
    --out "$m-categories" 2> "eval/runs/$m-categories/categorize.log"
  /usr/bin/time -l "$cli" eval questions "$m" 2> "$dir/questions.log"
done
