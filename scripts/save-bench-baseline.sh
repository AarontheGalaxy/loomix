#!/usr/bin/env bash
# Regenerates the checked-in bench baseline from the current `pr` criterion
# run. Deliberate, reviewed in the diff — spec section 4.1 layer 4 applies
# the same rule to golden audio files; this is the same idea for benches.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
criterion_dir="$root/target/criterion"
baseline_dir="$root/testdata/bench-baseline"
mkdir -p "$baseline_dir"

shopt -s nullglob
found=0
for estimates in "$criterion_dir"/*/pr/estimates.json; do
    found=1
    bench_name="$(basename "$(dirname "$(dirname "$estimates")")")"
    python3 - "$estimates" "$baseline_dir/$bench_name.json" <<'PY'
import json
import sys

estimates_path, out_path = sys.argv[1], sys.argv[2]
with open(estimates_path) as f:
    mean_ns = json.load(f)["mean"]["point_estimate"]

with open(out_path, "w") as f:
    json.dump({"mean_ns": mean_ns}, f, indent=2)
    f.write("\n")
PY
    echo "wrote $baseline_dir/$bench_name.json"
done

if [ "$found" -eq 0 ]; then
    echo "no criterion 'pr' baseline found under $criterion_dir; run 'cargo bench -p loomix-core -- --save-baseline pr' first" >&2
    exit 1
fi
