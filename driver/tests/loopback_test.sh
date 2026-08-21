#!/usr/bin/env bash
# M1 manual smoke test (spec section 3.4): play a known signal into
# "Loomix In 1" from one client, capture it back with another, over two
# 30-second segments at different sample rates (44.1/48/etc through
# 192 kHz, spec section 1.11) -- 60+ seconds total, long enough for a
# transient bug to show up rather than average away. Needs the driver
# already installed (spec layer 7's "real macOS runner with the driver
# installed"), so unlike driver/tests/test_ring_buffer.c this does not run
# in CI -- run it by hand after `just install-driver`.
#
# test_ring_buffer.c, not this script, is the load-bearing proof that the
# ring buffer never loses, duplicates or reorders a sample: it drives
# RingBuffer.c directly with synthetic sample times, deterministically,
# with no CoreAudio and no installed driver. This script is a real
# end-to-end check on top of that, but it cannot assert literal
# byte-for-byte equality from sample zero, and it changes the sample rate
# between two discrete capture segments rather than while a client has one
# open. Two findings shaped that:
#
#   1. Dropped buffers under load. The same harness run against BlackHole
#      (a known-good, widely used reference driver, already installed on
#      this machine) shows an identical failure signature to an early,
#      failing version of this test -- a clean forward gap of exactly the
#      capture chunk size, never a duplicate or a rewind. A bug specific
#      to this driver would not also reproduce against a different driver
#      under the same test code; a lossy test harness would. It's the
#      latter: the OS occasionally drops a whole capture buffer between
#      the device and the ffmpeg client process under system load,
#      upstream of anything this driver controls. So the bar for what a
#      capture DOES receive is: a duplicate or a rewind is a hard failure,
#      a forward gap is not.
#
#   2. Live reconfiguration. An earlier version of this test changed the
#      sample rate mid-segment, with ffmpeg's capture and playback
#      processes both still attached. The capture that followed was
#      heavily corrupted -- not a brief blip at the boundary, but roughly
#      a third of everything captured afterward, as nonsense values no
#      plausible driver bug produces (arbitrary large floats, not
#      duplicated or skipped ramp values). The same test against BlackHole
#      reproduces it: ~35% corruption starting right after the same live
#      rate change. Neither driver's client-facing behavior is being
#      exercised in a realistic way here -- a real client (this project's
#      own mixer app included) tears IO down and rebuilds it around a
#      format change; it doesn't expect an open capture handle to survive
#      the device changing rate underneath it. So this test changes the
#      rate between two closed segments, matching spec section 1.11's
#      claim ("supports 44.1 through 192 kHz") without also asserting the
#      unrelated, harder claim that a live client survives a live
#      reconfiguration.
#
# See docs/ARCHITECTURE.md for the dated record of both findings.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
work="$(mktemp -d)"
cleanup() {
    # Leave the device the way this test found it rather than stuck at
    # whatever rate the last segment used.
    [ -x "$work/set_sample_rate" ] && "$work/set_sample_rate" 48000 >/dev/null 2>&1
    rm -rf "$work"
}
trap cleanup EXIT

if ! command -v ffmpeg >/dev/null 2>&1; then
    echo "error: ffmpeg not found. Install it with: brew install ffmpeg" >&2
    exit 1
fi

clang -Wall -Wextra -Werror -framework CoreAudio -framework CoreFoundation -o "$work/count_loomix_devices" \
    "$root/driver/tests/count_loomix_devices.c"
clang -Wall -Wextra -Werror -framework CoreAudio -framework CoreFoundation -o "$work/query_device_stats" \
    "$root/driver/tests/query_device_stats.c"
clang -Wall -Wextra -Werror -framework CoreAudio -framework CoreFoundation -o "$work/set_sample_rate" \
    "$root/driver/tests/set_sample_rate.c"

# Fail immediately and unambiguously if the driver crashed coreaudiod or
# never published a device, rather than have that show up 30+ seconds from
# now as an ffmpeg device-index lookup coming up empty.
loomix_device_count="$("$work/count_loomix_devices")" || {
    echo "FAIL: zero Loomix devices visible to CoreAudio -- coreaudiod likely crashed on load or never published them. Check with: system_profiler SPAudioDataType" >&2
    exit 1
}
echo "Loomix devices visible: $loomix_device_count"

# Force a known starting rate rather than trusting whatever a previous run
# (or a crash before its cleanup trap ran) left the device at.
"$work/set_sample_rate" 48000

read -r safety_offset nominal_rate < <("$work/query_device_stats")
echo "== before =="
echo "input SafetyOffset: $safety_offset frames"
echo "nominal sample rate: $nominal_rate Hz"
if [ "$safety_offset" -eq 0 ]; then
    echo "FAIL: input SafetyOffset is 0, no margin between write and read" >&2
    exit 1
fi

# ffmpeg deliberately exits non-zero after -list_devices (it then fails to
# open the dummy input); wrap it so that expected failure doesn't trip
# set -e/pipefail before grep and sed get to run.
avfoundation_index="$({ ffmpeg -hide_banner -f avfoundation -list_devices true -i "" 2>&1 || true; } \
    | grep -E '\] Loomix In 1$' | sed -E 's/.*\[([0-9]+)\].*/\1/')"
audiotoolbox_index="$({ ffmpeg -hide_banner -f lavfi -i "sine=duration=0.01" -f audiotoolbox -list_devices true -y "$work/probe" 2>&1 || true; } \
    | grep -E 'com\.loomix\.audiodriver\.in1$' | sed -E 's/.*\[([0-9]+)\].*/\1/')"

if [ -z "$avfoundation_index" ] || [ -z "$audiotoolbox_index" ]; then
    echo "error: Loomix In 1 not found by ffmpeg -- is the driver installed?" >&2
    exit 1
fi

segment_seconds=30
capture_seconds=33
lead_in_seconds=1
overall_failed=0

# Each channel carries its own frame index as a literal Float32 value
# (ffmpeg's aevalsrc 'n' expression) -- unlike a tone, every sample is
# self-identifying, so a duplicate, gap or reorder anywhere is directly
# visible in the numbers rather than inferred from smoothness.
run_segment() {
    local rate_hz="$1"
    local label="$2"

    echo "== ${segment_seconds}s ramp loopback at ${rate_hz} Hz =="
    ffmpeg -hide_banner -loglevel error -y \
        -f lavfi -i "aevalsrc=exprs='n|n':s=${rate_hz}:d=${segment_seconds}" \
        -acodec pcm_f32le "$work/reference-$label.wav"

    ffmpeg -hide_banner -loglevel error -y \
        -f avfoundation -audio_device_index "$avfoundation_index" -i "none:$avfoundation_index" \
        -t "$capture_seconds" -acodec pcm_f32le "$work/capture-$label.wav" &
    local capture_pid=$!

    sleep "$lead_in_seconds"
    ffmpeg -hide_banner -loglevel error -y \
        -i "$work/reference-$label.wav" -acodec pcm_f32le \
        -f audiotoolbox -audio_device_index "$audiotoolbox_index" -

    wait "$capture_pid"

    ffmpeg -hide_banner -loglevel error -y -i "$work/capture-$label.wav" -f f32le -acodec pcm_f32le "$work/capture-$label.raw"
    ffmpeg -hide_banner -loglevel error -y -i "$work/reference-$label.wav" -f f32le -acodec pcm_f32le "$work/reference-$label.raw"

    python3 - "$work/capture-$label.raw" "$work/reference-$label.raw" <<'PY'
import struct
import sys

capture_path, reference_path = sys.argv[1], sys.argv[2]
with open(capture_path, "rb") as f:
    data = f.read()
n = len(data) // 4
ch0 = struct.unpack(f"<{n}f", data)[0::2]

with open(reference_path, "rb") as f:
    reference_frame_count = len(f.read()) // 4 // 2  # stereo float32

start = None
for i in range(len(ch0) - 10):
    window = [round(ch0[i + k]) for k in range(10)]
    if all(window[k + 1] == window[k] + 1 for k in range(9)):
        start = i
        break
if start is None:
    print("FAIL: the ramp never appears as 10 consecutive rising samples in the capture")
    sys.exit(1)

# Frames within the reference's own length can legitimately carry ramp
# values; past that the capture is trailing silence/noise by design
# (capture runs longer than the tone) and isn't scored. The ramp can also
# run dry *before* that point -- the safety offset silences the first
# ~512 frames, and real-time delivery lag can end the last real frame a
# little earlier than the reference's own frame count implies -- so a
# sustained run of exact zeros (a real ramp value is only ever exactly
# zero at frame 0) means the tone has ended, not that data was lost, and
# scoring stops there rather than counting the tail as duplicates. An
# isolated zero mid-ramp, by contrast, is exactly what a repeated frame 0
# would look like, so it's still scored normally.
window_end = min(len(ch0), start + reference_frame_count)
silence_run_to_end_scoring = 200

gaps = duplicates_or_reorders = unexplained = 0
prev = round(ch0[start]) - 1
i = start
while i < window_end:
    v = round(ch0[i])
    if v == 0 and i > start and all(abs(ch0[i + k]) < 1e-6 for k in range(min(silence_run_to_end_scoring, window_end - i))):
        print(f"tone ends at capture frame {i} (reference ran dry, last value {prev}); stopping scoring here")
        break
    if abs(ch0[i] - v) > 1e-2:
        unexplained += 1
        if unexplained <= 5:
            print(f"unexplained (non-integer) sample at capture frame {i}: {ch0[i]}")
        i += 1
        continue
    delta = v - prev
    if delta == 1:
        pass
    elif delta > 1:
        gaps += 1
    else:
        duplicates_or_reorders += 1
        if duplicates_or_reorders <= 5:
            print(f"duplicate or reorder at capture frame {i}: value {v}, expected > {prev}")
    prev = v
    i += 1

print(f"external capture over the ramp's span: {gaps} forward gaps (client-delivery drops, not a "
      f"driver defect), {duplicates_or_reorders} duplicates/reorders, {unexplained} unexplained samples")

max_unexplained = 10
if duplicates_or_reorders > 0:
    print("FAIL: capture contains a duplicated or reordered sample")
    sys.exit(1)
if unexplained > max_unexplained:
    print(f"FAIL: {unexplained} unexplained samples exceeds the allowance of {max_unexplained}")
    sys.exit(1)
print("PASS: no duplicated or reordered sample, and no more than a small handful of unexplained "
      "samples, reached the capture client")
PY
}

run_segment 48000 seg1 || overall_failed=1

echo "changing nominal sample rate to 96000 Hz between segments (no client attached)"
"$work/set_sample_rate" 96000
read -r safety_offset nominal_rate < <("$work/query_device_stats")
if [ "${nominal_rate%.*}" -ne 96000 ]; then
    echo "FAIL: sample-rate change did not take effect (device reports $nominal_rate Hz, expected 96000 Hz)" >&2
    overall_failed=1
fi

run_segment 96000 seg2 || overall_failed=1

if [ "$overall_failed" -ne 0 ]; then
    exit 1
fi
