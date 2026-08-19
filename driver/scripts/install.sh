#!/usr/bin/env bash
# Installs the Loomix virtual audio driver into the system HAL plug-in
# directory and restarts coreaudiod to load it (spec section 2.1). This
# briefly interrupts all audio on the machine -- needs sudo, and asks
# nothing silently.
#
# The M0 build product is a placeholder dynamic library, not yet the real
# AudioServerPlugIn bundle that M1 adds (with an Info.plist and factory
# function CoreAudio can actually load); the copy-and-restart mechanics
# here don't change when that lands, only PRODUCT_NAME below does.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
plugin_dir="/Library/Audio/Plug-Ins/HAL"
product_name="libLoomixAudioDriver.dylib"
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
