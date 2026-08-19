#!/usr/bin/env bash
# Removes the Loomix virtual audio driver from the system HAL plug-in
# directory and restarts coreaudiod. Needs sudo; briefly interrupts all
# audio on the machine.
set -euo pipefail

plugin_dir="/Library/Audio/Plug-Ins/HAL"
product_name="libLoomixAudioDriver.dylib"
installed="$plugin_dir/$product_name"

if [ ! -e "$installed" ]; then
    echo "$installed is not installed, nothing to do"
    exit 0
fi

echo "Removing $installed"
echo "This restarts coreaudiod and will briefly interrupt all audio on this machine."
sudo rm -rf "$installed"
sudo killall coreaudiod
