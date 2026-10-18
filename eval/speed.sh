#!/bin/sh
# Speed runs for T2.4, in a quiet window: per model, 100 transactions on Metal (25 per
# household) and 40 CPU only (10 per household), with the load average before and after.
# Usage: eval/speed.sh [MODEL...]
set -eu
cd "$(dirname "$0")/.."
cli=./target/release/fm-cli
models=${*:-"qwen3.5-4b qwen3-4b-2507 ministral-3-3b phi-4-mini"}
for m in $models; do
  for mode in speed cpu; do
    dir=eval/runs/$m-$mode
    mkdir -p "$dir"
    uptime > "$dir/load.txt"
    if [ "$mode" = cpu ]; then
      flags="--cpu --limit 10"
    else
      flags="--limit 25"
    fi
    # shellcheck disable=SC2086
    /usr/bin/time -l "$cli" eval categorize "$m" --variants categories+income $flags \
      --out "$m-$mode" 2> "$dir/categorize.log"
    uptime >> "$dir/load.txt"
  done
done
