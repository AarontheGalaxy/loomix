#!/usr/bin/env bash
# Static analysis pass for the driver (spec section 4.2): clang's static
# analyzer plus a clang-tidy pass. Both are required checks -- a missing
# tool fails the run rather than silently skipping it.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
driver_dir="$root/driver/LoomixAudioDriver"
sdk="$(xcrun --sdk macosx --show-sdk-path)"

echo "== clang static analyzer =="
clang \
    --analyze \
    -Xanalyzer -analyzer-output=text \
    -std=gnu17 -Wall -Wextra -Werror \
    -isysroot "$sdk" \
    "$driver_dir/LoomixAudioDriver.c" \
    -o /dev/null

echo "== clang-tidy =="
# The Xcode command line tools don't ship clang-tidy; it comes from
# Homebrew's llvm formula, which is keg-only and not linked onto PATH.
clang_tidy="$(command -v clang-tidy || true)"
if [ -z "$clang_tidy" ] && command -v brew >/dev/null 2>&1; then
    llvm_prefix="$(brew --prefix llvm 2>/dev/null || true)"
    if [ -n "$llvm_prefix" ] && [ -x "$llvm_prefix/bin/clang-tidy" ]; then
        clang_tidy="$llvm_prefix/bin/clang-tidy"
    fi
fi

if [ -z "$clang_tidy" ]; then
    echo "error: clang-tidy not found." >&2
    echo "Install it with: brew install llvm" >&2
    echo "Then either add \$(brew --prefix llvm)/bin to PATH, or re-run this script as is -- it also checks that path directly." >&2
    exit 1
fi

"$clang_tidy" "$driver_dir/LoomixAudioDriver.c" -- \
    -std=gnu17 -isysroot "$sdk"
