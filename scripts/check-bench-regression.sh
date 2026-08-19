#!/usr/bin/env bash
# Fails if the current `pr` criterion run regresses more than the given
# percentage against the checked-in baseline (spec section 4.3, bench job).
# A benchmark with no stored baseline yet is reported and skipped rather
# than failed, since the first bench for a new function has nothing to
# compare against; run scripts/save-bench-baseline.sh to create one.
set -euo pipefail

max_percent="${1:?usage: check-bench-regression.sh <max-percent-regression>}"

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
criterion_dir="$root/target/criterion"
baseline_dir="$root/testdata/bench-baseline"

shopt -s nullglob
estimates_files=("$criterion_dir"/*/pr/estimates.json)
if [ "${#estimates_files[@]}" -eq 0 ]; then
    echo "no criterion 'pr' baseline found under $criterion_dir; run 'cargo bench -p loomix-core -- --save-baseline pr' first" >&2
    exit 1
fi

failed=0
for estimates in "${estimates_files[@]}"; do
    bench_name="$(basename "$(dirname "$(dirname "$estimates")")")"
    baseline_file="$baseline_dir/$bench_name.json"

    if [ ! -f "$baseline_file" ]; then
        echo "skip: $bench_name has no stored baseline yet"
        continue
    fi

    if ! python3 - "$estimates" "$baseline_file" "$bench_name" "$max_percent" <<'PY'
import json
import sys

estimates_path, baseline_path, bench_name, max_percent = sys.argv[1:5]
max_percent = float(max_percent)

with open(estimates_path) as f:
    current_ns = json.load(f)["mean"]["point_estimate"]
with open(baseline_path) as f:
    baseline_ns = json.load(f)["mean_ns"]

regression_percent = (current_ns - baseline_ns) / baseline_ns * 100
verdict = "REGRESSION" if regression_percent > max_percent else "ok"
print(
    f"{verdict}: {bench_name} {current_ns:.3f}ns vs baseline {baseline_ns:.3f}ns "
    f"({regression_percent:+.2f}%, limit {max_percent:.0f}%)"
)
sys.exit(1 if verdict == "REGRESSION" else 0)
PY
    then
        failed=1
    fi
done

exit "$failed"
