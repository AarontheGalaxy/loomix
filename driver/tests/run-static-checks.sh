#!/usr/bin/env bash
# Static analysis pass for the driver (spec section 4.2): clang's static
# analyzer plus a clang-tidy pass. clang-tidy ships with the Xcode command
# line tools on some machines and not others, so its absence is a skipped
# check here, not a failure -- the compiler's own -Wall -Wextra -Werror in
# the Xcode build is what actually gates CI.
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
if command -v clang-tidy >/dev/null 2>&1; then
    clang-tidy "$driver_dir/LoomixAudioDriver.c" -- \
        -std=gnu17 -isysroot "$sdk"
else
    echo "clang-tidy not found on this runner, skipping"
fi
