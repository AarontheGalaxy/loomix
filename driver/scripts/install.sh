#!/usr/bin/env bash
# Installs the Loomix virtual audio driver into the system HAL plug-in
# directory and restarts coreaudiod to load it (spec section 2.1). This
# briefly interrupts all audio on the machine -- needs sudo, and asks
# nothing silently.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
plugin_dir="/Library/Audio/Plug-Ins/HAL"
product_name="LoomixAudioDriver.driver"
product="$root/driver/build/Build/Products/Release/$product_name"

if [ ! -e "$product" ]; then
    echo "error: $product not found. Build it first with: just install-driver" >&2
    exit 1
fi

echo "Installing $product_name to $plugin_dir"
echo "This restarts coreaudiod and will briefly interrupt all audio on this machine."
sudo mkdir -p "$plugin_dir"
sudo rm -rf "${plugin_dir:?}/$product_name"
sudo cp -R "$product" "$plugin_dir/"
sudo killall coreaudiod

# Poll for coreaudiod to come back up and load the plug-in, rather than
# guessing a fixed delay -- under load a restart can take longer than any
# single guess, and a delay too short here means a healthy install gets
# misreported as a crash. Give it up to 15 seconds before calling it
# failed.
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
clang -Wall -Wextra -Werror -framework CoreAudio -framework CoreFoundation -o "$work/count_loomix_devices" \
    "$root/driver/tests/count_loomix_devices.c"

loomix_device_count=0
deadline=$((SECONDS + 15))
while [ "$SECONDS" -lt "$deadline" ]; do
    if loomix_device_count="$("$work/count_loomix_devices")"; then
        break
    fi
    sleep 1
done

if [ "$loomix_device_count" -eq 0 ]; then
    echo "error: zero Loomix devices visible 15s after install -- coreaudiod likely crashed on load. Check with: system_profiler SPAudioDataType" >&2
    exit 1
fi
echo "Loomix devices visible: $loomix_device_count"
