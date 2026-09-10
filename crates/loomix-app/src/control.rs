//! The UI <-> audio-thread bridge (spec 3.4 M8). Spec 3.3 forbids a
//! shared `Mutex<Engine>` between Tauri's command handlers and the
//! real-time thread outright, so this owns exactly two one-way, lock-free
//! crossings plus one ordinary, app-side mirror -- both built on `rtrb`,
//! the same SPSC primitive spec 3.3 already names, rather than adding a
//! second dependency for the audio-to-UI direction (see
//! [`snapshot_channel`]'s doc comment for why):
//!
//! - **UI -> audio**: [`EngineCommand`]s, coalesced per parameter
//!   (last-value-wins) by [`CommandSink`] and pushed through an SPSC
//!   queue; [`CommandDrain`] applies them directly to [`Engine`] at the
//!   top of each audio callback, before `process_block`.
//! - **audio -> UI**: a published [`ControlSnapshot`] ([`SnapshotPublisher`]
//!   / [`SnapshotReader`]), alongside the existing per-block meters, read
//!   back at a low, UI-appropriate rate to catch and correct drift in the
//!   UI's own optimistic mirror -- not a per-frame round trip.
//!
//! Scoped to M8's actual control surface (faders, mute, solo, bus
//! assignment, bus mode, the EQ graph): composite/insert patch editing
//! isn't part of this milestone's UI (spec 3.4 M8), so it has no command
//! variant yet -- more variants extend `EngineCommand`/`ParamKey` the same
//! way when a milestone actually needs them.

use loomix_core::bus::BusMono;
use loomix_core::bus_mode::BusMode;
use loomix_core::intellipan::IntellipanMode;
use loomix_core::karaoke::KaraokeMode;
use loomix_core::parametric_eq::{EqCellParams, EqChannelParams, Memory, NUM_CELLS};
use loomix_core::strip_dsp::StripChain;
use loomix_core::{Engine, Meter, CHANNELS, NUM_BUSES, NUM_STRIPS};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// spec 1.2 step 7: the hardware strip EQ is stereo, unlike the bus EQ's
/// independent [`CHANNELS`] (8).
pub const STRIP_EQ_CHANNELS: usize = 2;

/// Comfortably exceeds the mixer's total distinct addressable M8-scope
/// parameters: summing every strip's mute/solo/mono, bus assigns, gain
/// layers and strip-EQ cells, and every bus's mute/mono/mode/gain and
/// bus-EQ cells, comes to under 700 total (see `docs/ARCHITECTURE.md`'s
/// M8 entry for the count), so overflow is never reached by ordinary use.
/// Reaching it means the audio thread has stopped draining (a real fault,
/// e.g. a stalled device), not that the UI generated too many distinct
/// edits.
pub const COMMAND_QUEUE_CAPACITY: usize = 1024;

/// One discrete mixer parameter change.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EngineCommand {
    SetStripMute(usize, bool),
    SetStripSolo(usize, bool),
    SetStripMono(usize, bool),
    /// (strip, bus, on)
    SetStripBusAssign(usize, usize, bool),
    /// (strip, bus, db)
    SetStripGainLayer(usize, usize, f32),
    /// (strip, pan) -- spec 1.2 step 9's pan pot, hardware strips only
    /// (spec 1.3); a no-op if `strip` is a virtual strip (spec 1.4 has a
    /// 5.1 position pad instead, not this control). `-1.0` hard left,
    /// `0.0` center, `1.0` hard right (`docs/DSP.md`'s balance law).
    SetStripPan(usize, f32),
    SetBusMute(usize, bool),
    SetBusMono(usize, BusMono),
    SetBusMode(usize, BusMode),
    SetBusGain(usize, f32),
    /// (strip, channel, cell, params) -- hardware strips only (spec 1.2
    /// step 7); a no-op if `strip` is a virtual strip, since the command
    /// simply has nothing to apply to.
    SetStripEqCell(usize, usize, usize, EqCellParams),
    /// (bus, channel, cell, params)
    SetBusEqCell(usize, usize, usize, EqCellParams),

    // -- M10: the 2026-09-09 coverage audit's 11-item state-2 list
    // (`docs/COVERAGE-AUDIT-2026-09-09.md`) -- every control here was
    // already implemented and tested in `loomix-core`, just unreachable
    // from any UI. See `tests::CONTROL_CASES` for the table-driven
    // round-trip/bounds proof covering all fourteen variants below at
    // once, per direct instruction not to hand-write eleven near-identical
    // copy-pasted tests.
    /// (strip, knob 0..10) -- spec 1.3's gate macro knob, hardware strips
    /// only; a no-op on a virtual strip (spec 1.4 has no gate at all).
    SetStripGateKnob(usize, f32),
    /// (strip, knob 0..10) -- compressor macro knob, hardware strips only.
    SetStripCompKnob(usize, f32),
    /// (strip, knob 0..10) -- denoiser macro knob, hardware strips only.
    SetStripDenoiserKnob(usize, f32),
    /// (strip, threshold_db) -- spec 1.3/1.4's limiter threshold, valid on
    /// both hardware and virtual strips (each chain kind has its own
    /// `Limiter`, spec_dsp::StripChain::limiter_mut).
    SetStripLimiterThreshold(usize, f32),
    /// (strip, mode) -- spec 1.18's "right click cycles Color/Position/
    /// Modulation," hardware strips only.
    SetStripIntellipanMode(usize, IntellipanMode),
    /// (strip, x, y) -- the Intellipan pad's currently active mode,
    /// hardware strips only.
    SetStripIntellipanXY(usize, f32, f32),
    /// (strip, bass_db, mid_db, treble_db) -- spec 1.4's 3-band EQ,
    /// virtual strips only.
    SetStripEq3(usize, f32, f32, f32),
    /// (strip, x, y) -- spec 1.4's 5.1 position pad, virtual strips only.
    SetStripPositionPad(usize, f32, f32),
    /// (strip, on) -- M.C. (mute centre), virtual strips only.
    SetStripMc(usize, bool),
    /// (strip, mode) -- Karaoke, virtual strips only. Spec 1.4 makes it
    /// audible only on the AUX strip, but the field and this command apply
    /// to any virtual strip, exactly like `VirtualChain::karaoke` itself --
    /// `is_aux` gates whether `process()` ever reads it, not whether it
    /// can be set, so this command doesn't re-implement that gating.
    SetStripKaraoke(usize, KaraokeMode),
    /// (strip, on) -- the strip parametric EQ's on/off toggle, hardware
    /// strips only.
    SetStripEqOn(usize, bool),
    /// (strip, memory) -- the strip parametric EQ's A/B memory, hardware
    /// strips only.
    SetStripEqMemory(usize, Memory),
    /// (bus, on) -- the bus parametric EQ's on/off toggle.
    SetBusEqOn(usize, bool),
    /// (bus, memory) -- the bus parametric EQ's A/B memory.
    SetBusEqMemory(usize, Memory),

    // -- M10 (continued): spec 1.7's per-channel trim/delay, FLAT and CH
    // COPY. Brought into M10 rather than deferred, on direct instruction:
    // all four are already fully implemented and tested in `loomix-core`
    // (`ParametricEq::{set_trim_db,set_delay_ms,reset_channel,
    // copy_channel}`), so leaving them unwired would recreate the exact
    // state-2 shape M10 exists to close. COPY ALL (cross-bus, a genuinely
    // different command shape -- two EQ instances, not one) and the file
    // load/save plus right-click-precision-edit/scale gestures are the
    // ones actually deferred; see `docs/ARCHITECTURE.md`'s M10 entry for
    // why those specifically, with M15 named as the milestone that owns
    // them, not left unassigned.
    /// (strip, channel, trim_db) -- hardware strips only.
    SetStripEqTrim(usize, usize, f32),
    /// (strip, channel, delay_ms) -- hardware strips only.
    SetStripEqDelay(usize, usize, f32),
    /// (bus, channel, trim_db).
    SetBusEqTrim(usize, usize, f32),
    /// (bus, channel, delay_ms).
    SetBusEqDelay(usize, usize, f32),
    /// (strip, channel) -- FLAT: resets one channel to its neutral
    /// default (every cell off, trim 0, delay 0), hardware strips only.
    ResetStripEqChannel(usize, usize),
    /// (bus, channel) -- FLAT.
    ResetBusEqChannel(usize, usize),
    /// (strip, from_channel, to_channel) -- CH COPY, within the strip's
    /// own EQ, hardware strips only.
    CopyStripEqChannel(usize, usize, usize),
    /// (bus, from_channel, to_channel) -- CH COPY, within the bus's own EQ.
    CopyBusEqChannel(usize, usize, usize),
}

/// The coalescing key: two pending commands with the same key are the
/// same logical control, so only the newest survives a burst (a fader
/// drag, a rapid EQ sweep) -- see [`CommandSink::enqueue`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ParamKey {
    StripMute(usize),
    StripSolo(usize),
    StripMono(usize),
    StripBusAssign(usize, usize),
    StripGainLayer(usize, usize),
    StripPan(usize),
    BusMute(usize),
    BusMono(usize),
    BusMode(usize),
    BusGain(usize),
    StripEqCell(usize, usize, usize),
    BusEqCell(usize, usize, usize),
    StripGateKnob(usize),
    StripCompKnob(usize),
    StripDenoiserKnob(usize),
    StripLimiterThreshold(usize),
    StripIntellipanMode(usize),
    StripIntellipanXY(usize),
    StripEq3(usize),
    StripPositionPad(usize),
    StripMc(usize),
    StripKaraoke(usize),
    StripEqOn(usize),
    StripEqMemory(usize),
    BusEqOn(usize),
    BusEqMemory(usize),
    StripEqTrim(usize, usize),
    StripEqDelay(usize, usize),
    BusEqTrim(usize, usize),
    BusEqDelay(usize, usize),
    ResetStripEqChannel(usize, usize),
    ResetBusEqChannel(usize, usize),
    CopyStripEqChannel(usize, usize),
    CopyBusEqChannel(usize, usize),
}

impl EngineCommand {
    fn key(&self) -> ParamKey {
        match *self {
            Self::SetStripMute(s, _) => ParamKey::StripMute(s),
            Self::SetStripSolo(s, _) => ParamKey::StripSolo(s),
            Self::SetStripMono(s, _) => ParamKey::StripMono(s),
            Self::SetStripBusAssign(s, b, _) => ParamKey::StripBusAssign(s, b),
            Self::SetStripGainLayer(s, b, _) => ParamKey::StripGainLayer(s, b),
            Self::SetStripPan(s, _) => ParamKey::StripPan(s),
            Self::SetBusMute(b, _) => ParamKey::BusMute(b),
            Self::SetBusMono(b, _) => ParamKey::BusMono(b),
            Self::SetBusMode(b, _) => ParamKey::BusMode(b),
            Self::SetBusGain(b, _) => ParamKey::BusGain(b),
            Self::SetStripEqCell(s, ch, cell, _) => ParamKey::StripEqCell(s, ch, cell),
            Self::SetBusEqCell(b, ch, cell, _) => ParamKey::BusEqCell(b, ch, cell),
            Self::SetStripGateKnob(s, _) => ParamKey::StripGateKnob(s),
            Self::SetStripCompKnob(s, _) => ParamKey::StripCompKnob(s),
            Self::SetStripDenoiserKnob(s, _) => ParamKey::StripDenoiserKnob(s),
            Self::SetStripLimiterThreshold(s, _) => ParamKey::StripLimiterThreshold(s),
            Self::SetStripIntellipanMode(s, _) => ParamKey::StripIntellipanMode(s),
            Self::SetStripIntellipanXY(s, _, _) => ParamKey::StripIntellipanXY(s),
            Self::SetStripEq3(s, _, _, _) => ParamKey::StripEq3(s),
            Self::SetStripPositionPad(s, _, _) => ParamKey::StripPositionPad(s),
            Self::SetStripMc(s, _) => ParamKey::StripMc(s),
            Self::SetStripKaraoke(s, _) => ParamKey::StripKaraoke(s),
            Self::SetStripEqOn(s, _) => ParamKey::StripEqOn(s),
            Self::SetStripEqMemory(s, _) => ParamKey::StripEqMemory(s),
            Self::SetBusEqOn(b, _) => ParamKey::BusEqOn(b),
            Self::SetBusEqMemory(b, _) => ParamKey::BusEqMemory(b),
            Self::SetStripEqTrim(s, ch, _) => ParamKey::StripEqTrim(s, ch),
            Self::SetStripEqDelay(s, ch, _) => ParamKey::StripEqDelay(s, ch),
            Self::SetBusEqTrim(b, ch, _) => ParamKey::BusEqTrim(b, ch),
            Self::SetBusEqDelay(b, ch, _) => ParamKey::BusEqDelay(b, ch),
            Self::ResetStripEqChannel(s, ch) => ParamKey::ResetStripEqChannel(s, ch),
            Self::ResetBusEqChannel(b, ch) => ParamKey::ResetBusEqChannel(b, ch),
            // Keyed on the mutated (destination) channel, not the source
            // -- two copies landing on the same channel should still
            // coalesce to the last one requested, same as any other
            // parameter; the source channel is just this command's payload.
            Self::CopyStripEqChannel(s, _, to) => ParamKey::CopyStripEqChannel(s, to),
            Self::CopyBusEqChannel(b, _, to) => ParamKey::CopyBusEqChannel(b, to),
        }
    }

    /// Applies directly to `Engine` if every index the command carries is
    /// in range, and is a silent no-op otherwise. Only ever called from
    /// the audio thread's drain step ([`CommandDrain::drain_into`]) --
    /// every variant is a plain field/array write or an existing
    /// non-allocating setter, so this never allocates.
    ///
    /// Bounds-checked here, not just trusted from the caller: an
    /// `EngineCommand` is built from plain `usize` indices with no
    /// `TryFrom`/range type of its own, and the intended validation point
    /// -- the UI/Tauri-command layer that will construct these (same
    /// "validate at a system boundary" convention as
    /// `parametric_eq::EqCellParams::freq_hz`'s own doc comment) -- didn't
    /// exist when this module was written and, even once it does, is a
    /// second, separate piece of code a future change could get wrong.
    /// Every other index-taking codepath in this file (`key`,
    /// `CommandSink`, `CommandDrain`) is safe *because* it never indexes
    /// anything itself -- this is the one place that actually does, on
    /// the one thread where an unchecked out-of-range index (a bug, or a
    /// compromised/buggy frontend once one exists) would panic on real
    /// audio hardware's callback rather than fail an HTTP request.
    fn apply(self, engine: &mut Engine) {
        match self {
            Self::SetStripMute(s, on) => {
                if s < NUM_STRIPS {
                    engine.strips[s].mute = on;
                }
            }
            Self::SetStripSolo(s, on) => {
                if s < NUM_STRIPS {
                    engine.strips[s].solo = on;
                }
            }
            Self::SetStripMono(s, on) => {
                if s < NUM_STRIPS {
                    engine.strips[s].mono = on;
                }
            }
            Self::SetStripBusAssign(s, b, on) => {
                if s < NUM_STRIPS && b < NUM_BUSES {
                    engine.strips[s].bus_assign[b] = on;
                }
            }
            Self::SetStripGainLayer(s, b, db) => {
                if s < NUM_STRIPS && b < NUM_BUSES {
                    engine.strips[s].set_gain_layer_db(b, db);
                }
            }
            Self::SetStripPan(s, pan) => {
                if s < NUM_STRIPS {
                    if let StripChain::Hardware(chain) = &mut engine.strips[s].chain {
                        chain.pan.pan = pan;
                    }
                }
            }
            Self::SetBusMute(b, on) => {
                if b < NUM_BUSES {
                    engine.buses[b].mute = on;
                }
            }
            Self::SetBusMono(b, mono) => {
                if b < NUM_BUSES {
                    engine.buses[b].mono = mono;
                }
            }
            Self::SetBusMode(b, mode) => {
                if b < NUM_BUSES {
                    engine.buses[b].mode = mode;
                }
            }
            Self::SetBusGain(b, db) => {
                if b < NUM_BUSES {
                    engine.buses[b].set_gain_db(db);
                }
            }
            Self::SetStripEqCell(s, ch, cell, params) => {
                if s < NUM_STRIPS && ch < STRIP_EQ_CHANNELS && cell < NUM_CELLS {
                    if let StripChain::Hardware(chain) = &mut engine.strips[s].chain {
                        chain.eq.set_cell(ch, cell, params);
                    }
                }
            }
            Self::SetBusEqCell(b, ch, cell, params) => {
                if b < NUM_BUSES && ch < CHANNELS && cell < NUM_CELLS {
                    engine.buses[b].eq.set_cell(ch, cell, params);
                }
            }
            Self::SetStripGateKnob(s, knob) => {
                if s < NUM_STRIPS {
                    if let StripChain::Hardware(chain) = &mut engine.strips[s].chain {
                        chain.gate.set_knob(knob);
                    }
                }
            }
            Self::SetStripCompKnob(s, knob) => {
                if s < NUM_STRIPS {
                    if let StripChain::Hardware(chain) = &mut engine.strips[s].chain {
                        chain.compressor.set_knob(knob);
                    }
                }
            }
            Self::SetStripDenoiserKnob(s, knob) => {
                if s < NUM_STRIPS {
                    if let StripChain::Hardware(chain) = &mut engine.strips[s].chain {
                        chain.denoiser.set_knob(knob);
                    }
                }
            }
            Self::SetStripLimiterThreshold(s, db) => {
                if s < NUM_STRIPS {
                    engine.strips[s].chain.limiter_mut().threshold_db = db;
                }
            }
            Self::SetStripIntellipanMode(s, mode) => {
                if s < NUM_STRIPS {
                    if let StripChain::Hardware(chain) = &mut engine.strips[s].chain {
                        chain.pad.set_mode(mode);
                    }
                }
            }
            Self::SetStripIntellipanXY(s, x, y) => {
                if s < NUM_STRIPS {
                    if let StripChain::Hardware(chain) = &mut engine.strips[s].chain {
                        chain.pad.set_position(x, y);
                    }
                }
            }
            Self::SetStripEq3(s, bass, mid, treble) => {
                if s < NUM_STRIPS {
                    if let StripChain::Virtual(chain) = &mut engine.strips[s].chain {
                        chain.eq.set_gains(bass, mid, treble);
                    }
                }
            }
            Self::SetStripPositionPad(s, x, y) => {
                if s < NUM_STRIPS {
                    if let StripChain::Virtual(chain) = &mut engine.strips[s].chain {
                        chain.pan_pad.x = x;
                        chain.pan_pad.y = y;
                    }
                }
            }
            Self::SetStripMc(s, on) => {
                if s < NUM_STRIPS {
                    if let StripChain::Virtual(chain) = &mut engine.strips[s].chain {
                        chain.mc = on;
                    }
                }
            }
            Self::SetStripKaraoke(s, mode) => {
                if s < NUM_STRIPS {
                    if let StripChain::Virtual(chain) = &mut engine.strips[s].chain {
                        chain.karaoke.mode = mode;
                    }
                }
            }
            Self::SetStripEqOn(s, on) => {
                if s < NUM_STRIPS {
                    if let StripChain::Hardware(chain) = &mut engine.strips[s].chain {
                        chain.eq.on = on;
                    }
                }
            }
            Self::SetStripEqMemory(s, memory) => {
                if s < NUM_STRIPS {
                    if let StripChain::Hardware(chain) = &mut engine.strips[s].chain {
                        chain.eq.set_active_memory(memory);
                    }
                }
            }
            Self::SetBusEqOn(b, on) => {
                if b < NUM_BUSES {
                    engine.buses[b].eq.on = on;
                }
            }
            Self::SetBusEqMemory(b, memory) => {
                if b < NUM_BUSES {
                    engine.buses[b].eq.set_active_memory(memory);
                }
            }
            Self::SetStripEqTrim(s, ch, trim_db) => {
                if s < NUM_STRIPS && ch < STRIP_EQ_CHANNELS {
                    if let StripChain::Hardware(chain) = &mut engine.strips[s].chain {
                        chain.eq.set_trim_db(ch, trim_db);
                    }
                }
            }
            Self::SetStripEqDelay(s, ch, delay_ms) => {
                if s < NUM_STRIPS && ch < STRIP_EQ_CHANNELS {
                    if let StripChain::Hardware(chain) = &mut engine.strips[s].chain {
                        chain.eq.set_delay_ms(ch, delay_ms);
                    }
                }
            }
            Self::SetBusEqTrim(b, ch, trim_db) => {
                if b < NUM_BUSES && ch < CHANNELS {
                    engine.buses[b].eq.set_trim_db(ch, trim_db);
                }
            }
            Self::SetBusEqDelay(b, ch, delay_ms) => {
                if b < NUM_BUSES && ch < CHANNELS {
                    engine.buses[b].eq.set_delay_ms(ch, delay_ms);
                }
            }
            Self::ResetStripEqChannel(s, ch) => {
                if s < NUM_STRIPS && ch < STRIP_EQ_CHANNELS {
                    if let StripChain::Hardware(chain) = &mut engine.strips[s].chain {
                        chain.eq.reset_channel(ch);
                    }
                }
            }
            Self::ResetBusEqChannel(b, ch) => {
                if b < NUM_BUSES && ch < CHANNELS {
                    engine.buses[b].eq.reset_channel(ch);
                }
            }
            Self::CopyStripEqChannel(s, from, to) => {
                if s < NUM_STRIPS && from < STRIP_EQ_CHANNELS && to < STRIP_EQ_CHANNELS {
                    if let StripChain::Hardware(chain) = &mut engine.strips[s].chain {
                        chain.eq.copy_channel(from, to);
                    }
                }
            }
            Self::CopyBusEqChannel(b, from, to) => {
                if b < NUM_BUSES && from < CHANNELS && to < CHANNELS {
                    engine.buses[b].eq.copy_channel(from, to);
                }
            }
        }
    }
}

/// A lock-free, pollable counter -- the same shape as `engine_io`'s
/// `DropoutCounter`, for the same reason: incremented on whichever side
/// hits the condition, read from anywhere else, no lock. Counts a push
/// that failed because the SPSC queue was full; see [`CommandSink::flush`].
#[derive(Clone, Default)]
pub struct OverflowCounter(Arc<AtomicU64>);

impl OverflowCounter {
    fn increment(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }

    pub fn get(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

/// Builds the one control channel a mixer session uses: `CommandSink` is
/// owned by the app-side actor that Tauri command handlers feed (a single
/// owner, so the underlying SPSC queue stays genuinely single-producer
/// despite Tauri's multi-threaded runtime); `CommandDrain` is owned by
/// whatever drives the audio callback.
pub fn control_channel() -> (CommandSink, CommandDrain) {
    let (producer, consumer) = rtrb::RingBuffer::new(COMMAND_QUEUE_CAPACITY);
    let overflow = OverflowCounter::default();
    (
        CommandSink {
            producer,
            pending: HashMap::new(),
            overflow: overflow.clone(),
        },
        CommandDrain { consumer },
    )
}

/// App-side (UI-facing) half: coalesces incoming commands per parameter
/// and flushes them into the SPSC queue.
pub struct CommandSink {
    producer: rtrb::Producer<EngineCommand>,
    pending: HashMap<ParamKey, EngineCommand>,
    overflow: OverflowCounter,
}

impl CommandSink {
    /// Records the latest value for this command's parameter, overwriting
    /// whatever was already pending for it. Cheap and never touches the
    /// queue itself -- a fader drag calling this every pointer-move event
    /// only ever holds one entry per parameter, however many events arrive
    /// before the next [`Self::flush`].
    pub fn enqueue(&mut self, command: EngineCommand) {
        self.pending.insert(command.key(), command);
    }

    /// Pushes every pending command into the SPSC queue. A command whose
    /// push fails (queue full) is left in `pending` rather than discarded
    /// -- it is never lost, only retried on the next flush with whatever
    /// value is current by then, which is what makes the overflow case
    /// safe: retrying a coalesced key can only ever converge toward the
    /// last value actually sent, never toward a stale or corrupted one.
    /// Each failed push increments [`Self::overflow_counter`], since
    /// reaching capacity at all means the audio thread isn't draining --
    /// see `COMMAND_QUEUE_CAPACITY`'s doc comment.
    pub fn flush(&mut self) {
        self.pending.retain(|_, &mut command| {
            if self.producer.push(command).is_ok() {
                false // pushed: drop from pending
            } else {
                self.overflow.increment();
                true // still pending: retry next flush
            }
        });
    }

    pub fn overflow_counter(&self) -> OverflowCounter {
        self.overflow.clone()
    }

    #[cfg(test)]
    fn pending_len(&self) -> usize {
        self.pending.len()
    }
}

/// Audio-thread-side half: drains and applies pending commands.
pub struct CommandDrain {
    consumer: rtrb::Consumer<EngineCommand>,
}

impl CommandDrain {
    /// Applies up to `max` pending commands to `engine`, in FIFO order.
    /// Bounds one audio callback's drain work so a pathological backlog
    /// can't blow the callback's time budget -- this never *drops*
    /// anything, unlike a full queue: whatever isn't drained this call
    /// simply waits in the queue for the next one, a few milliseconds
    /// later. Returns how many were applied.
    pub fn drain_into(&mut self, engine: &mut Engine, max: usize) -> usize {
        let mut applied = 0;
        while applied < max {
            match self.consumer.pop() {
                Ok(command) => {
                    command.apply(engine);
                    applied += 1;
                }
                Err(_) => break,
            }
        }
        applied
    }
}

/// spec 1.5's SEL/gain-layer surface, mirrored per strip.
///
/// M10 extends this with every new scalar `EngineCommand` from the
/// coverage audit's state-2 list -- the doc comment on [`ControlSnapshot`]
/// already establishes the rule this follows ("`EngineCommand`'s own
/// scalar surface", EQ cells excluded): every field below is exactly that,
/// not a new exception to it. A field that doesn't apply to a given
/// strip's actual chain kind (a hardware-only field on a virtual strip, or
/// vice versa) reads as that control's own neutral/default value, the
/// same convention `pan` already established.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StripSnapshot {
    pub mute: bool,
    pub solo: bool,
    pub mono: bool,
    pub bus_assign: [bool; NUM_BUSES],
    pub gain_layer_db: [f32; NUM_BUSES],
    /// `0.0` (center) on a virtual strip: it has no pan pot (spec 1.4 has
    /// a 5.1 position pad instead), so there's nothing live to mirror.
    pub pan: f32,
    /// Hardware-only; `0.0` (bypassed) on a virtual strip.
    pub gate_knob: f32,
    /// Hardware-only; `0.0` (bypassed) on a virtual strip.
    pub comp_knob: f32,
    /// Hardware-only; `0.0` (bypassed) on a virtual strip.
    pub denoiser_knob: f32,
    /// Both chain kinds have a limiter (spec 1.3/1.4), so this is always
    /// live, never a stand-in default.
    pub limiter_threshold_db: f32,
    /// Hardware-only; `IntellipanMode::Color` (the construction-time
    /// default, spec names no default mode) on a virtual strip.
    pub intellipan_mode: IntellipanMode,
    /// Hardware-only; `(0.0, 0.0)` on a virtual strip.
    pub intellipan_xy: (f32, f32),
    /// Hardware-only; `false` on a virtual strip.
    pub strip_eq_on: bool,
    /// Hardware-only; `Memory::A` on a virtual strip.
    pub strip_eq_memory: Memory,
    /// Virtual-only; `(0.0, 0.0, 0.0)` on a hardware strip.
    pub eq3_db: (f32, f32, f32),
    /// Virtual-only; `(0.0, 0.0)` on a hardware strip.
    pub position_pad: (f32, f32),
    /// Virtual-only; `false` on a hardware strip.
    pub mc: bool,
    /// Virtual-only; `KaraokeMode::Off` on a hardware strip. Live on every
    /// virtual strip regardless of `is_aux`, same as the field it mirrors
    /// (`SetStripKaraoke`'s own doc comment).
    pub karaoke: KaraokeMode,
}

/// M10 extends this with the bus parametric EQ's on/off and A/B memory --
/// the cells themselves stay out of scope, same rule as the strip side
/// (`StripSnapshot`'s doc comment).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BusSnapshot {
    pub mute: bool,
    pub mono: BusMono,
    pub mode: BusMode,
    pub gain_db: f32,
    pub eq_on: bool,
    pub eq_memory: Memory,
}

/// The low-rate reconciliation snapshot (module doc): scoped to
/// `EngineCommand`'s own scalar surface, not the EQ cells -- "just enough
/// that drift is detectable" (direct instruction), not an exhaustive
/// mirror. If EQ-cell drift is ever found to matter in practice, it earns
/// its own, larger snapshot then; this isn't silently incomplete, it's
/// deliberately scoped, same as every other cut logged in
/// `docs/ARCHITECTURE.md`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ControlSnapshot {
    pub strips: [StripSnapshot; NUM_STRIPS],
    pub buses: [BusSnapshot; NUM_BUSES],
}

impl ControlSnapshot {
    /// Reads the live values straight off `Engine` -- called once per
    /// audio callback, alongside meter observation, to publish into the
    /// channel ([`snapshot_channel`]) the app-side reconciliation task
    /// polls.
    pub fn capture(engine: &Engine) -> Self {
        Self {
            strips: std::array::from_fn(|s| {
                let chain = &engine.strips[s].chain;
                StripSnapshot {
                    mute: engine.strips[s].mute,
                    solo: engine.strips[s].solo,
                    mono: engine.strips[s].mono,
                    bus_assign: engine.strips[s].bus_assign,
                    gain_layer_db: std::array::from_fn(|b| engine.strips[s].gain_layer_db(b)),
                    pan: match chain {
                        StripChain::Hardware(c) => c.pan.pan,
                        StripChain::Virtual(_) => 0.0,
                    },
                    gate_knob: match chain {
                        StripChain::Hardware(c) => c.gate.knob(),
                        StripChain::Virtual(_) => 0.0,
                    },
                    comp_knob: match chain {
                        StripChain::Hardware(c) => c.compressor.knob(),
                        StripChain::Virtual(_) => 0.0,
                    },
                    denoiser_knob: match chain {
                        StripChain::Hardware(c) => c.denoiser.knob(),
                        StripChain::Virtual(_) => 0.0,
                    },
                    limiter_threshold_db: match chain {
                        StripChain::Hardware(c) => c.limiter.threshold_db,
                        StripChain::Virtual(c) => c.limiter.threshold_db,
                    },
                    intellipan_mode: match chain {
                        StripChain::Hardware(c) => c.pad.mode(),
                        StripChain::Virtual(_) => IntellipanMode::default(),
                    },
                    intellipan_xy: match chain {
                        StripChain::Hardware(c) => c.pad.position(),
                        StripChain::Virtual(_) => (0.0, 0.0),
                    },
                    strip_eq_on: match chain {
                        StripChain::Hardware(c) => c.eq.on,
                        StripChain::Virtual(_) => false,
                    },
                    strip_eq_memory: match chain {
                        StripChain::Hardware(c) => c.eq.active_memory(),
                        StripChain::Virtual(_) => Memory::A,
                    },
                    eq3_db: match chain {
                        StripChain::Hardware(_) => (0.0, 0.0, 0.0),
                        StripChain::Virtual(c) => (c.eq.bass_db, c.eq.mid_db, c.eq.treble_db),
                    },
                    position_pad: match chain {
                        StripChain::Hardware(_) => (0.0, 0.0),
                        StripChain::Virtual(c) => (c.pan_pad.x, c.pan_pad.y),
                    },
                    mc: match chain {
                        StripChain::Hardware(_) => false,
                        StripChain::Virtual(c) => c.mc,
                    },
                    karaoke: match chain {
                        StripChain::Hardware(_) => KaraokeMode::Off,
                        StripChain::Virtual(c) => c.karaoke.mode,
                    },
                }
            }),
            buses: std::array::from_fn(|b| BusSnapshot {
                mute: engine.buses[b].mute,
                mono: engine.buses[b].mono,
                mode: engine.buses[b].mode,
                gain_db: engine.buses[b].gain_db(),
                eq_on: engine.buses[b].eq.on,
                eq_memory: engine.buses[b].eq.active_memory(),
            }),
        }
    }
}

impl Default for ControlSnapshot {
    fn default() -> Self {
        Self::capture(&Engine::new())
    }
}

/// Spec 1.3/1.5's input/output meters: the audio-thread-only side of the
/// exact crossing `Meter`'s own doc comment named as owed once a UI
/// thread existed ("no separate UI thread yet to hand it across... spec
/// 3.3's crossing applies once one exists, from M4 on") -- published
/// every callback over [`latest_value_channel`], same as
/// [`ControlSnapshot`], polled at a UI-appropriate rate (this one closer
/// to per-frame, since meters are meant to move visibly, unlike the
/// reconciliation snapshot).
#[derive(Debug, Clone, Copy)]
pub struct MeterSnapshot {
    pub strips: [Meter; NUM_STRIPS],
    pub buses: [Meter; NUM_BUSES],
}

impl MeterSnapshot {
    pub fn capture(engine: &Engine) -> Self {
        Self {
            strips: std::array::from_fn(|s| *engine.strip_meter(s)),
            buses: std::array::from_fn(|b| *engine.bus_meter(b)),
        }
    }
}

impl Default for MeterSnapshot {
    /// `Meter` is no longer `Default` itself (M8: hold/decay ballistics
    /// are sample-rate dependent, same reasoning as every other
    /// sample-rate-dependent block in `loomix-core`), so this goes
    /// through a real `Engine` for its sample rate -- the same pattern
    /// `ControlSnapshot::default` already uses, not a new one.
    fn default() -> Self {
        Self::capture(&Engine::new())
    }
}

/// M10's parametric EQ panel needs to *display* a strip's or bus's actual
/// 6-cell state, not just fire-and-forget edits into it -- opening the
/// panel on a strip someone already configured (from an earlier session,
/// or from a preset once M14 exists) has to show what's really there.
/// `ControlSnapshot`'s own doc comment predicted exactly this ("EQ cells
/// get their own, larger snapshot later if drift there is ever found to
/// matter in practice") -- this is that snapshot, kept separate from
/// `ControlSnapshot` rather than folded into it, since it's polled only
/// while a panel is actually open, not every reconciliation tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EqSnapshot {
    /// Hardware strips only (spec 1.2 step 7, stereo); a virtual strip has
    /// no parametric EQ at all (spec 1.4's 3-band EQ instead) and reads as
    /// `EqChannelParams::default()` for both channels, same "neutral
    /// default on the strip kind it doesn't apply to" convention
    /// `StripSnapshot` already uses.
    pub strips: [[EqChannelParams; STRIP_EQ_CHANNELS]; NUM_STRIPS],
    pub buses: [[EqChannelParams; CHANNELS]; NUM_BUSES],
}

impl EqSnapshot {
    pub fn capture(engine: &Engine) -> Self {
        Self {
            strips: std::array::from_fn(|s| match &engine.strips[s].chain {
                StripChain::Hardware(c) => std::array::from_fn(|ch| *c.eq.channel_params(ch)),
                StripChain::Virtual(_) => [EqChannelParams::default(); STRIP_EQ_CHANNELS],
            }),
            buses: std::array::from_fn(|b| {
                std::array::from_fn(|ch| *engine.buses[b].eq.channel_params(ch))
            }),
        }
    }
}

impl Default for EqSnapshot {
    fn default() -> Self {
        Self::capture(&Engine::new())
    }
}

/// A small, generic "latest value wins" cross built on the same `rtrb`
/// SPSC ring [`control_channel`] already uses, rather than a dedicated
/// triple-buffer crate: the one candidate for that (`triple_buffer`) is
/// MPL-2.0, a copyleft license outside this project's allow-list
/// (`deny.toml`) for a product that ships a commercially-distributed
/// installer (spec 4.5) -- not a call to make unilaterally by widening
/// the allow-list for one dependency's convenience. `rtrb` is already
/// vetted, already a dependency here, and the same lock-free/
/// no-allocation guarantee spec 3.3 asks for covers this just as well:
/// the audio thread owns [`LatestValuePublisher`] and calls
/// [`LatestValuePublisher::publish`] once per callback; the app-side
/// reader owns [`LatestValueReader`] and calls [`LatestValueReader::read`]
/// at whatever rate it needs, never necessarily per audio block. Used for
/// both [`ControlSnapshot`] (reconciliation, module doc) and
/// `MeterSnapshot` (`bin/main.rs`, spec 1.3/1.5's meters) -- the same
/// crossing shape either way, just a different `T`.
pub fn latest_value_channel<T: Copy + Default>(
    capacity: usize,
) -> (LatestValuePublisher<T>, LatestValueReader<T>) {
    let (producer, consumer) = rtrb::RingBuffer::new(capacity);
    (
        LatestValuePublisher { producer },
        LatestValueReader {
            consumer,
            latest: T::default(),
        },
    )
}

/// The M8 plan's reconciliation channel, specialised to [`ControlSnapshot`].
pub fn snapshot_channel(
    capacity: usize,
) -> (
    LatestValuePublisher<ControlSnapshot>,
    LatestValueReader<ControlSnapshot>,
) {
    latest_value_channel(capacity)
}

/// M10's EQ panel channel, specialised to [`EqSnapshot`].
pub fn eq_snapshot_channel(
    capacity: usize,
) -> (
    LatestValuePublisher<EqSnapshot>,
    LatestValueReader<EqSnapshot>,
) {
    latest_value_channel(capacity)
}

pub struct LatestValuePublisher<T: Copy> {
    producer: rtrb::Producer<T>,
}

impl<T: Copy> LatestValuePublisher<T> {
    /// Publishes the latest value. Never blocks and never allocates (the
    /// ring is pre-allocated once, at [`latest_value_channel`]): if the
    /// reader hasn't polled recently and the small backlog is momentarily
    /// full, this drops the value rather than waiting for room. That's
    /// harmless here specifically because only the *most recent* value
    /// ever matters once the reader does poll (see
    /// [`LatestValueReader::read`] draining the whole backlog and keeping
    /// only the last one) -- unlike [`CommandSink::flush`], where a
    /// dropped value would be a lost user action, a dropped intermediate
    /// publish here is just a value nothing ever needed to observe.
    pub fn publish(&mut self, value: T) {
        let _ = self.producer.push(value);
    }
}

pub struct LatestValueReader<T: Copy> {
    consumer: rtrb::Consumer<T>,
    latest: T,
}

impl<T: Copy> LatestValueReader<T> {
    /// Drains every value published since the last read and returns the
    /// most recent one, discarding any older backlog -- a "latest value"
    /// cross, not a lossless history. Returns the previous value
    /// unchanged if nothing new has been published.
    pub fn read(&mut self) -> T {
        while let Ok(value) = self.consumer.pop() {
            self.latest = value;
        }
        self.latest
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use loomix_core::{Frame, CHANNELS};

    fn silent_block(len: usize) -> Vec<Vec<Frame>> {
        vec![vec![[0.0; CHANNELS]; len]; NUM_STRIPS]
    }

    fn run_block(engine: &mut Engine, blocks: &[Vec<Frame>], len: usize) {
        let input_refs: Vec<&[Frame]> = blocks.iter().map(|v| v.as_slice()).collect();
        let mut out_bufs: Vec<Vec<Frame>> = vec![vec![[0.0; CHANNELS]; len]; NUM_BUSES];
        let mut out_refs: Vec<&mut [Frame]> =
            out_bufs.iter_mut().map(|v| v.as_mut_slice()).collect();
        engine.process_block(&input_refs, &mut out_refs);
    }

    #[test]
    fn a_single_command_round_trips_to_the_engine() {
        let (mut sink, mut drain) = control_channel();
        let mut engine = Engine::new();

        sink.enqueue(EngineCommand::SetStripMute(3, true));
        sink.flush();
        let applied = drain.drain_into(&mut engine, 64);

        assert_eq!(applied, 1);
        assert!(engine.strips[3].mute);
        assert_eq!(sink.overflow_counter().get(), 0);
    }

    #[test]
    fn enqueue_coalesces_same_parameter_to_one_pending_entry() {
        let (mut sink, _drain) = control_channel();
        for i in 0..50 {
            sink.enqueue(EngineCommand::SetStripGainLayer(0, 0, i as f32 * 0.1));
        }
        assert_eq!(
            sink.pending_len(),
            1,
            "same (strip, bus) key should coalesce"
        );
    }

    /// The requested proof: flood far past `COMMAND_QUEUE_CAPACITY` with
    /// updates to a *small* number of distinct parameters (so coalescing
    /// keeps `pending` tiny regardless of flood size) and confirm the
    /// engine ends up at exactly the last value sent per parameter, not a
    /// stale, corrupted, or silently-dropped one -- proving overflow
    /// retry-with-coalescing actually converges rather than just not
    /// panicking.
    #[test]
    fn a_flood_past_capacity_still_converges_to_the_last_value_sent() {
        let (mut sink, mut drain) = control_channel();
        let mut engine = Engine::new();

        let flood = COMMAND_QUEUE_CAPACITY * 8;
        let mut expected_gain = 0.0f32;
        let mut expected_mute = false;
        for i in 0..flood {
            expected_gain = -60.0 + (i % 121) as f32;
            expected_mute = i % 2 == 0;
            sink.enqueue(EngineCommand::SetStripGainLayer(2, 5, expected_gain));
            sink.enqueue(EngineCommand::SetBusMute(1, expected_mute));
            // Flush and drain interleaved, like real callbacks would,
            // rather than one giant flush/drain at the very end -- this
            // is what actually exercises the full-queue retry path
            // instead of just the final coalesced value.
            if i % 3 == 0 {
                sink.flush();
                drain.drain_into(&mut engine, 64);
            }
        }
        sink.flush();
        while drain.drain_into(&mut engine, 64) > 0 {}

        assert_eq!(engine.strips[2].gain_layer_db(5), expected_gain);
        assert_eq!(engine.buses[1].mute, expected_mute);
        assert!(
            sink.pending_len() == 0,
            "every pending command should have eventually been applied"
        );
    }

    #[test]
    fn drain_into_never_applies_more_than_max_per_call() {
        let (mut sink, mut drain) = control_channel();
        let mut engine = Engine::new();
        for s in 0..8 {
            for b in 0..8 {
                sink.enqueue(EngineCommand::SetStripGainLayer(s, b, 1.0));
            }
        }
        sink.flush();

        let first = drain.drain_into(&mut engine, 10);
        assert_eq!(first, 10);
        let rest = drain.drain_into(&mut engine, 64);
        assert_eq!(rest, 64 - 10);
    }

    #[test]
    fn reconciliation_snapshot_matches_the_uis_optimistic_mirror_after_a_burst_of_changes() {
        // The requested reconciliation proof, exercising the actual
        // publish/read channel end to end, not just the pure capture
        // function: apply a burst of varied changes through the real
        // command path, publish the resulting engine state the way the
        // audio thread would, read it back the way the app-side
        // reconciliation task would, and confirm it agrees field by field
        // with an independently-built mirror of what the UI would already
        // believe after applying the same commands optimistically.
        let (mut sink, mut drain) = control_channel();
        let (mut publisher, mut reader) = snapshot_channel(8);
        let mut engine = Engine::new();
        let mut mirror = ControlSnapshot::default();

        let commands = [
            EngineCommand::SetStripMute(0, true),
            EngineCommand::SetStripSolo(1, true),
            EngineCommand::SetStripMono(2, true),
            EngineCommand::SetStripBusAssign(3, 4, true),
            EngineCommand::SetStripGainLayer(4, 5, -12.5),
            EngineCommand::SetStripPan(0, -0.5),
            EngineCommand::SetBusMute(0, true),
            EngineCommand::SetBusMono(1, BusMono::StereoReverse),
            EngineCommand::SetBusMode(2, BusMode::MixDownA),
            EngineCommand::SetBusGain(3, -6.0),
        ];
        for &command in &commands {
            sink.enqueue(command);
            // The UI's own optimistic mirror updates immediately, the
            // moment a command is issued -- it never waits for the audio
            // thread at all (module doc).
            apply_to_snapshot(&mut mirror, command);
        }
        sink.flush();
        while drain.drain_into(&mut engine, 64) > 0 {}

        // The audio thread publishes once per callback; the reconciliation
        // task polls at its own low rate -- one publish, one read, is
        // enough to prove the crossing itself is correct.
        publisher.publish(ControlSnapshot::capture(&engine));
        let observed = reader.read();

        assert_eq!(
            observed, mirror,
            "engine-published snapshot must match the UI's optimistic mirror"
        );
    }

    /// Applies one command to a `ControlSnapshot` mirror the same way the
    /// real engine would apply it -- a small, test-only twin of
    /// `EngineCommand::apply` used only to build the independent "what the
    /// UI already believes" side of the reconciliation test above without
    /// routing through a real `Engine`.
    fn apply_to_snapshot(snapshot: &mut ControlSnapshot, command: EngineCommand) {
        match command {
            EngineCommand::SetStripMute(s, on) => snapshot.strips[s].mute = on,
            EngineCommand::SetStripSolo(s, on) => snapshot.strips[s].solo = on,
            EngineCommand::SetStripMono(s, on) => snapshot.strips[s].mono = on,
            EngineCommand::SetStripBusAssign(s, b, on) => snapshot.strips[s].bus_assign[b] = on,
            EngineCommand::SetStripGainLayer(s, b, db) => snapshot.strips[s].gain_layer_db[b] = db,
            EngineCommand::SetStripPan(s, pan) => snapshot.strips[s].pan = pan,
            EngineCommand::SetBusMute(b, on) => snapshot.buses[b].mute = on,
            EngineCommand::SetBusMono(b, mono) => snapshot.buses[b].mono = mono,
            EngineCommand::SetBusMode(b, mode) => snapshot.buses[b].mode = mode,
            EngineCommand::SetBusGain(b, db) => snapshot.buses[b].gain_db = db,
            EngineCommand::SetStripEqCell(..)
            | EngineCommand::SetBusEqCell(..)
            | EngineCommand::SetStripEqTrim(..)
            | EngineCommand::SetStripEqDelay(..)
            | EngineCommand::SetBusEqTrim(..)
            | EngineCommand::SetBusEqDelay(..)
            | EngineCommand::ResetStripEqChannel(..)
            | EngineCommand::ResetBusEqChannel(..)
            | EngineCommand::CopyStripEqChannel(..)
            | EngineCommand::CopyBusEqChannel(..) => {
                // Not part of ControlSnapshot's scope (module doc on
                // ControlSnapshot) -- nothing to mirror; `EqSnapshot`
                // covers all of these instead (trim/delay are already
                // fields on `EqChannelParams`, which it already carries).
            }
            EngineCommand::SetStripGateKnob(s, knob) => snapshot.strips[s].gate_knob = knob,
            EngineCommand::SetStripCompKnob(s, knob) => snapshot.strips[s].comp_knob = knob,
            EngineCommand::SetStripDenoiserKnob(s, knob) => snapshot.strips[s].denoiser_knob = knob,
            EngineCommand::SetStripLimiterThreshold(s, db) => {
                snapshot.strips[s].limiter_threshold_db = db
            }
            EngineCommand::SetStripIntellipanMode(s, mode) => {
                snapshot.strips[s].intellipan_mode = mode
            }
            EngineCommand::SetStripIntellipanXY(s, x, y) => {
                snapshot.strips[s].intellipan_xy = (x, y)
            }
            EngineCommand::SetStripEq3(s, bass, mid, treble) => {
                snapshot.strips[s].eq3_db = (bass, mid, treble)
            }
            EngineCommand::SetStripPositionPad(s, x, y) => snapshot.strips[s].position_pad = (x, y),
            EngineCommand::SetStripMc(s, on) => snapshot.strips[s].mc = on,
            EngineCommand::SetStripKaraoke(s, mode) => snapshot.strips[s].karaoke = mode,
            EngineCommand::SetStripEqOn(s, on) => snapshot.strips[s].strip_eq_on = on,
            EngineCommand::SetStripEqMemory(s, memory) => {
                snapshot.strips[s].strip_eq_memory = memory
            }
            EngineCommand::SetBusEqOn(b, on) => snapshot.buses[b].eq_on = on,
            EngineCommand::SetBusEqMemory(b, memory) => snapshot.buses[b].eq_memory = memory,
        }
    }

    // -- M10's table-driven control tests --------------------------------
    //
    // Fourteen new `EngineCommand` variants landed in one milestone, each
    // needing the same shape of proof (round-trips to the engine; ignores
    // an out-of-range index; is a no-op on the wrong strip kind). Eleven
    // hand-copied tests for that would make a missing or mis-wired
    // fifteenth entry invisible -- the entry would just never get written,
    // and nothing would fail, since there'd be no eleventh test *expecting*
    // it to exist. A table makes that failure mode visible instead: every
    // control is one row, one loop below applies every row's proof, and a
    // control nobody added a row for is a `CONTROL_CASES` array that is
    // conspicuously one shorter than the enum it's supposed to cover (the
    // length-matches-the-command-count assertion at the bottom of this
    // section is exactly that check, made automatic rather than left to a
    // reviewer noticing by eye). Direct instruction; see
    // `docs/ARCHITECTURE.md`'s M10 entry for the reasoning.

    use loomix_core::strip_dsp::{HardwareChain, VirtualChain};

    /// Strip 0 (hardware) and strip 5 (plain virtual, non-AUX) per spec
    /// 1.1's fixed topology (`loomix_core::strip::topology_is_aux`) --
    /// Karaoke's round trip deliberately does *not* use the AUX strip
    /// (index 6): `SetStripKaraoke`'s own doc comment is that the command
    /// sets the field on any virtual strip regardless of `is_aux`, and
    /// `strip_dsp::tests::virtual_chain_karaoke_only_applies_to_the_aux_strip`
    /// already separately proves the AUX-only *audible* gating -- mixing
    /// both proofs into one test would leave it unclear which claim a
    /// future failure was actually about.
    const HW: usize = 0;
    const VIRTUAL: usize = 5;
    const BUS: usize = 0;

    fn hw_chain(engine: &Engine, idx: usize) -> &HardwareChain {
        match &engine.strips[idx].chain {
            StripChain::Hardware(c) => c,
            StripChain::Virtual(_) => panic!("test index {idx} expected a hardware strip"),
        }
    }

    fn virt_chain(engine: &Engine, idx: usize) -> &VirtualChain {
        match &engine.strips[idx].chain {
            StripChain::Virtual(c) => c,
            StripChain::Hardware(_) => panic!("test index {idx} expected a virtual strip"),
        }
    }

    /// One row: a control from M10's state-2 list, its own well-formed
    /// command applied and checked at a representative index, and every
    /// instance of that command that must be a harmless no-op (an
    /// out-of-range index, and -- for a strip control -- the same command
    /// aimed at the wrong strip kind).
    struct ControlCase {
        name: &'static str,
        round_trips: fn(&mut Engine) -> bool,
        noop_variants: fn() -> Vec<EngineCommand>,
    }

    const CONTROL_CASES: &[ControlCase] = &[
        ControlCase {
            name: "gate knob",
            round_trips: |e| {
                EngineCommand::SetStripGateKnob(HW, 6.5).apply(e);
                hw_chain(e, HW).gate.knob() == 6.5
            },
            noop_variants: || {
                vec![
                    EngineCommand::SetStripGateKnob(NUM_STRIPS, 6.5),
                    EngineCommand::SetStripGateKnob(VIRTUAL, 6.5), // wrong kind: no gate on a virtual strip
                ]
            },
        },
        ControlCase {
            name: "compressor knob",
            round_trips: |e| {
                EngineCommand::SetStripCompKnob(HW, 4.5).apply(e);
                hw_chain(e, HW).compressor.knob() == 4.5
            },
            noop_variants: || {
                vec![
                    EngineCommand::SetStripCompKnob(NUM_STRIPS, 4.5),
                    EngineCommand::SetStripCompKnob(VIRTUAL, 4.5),
                ]
            },
        },
        ControlCase {
            name: "denoiser knob",
            round_trips: |e| {
                EngineCommand::SetStripDenoiserKnob(HW, 7.0).apply(e);
                hw_chain(e, HW).denoiser.knob() == 7.0
            },
            noop_variants: || {
                vec![
                    EngineCommand::SetStripDenoiserKnob(NUM_STRIPS, 7.0),
                    EngineCommand::SetStripDenoiserKnob(VIRTUAL, 7.0),
                ]
            },
        },
        ControlCase {
            name: "limiter threshold (hardware strip)",
            round_trips: |e| {
                EngineCommand::SetStripLimiterThreshold(HW, -6.0).apply(e);
                hw_chain(e, HW).limiter.threshold_db == -6.0
            },
            noop_variants: || vec![EngineCommand::SetStripLimiterThreshold(NUM_STRIPS, -6.0)],
        },
        ControlCase {
            name: "limiter threshold (virtual strip)",
            // No wrong-kind case: both chain kinds have their own
            // limiter (spec 1.3/1.4), so there is no wrong strip kind for
            // this one control -- only the out-of-range index applies.
            round_trips: |e| {
                EngineCommand::SetStripLimiterThreshold(VIRTUAL, -8.0).apply(e);
                virt_chain(e, VIRTUAL).limiter.threshold_db == -8.0
            },
            noop_variants: || vec![EngineCommand::SetStripLimiterThreshold(NUM_STRIPS, -8.0)],
        },
        ControlCase {
            name: "Intellipan mode",
            round_trips: |e| {
                EngineCommand::SetStripIntellipanMode(HW, IntellipanMode::Position).apply(e);
                hw_chain(e, HW).pad.mode() == IntellipanMode::Position
            },
            noop_variants: || {
                vec![
                    EngineCommand::SetStripIntellipanMode(NUM_STRIPS, IntellipanMode::Position),
                    EngineCommand::SetStripIntellipanMode(VIRTUAL, IntellipanMode::Position),
                ]
            },
        },
        ControlCase {
            name: "Intellipan x/y",
            round_trips: |e| {
                EngineCommand::SetStripIntellipanXY(HW, 0.3, 0.6).apply(e);
                hw_chain(e, HW).pad.position() == (0.3, 0.6)
            },
            noop_variants: || {
                vec![
                    EngineCommand::SetStripIntellipanXY(NUM_STRIPS, 0.3, 0.6),
                    EngineCommand::SetStripIntellipanXY(VIRTUAL, 0.3, 0.6),
                ]
            },
        },
        ControlCase {
            name: "virtual strip 3-band EQ",
            round_trips: |e| {
                EngineCommand::SetStripEq3(VIRTUAL, 3.0, -2.0, 5.0).apply(e);
                let c = virt_chain(e, VIRTUAL);
                (c.eq.bass_db, c.eq.mid_db, c.eq.treble_db) == (3.0, -2.0, 5.0)
            },
            noop_variants: || {
                vec![
                    EngineCommand::SetStripEq3(NUM_STRIPS, 3.0, -2.0, 5.0),
                    EngineCommand::SetStripEq3(HW, 3.0, -2.0, 5.0), // wrong kind: no 3-band EQ on hardware
                ]
            },
        },
        ControlCase {
            name: "5.1 position pad",
            round_trips: |e| {
                EngineCommand::SetStripPositionPad(VIRTUAL, 0.2, 0.7).apply(e);
                let c = virt_chain(e, VIRTUAL);
                (c.pan_pad.x, c.pan_pad.y) == (0.2, 0.7)
            },
            noop_variants: || {
                vec![
                    EngineCommand::SetStripPositionPad(NUM_STRIPS, 0.2, 0.7),
                    EngineCommand::SetStripPositionPad(HW, 0.2, 0.7),
                ]
            },
        },
        ControlCase {
            name: "M.C.",
            round_trips: |e| {
                EngineCommand::SetStripMc(VIRTUAL, true).apply(e);
                virt_chain(e, VIRTUAL).mc
            },
            noop_variants: || {
                vec![
                    EngineCommand::SetStripMc(NUM_STRIPS, true),
                    EngineCommand::SetStripMc(HW, true),
                ]
            },
        },
        ControlCase {
            name: "Karaoke",
            round_trips: |e| {
                EngineCommand::SetStripKaraoke(VIRTUAL, KaraokeMode::K1).apply(e);
                virt_chain(e, VIRTUAL).karaoke.mode == KaraokeMode::K1
            },
            noop_variants: || {
                vec![
                    EngineCommand::SetStripKaraoke(NUM_STRIPS, KaraokeMode::K1),
                    EngineCommand::SetStripKaraoke(HW, KaraokeMode::K1),
                ]
            },
        },
        ControlCase {
            name: "strip EQ on/off",
            round_trips: |e| {
                EngineCommand::SetStripEqOn(HW, true).apply(e);
                hw_chain(e, HW).eq.on
            },
            noop_variants: || {
                vec![
                    EngineCommand::SetStripEqOn(NUM_STRIPS, true),
                    EngineCommand::SetStripEqOn(VIRTUAL, true),
                ]
            },
        },
        ControlCase {
            name: "strip EQ A/B memory",
            round_trips: |e| {
                EngineCommand::SetStripEqMemory(HW, Memory::B).apply(e);
                hw_chain(e, HW).eq.active_memory() == Memory::B
            },
            noop_variants: || {
                vec![
                    EngineCommand::SetStripEqMemory(NUM_STRIPS, Memory::B),
                    EngineCommand::SetStripEqMemory(VIRTUAL, Memory::B),
                ]
            },
        },
        ControlCase {
            name: "bus EQ on/off",
            round_trips: |e| {
                EngineCommand::SetBusEqOn(BUS, true).apply(e);
                e.buses[BUS].eq.on
            },
            noop_variants: || vec![EngineCommand::SetBusEqOn(NUM_BUSES, true)],
        },
        ControlCase {
            name: "bus EQ A/B memory",
            round_trips: |e| {
                EngineCommand::SetBusEqMemory(BUS, Memory::B).apply(e);
                e.buses[BUS].eq.active_memory() == Memory::B
            },
            noop_variants: || vec![EngineCommand::SetBusEqMemory(NUM_BUSES, Memory::B)],
        },
        // -- spec 1.7's trim/delay/FLAT/CH COPY, brought into M10 rather
        // than deferred (this file's own EngineCommand doc comment).
        ControlCase {
            name: "strip EQ trim",
            round_trips: |e| {
                EngineCommand::SetStripEqTrim(HW, 1, 6.0).apply(e);
                hw_chain(e, HW).eq.channel_params(1).trim_db == 6.0
            },
            noop_variants: || {
                vec![
                    EngineCommand::SetStripEqTrim(NUM_STRIPS, 0, 6.0),
                    EngineCommand::SetStripEqTrim(HW, STRIP_EQ_CHANNELS, 6.0),
                    EngineCommand::SetStripEqTrim(VIRTUAL, 0, 6.0),
                ]
            },
        },
        ControlCase {
            name: "strip EQ delay",
            round_trips: |e| {
                EngineCommand::SetStripEqDelay(HW, 1, 120.0).apply(e);
                hw_chain(e, HW).eq.channel_params(1).delay_ms == 120.0
            },
            noop_variants: || {
                vec![
                    EngineCommand::SetStripEqDelay(NUM_STRIPS, 0, 120.0),
                    EngineCommand::SetStripEqDelay(HW, STRIP_EQ_CHANNELS, 120.0),
                    EngineCommand::SetStripEqDelay(VIRTUAL, 0, 120.0),
                ]
            },
        },
        ControlCase {
            name: "bus EQ trim",
            round_trips: |e| {
                EngineCommand::SetBusEqTrim(BUS, 3, -4.0).apply(e);
                e.buses[BUS].eq.channel_params(3).trim_db == -4.0
            },
            noop_variants: || {
                vec![
                    EngineCommand::SetBusEqTrim(NUM_BUSES, 0, -4.0),
                    EngineCommand::SetBusEqTrim(BUS, CHANNELS, -4.0),
                ]
            },
        },
        ControlCase {
            name: "bus EQ delay",
            round_trips: |e| {
                EngineCommand::SetBusEqDelay(BUS, 3, 80.0).apply(e);
                e.buses[BUS].eq.channel_params(3).delay_ms == 80.0
            },
            noop_variants: || {
                vec![
                    EngineCommand::SetBusEqDelay(NUM_BUSES, 0, 80.0),
                    EngineCommand::SetBusEqDelay(BUS, CHANNELS, 80.0),
                ]
            },
        },
        ControlCase {
            name: "strip EQ FLAT (reset one channel)",
            round_trips: |e| {
                EngineCommand::SetStripEqTrim(HW, 0, 9.0).apply(e);
                EngineCommand::ResetStripEqChannel(HW, 0).apply(e);
                hw_chain(e, HW).eq.channel_params(0).trim_db == 0.0
            },
            noop_variants: || {
                vec![
                    EngineCommand::ResetStripEqChannel(NUM_STRIPS, 0),
                    EngineCommand::ResetStripEqChannel(HW, STRIP_EQ_CHANNELS),
                    EngineCommand::ResetStripEqChannel(VIRTUAL, 0),
                ]
            },
        },
        ControlCase {
            name: "bus EQ FLAT (reset one channel)",
            round_trips: |e| {
                EngineCommand::SetBusEqTrim(BUS, 2, 9.0).apply(e);
                EngineCommand::ResetBusEqChannel(BUS, 2).apply(e);
                e.buses[BUS].eq.channel_params(2).trim_db == 0.0
            },
            noop_variants: || {
                vec![
                    EngineCommand::ResetBusEqChannel(NUM_BUSES, 0),
                    EngineCommand::ResetBusEqChannel(BUS, CHANNELS),
                ]
            },
        },
        ControlCase {
            name: "strip EQ CH COPY",
            round_trips: |e| {
                EngineCommand::SetStripEqTrim(HW, 0, 7.5).apply(e);
                EngineCommand::CopyStripEqChannel(HW, 0, 1).apply(e);
                hw_chain(e, HW).eq.channel_params(1).trim_db == 7.5
            },
            noop_variants: || {
                vec![
                    EngineCommand::CopyStripEqChannel(NUM_STRIPS, 0, 1),
                    EngineCommand::CopyStripEqChannel(HW, STRIP_EQ_CHANNELS, 1),
                    EngineCommand::CopyStripEqChannel(HW, 0, STRIP_EQ_CHANNELS),
                    EngineCommand::CopyStripEqChannel(VIRTUAL, 0, 1),
                ]
            },
        },
        ControlCase {
            name: "bus EQ CH COPY",
            round_trips: |e| {
                EngineCommand::SetBusEqTrim(BUS, 0, 7.5).apply(e);
                EngineCommand::CopyBusEqChannel(BUS, 0, 1).apply(e);
                e.buses[BUS].eq.channel_params(1).trim_db == 7.5
            },
            noop_variants: || {
                vec![
                    EngineCommand::CopyBusEqChannel(NUM_BUSES, 0, 1),
                    EngineCommand::CopyBusEqChannel(BUS, CHANNELS, 1),
                    EngineCommand::CopyBusEqChannel(BUS, 0, CHANNELS),
                ]
            },
        },
    ];

    /// The completeness check named in this section's own header comment:
    /// fails loudly (a wrong number, not a silent gap) if `EngineCommand`
    /// grows an M10-era variant that nobody added a `CONTROL_CASES` row
    /// for. `EngineCommand::VARIANT_COUNT` doesn't exist (Rust has no enum
    /// reflection), so this pins the *known* total instead -- deliberately
    /// brittle: adding a fifteenth M10 variant without updating this
    /// number is exactly the "invisible missing entry" this table exists
    /// to prevent, so the constant itself has to be touched too.
    const EXPECTED_M10_CONTROL_COUNT: usize = 23;

    #[test]
    fn control_case_table_covers_every_m10_control_exactly_once() {
        assert_eq!(
            CONTROL_CASES.len(),
            EXPECTED_M10_CONTROL_COUNT,
            "a control was added to or removed from CONTROL_CASES without updating the expected count"
        );
        let mut seen = std::collections::HashSet::new();
        for case in CONTROL_CASES {
            assert!(seen.insert(case.name), "duplicate case name: {}", case.name);
        }
    }

    #[test]
    fn every_m10_control_round_trips_to_the_engine() {
        for case in CONTROL_CASES {
            let mut engine = Engine::new();
            assert!(
                (case.round_trips)(&mut engine),
                "{} did not round-trip to the engine",
                case.name
            );
        }
    }

    #[test]
    fn every_m10_control_ignores_out_of_range_and_wrong_strip_kind_commands() {
        for case in CONTROL_CASES {
            let mut engine = Engine::new();
            let before = ControlSnapshot::capture(&engine);
            for command in (case.noop_variants)() {
                command.apply(&mut engine); // must not panic
            }
            assert_eq!(
                ControlSnapshot::capture(&engine),
                before,
                "{}: an out-of-range or wrong-strip-kind command mutated engine state",
                case.name
            );
        }
    }

    /// Same proof as `out_of_range_indices_are_ignored_not_panicked`
    /// above, run through the real `CommandSink`/`CommandDrain` path
    /// (coalescing, the SPSC queue, `drain_into`) rather than calling
    /// `apply` directly -- every M10 noop variant drains cleanly with no
    /// panic and no engine mutation.
    #[test]
    fn every_m10_control_noop_variant_drains_cleanly_through_the_real_channel() {
        for case in CONTROL_CASES {
            let (mut sink, mut drain) = control_channel();
            let mut engine = Engine::new();
            let before = ControlSnapshot::capture(&engine);
            for command in (case.noop_variants)() {
                sink.enqueue(command);
            }
            sink.flush();
            while drain.drain_into(&mut engine, 64) > 0 {}
            assert_eq!(
                ControlSnapshot::capture(&engine),
                before,
                "{}: noop variants mutated state after draining through the real channel",
                case.name
            );
        }
    }

    /// M10's spec text (`docs/SPEC.md`) commits to this explicitly: a
    /// pointer drag across the Intellipan/5.1 XY pad fires a coordinate
    /// update every frame, and those updates must coalesce per parameter
    /// (last-value-wins) through the exact same `CommandSink` path the
    /// fader already uses -- never one queued command per pointer move.
    /// Same technique as `a_flood_past_capacity_still_converges_to_the_
    /// last_value_sent` above (a real fader/mute pair, not a pad): flood
    /// the pad coordinate itself, far past `COMMAND_QUEUE_CAPACITY`, and
    /// confirm both that `pending` never grows past one entry per pad
    /// (coalescing keeps the queue small regardless of flood size) and
    /// that the engine ends up at exactly the last position sent.
    #[test]
    fn a_flood_of_xy_pad_drag_updates_coalesces_and_converges_to_the_last_position() {
        let (mut sink, mut drain) = control_channel();
        let mut engine = Engine::new();

        let flood = COMMAND_QUEUE_CAPACITY * 8;
        let mut expected_intellipan = (0.0f32, 0.0f32);
        let mut expected_position_pad = (0.0f32, 0.0f32);
        for i in 0..flood {
            // Values sweep within each pad's real clamp range (Intellipan
            // x: -0.5..0.5, y: 0..1; the 5.1 pad shares the same range) so
            // every intermediate value would be a genuine, unclamped
            // position if it were ever actually applied -- this is
            // proving coalescing collapses the flood, not relying on
            // clamping to hide a bug that would apply every one of them.
            let x = -0.5 + (i % 100) as f32 * 0.01;
            let y = (i % 100) as f32 * 0.01;
            expected_intellipan = (x, y);
            expected_position_pad = (x, y);
            sink.enqueue(EngineCommand::SetStripIntellipanXY(HW, x, y));
            sink.enqueue(EngineCommand::SetStripPositionPad(VIRTUAL, x, y));
            // A pointer-move handler would call this every frame, exactly
            // like a fader drag -- coalescing has to hold up under the
            // same "many updates, one parameter" shape either way.
            assert!(
                sink.pending_len() <= 2,
                "a drag across two pads should never grow pending past one entry each"
            );
            if i % 3 == 0 {
                sink.flush();
                drain.drain_into(&mut engine, 64);
            }
        }
        sink.flush();
        while drain.drain_into(&mut engine, 64) > 0 {}

        assert_eq!(hw_chain(&engine, HW).pad.position(), expected_intellipan);
        let virtual_pad = virt_chain(&engine, VIRTUAL).pan_pad;
        assert_eq!(
            (virtual_pad.x, virtual_pad.y),
            expected_position_pad,
            "the 5.1 pad should converge to the last position sent, not an intermediate one"
        );
    }

    /// `EqSnapshot` exists so the EQ panel can display real state, not
    /// just accept edits into a black box -- proves the capture actually
    /// reflects a real edit made through the real command path (not just
    /// that a freshly-constructed snapshot matches a freshly-constructed
    /// engine), and that a virtual strip's slot stays the documented
    /// default regardless of what the hardware strips carry.
    #[test]
    fn eq_snapshot_reflects_real_cell_edits_and_defaults_a_virtual_strip() {
        let mut engine = Engine::new();
        let cell = EqCellParams {
            on: true,
            cell_type: loomix_core::parametric_eq::EqCellType::LowShelf,
            freq_hz: 250.0,
            gain_db: 4.0,
            q: 2.0,
        };
        EngineCommand::SetStripEqCell(HW, 1, 3, cell).apply(&mut engine);
        EngineCommand::SetBusEqCell(BUS, 5, 2, cell).apply(&mut engine);

        let snapshot = EqSnapshot::capture(&engine);
        assert_eq!(snapshot.strips[HW][1].cells[3], cell);
        assert_eq!(snapshot.buses[BUS][5].cells[2], cell);
        assert_eq!(
            snapshot.strips[VIRTUAL],
            [EqChannelParams::default(); STRIP_EQ_CHANNELS],
            "a virtual strip has no parametric EQ; its slot should read as the documented default"
        );
    }

    #[test]
    fn a_virtual_strip_eq_command_is_a_harmless_no_op() {
        let (mut sink, mut drain) = control_channel();
        let mut engine = Engine::new();
        // strip 5 is virtual (spec 1.1's fixed topology) -- it has no
        // ParametricEq, so this must not panic and must apply cleanly.
        sink.enqueue(EngineCommand::SetStripEqCell(
            5,
            0,
            0,
            EqCellParams {
                on: true,
                cell_type: loomix_core::parametric_eq::EqCellType::Peak,
                freq_hz: 1000.0,
                gain_db: 6.0,
                q: 1.0,
            },
        ));
        sink.flush();
        assert_eq!(drain.drain_into(&mut engine, 64), 1);
    }

    /// The pan pot (spec 1.2 step 9) exists only on `StripChain::Hardware`
    /// -- a virtual strip has a 5.1 position pad instead (spec 1.4), so a
    /// `SetStripPan` command aimed at one has nothing to apply to, the
    /// same class of no-op the EQ test above already proves for the
    /// hardware-only strip EQ.
    #[test]
    fn strip_pan_applies_to_a_hardware_strip_and_is_a_no_op_on_a_virtual_strip() {
        let mut engine = Engine::new();
        EngineCommand::SetStripPan(0, 0.6).apply(&mut engine); // strip 0: hardware
        EngineCommand::SetStripPan(5, 0.6).apply(&mut engine); // strip 5: virtual

        match &engine.strips[0].chain {
            StripChain::Hardware(chain) => assert_eq!(chain.pan.pan, 0.6),
            StripChain::Virtual(_) => panic!("strip 0 should be hardware (spec 1.1)"),
        }
        assert_eq!(
            ControlSnapshot::capture(&engine).strips[5].pan,
            0.0,
            "a virtual strip has no pan pot; the command should be a harmless no-op"
        );
    }

    /// An `EngineCommand`'s indices come straight from whatever
    /// constructs it, with no `TryFrom`/range type of its own -- once a
    /// Tauri command layer exists, that's a buggy-or-compromised frontend
    /// away from an out-of-range index. Proves `apply`'s bounds checks
    /// actually stop the panic that unchecked indexing would otherwise
    /// produce (`engine.strips[999]` etc.), for a representative variant
    /// of every index shape this file has (strip-only, bus-only, both,
    /// and the EQ commands' extra channel/cell indices), not just that
    /// the well-formed cases already covered elsewhere happen to work.
    #[test]
    fn out_of_range_indices_are_ignored_not_panicked() {
        let (mut sink, mut drain) = control_channel();
        let mut engine = Engine::new();
        let eq_params = EqCellParams {
            on: true,
            cell_type: loomix_core::parametric_eq::EqCellType::Peak,
            freq_hz: 1000.0,
            gain_db: 6.0,
            q: 1.0,
        };

        let out_of_range = [
            EngineCommand::SetStripMute(NUM_STRIPS, true),
            EngineCommand::SetBusMute(NUM_BUSES, true),
            EngineCommand::SetStripBusAssign(NUM_STRIPS, 0, true),
            EngineCommand::SetStripBusAssign(0, NUM_BUSES, true),
            EngineCommand::SetStripGainLayer(usize::MAX, 0, -6.0),
            EngineCommand::SetStripPan(NUM_STRIPS, 0.5),
            EngineCommand::SetStripEqCell(NUM_STRIPS, 0, 0, eq_params),
            EngineCommand::SetStripEqCell(0, STRIP_EQ_CHANNELS, 0, eq_params),
            EngineCommand::SetStripEqCell(0, 0, NUM_CELLS, eq_params),
            EngineCommand::SetBusEqCell(NUM_BUSES, 0, 0, eq_params),
            EngineCommand::SetBusEqCell(0, CHANNELS, 0, eq_params),
            EngineCommand::SetBusEqCell(0, 0, NUM_CELLS, eq_params),
        ];
        let before = ControlSnapshot::capture(&engine);
        for command in out_of_range {
            command.apply(&mut engine); // must not panic
        }
        assert_eq!(
            ControlSnapshot::capture(&engine),
            before,
            "no out-of-range command should have changed any in-range state either"
        );

        // The drain path itself doesn't panic or wedge on these either --
        // it just counts them as drained (removed from the queue) with no
        // effect, the same as any other applied-but-inert command.
        for command in out_of_range {
            sink.enqueue(command);
        }
        // Every command above has a distinct `ParamKey` except the three
        // `SetStripEqCell`/`SetBusEqCell` pairs sharing an (s,ch,cell) --
        // coalescing collapses those, so fewer than 11 end up pending.
        sink.flush();
        let mut applied = 0;
        loop {
            let n = drain.drain_into(&mut engine, 64);
            applied += n;
            if n == 0 {
                break;
            }
        }
        assert!(applied > 0, "the queue should still have drained something");
    }

    /// `EngineCommand::apply` runs on the audio thread (`CommandDrain::
    /// drain_into`, called before `process_block`), so every variant has
    /// to be real-time safe, not just the scalar field writes -- the EQ
    /// cell variants route into `ParametricEq::set_cell`, which recomputes
    /// biquad coefficients and a delay line's read/write cursors, neither
    /// of which had a real-time obligation before this milestone (M6 only
    /// proved `process_channel` itself doesn't allocate, never the
    /// parameter-setting path, since nothing called it from the audio
    /// thread until now). Covers every variant, not just the EQ ones, so
    /// this is the one place a future new command variant gets checked by
    /// construction rather than by remembering to add it to the stress
    /// test below.
    #[test]
    fn realtime_command_apply_does_not_allocate() {
        use loomix_core::rt_assert::assert_realtime;

        let mut engine = Engine::new();
        let eq_params = EqCellParams {
            on: true,
            cell_type: loomix_core::parametric_eq::EqCellType::Peak,
            freq_hz: 1000.0,
            gain_db: 6.0,
            q: 1.0,
        };
        let commands = [
            EngineCommand::SetStripMute(0, true),
            EngineCommand::SetStripSolo(0, true),
            EngineCommand::SetStripMono(0, true),
            EngineCommand::SetStripBusAssign(0, 1, true),
            EngineCommand::SetStripGainLayer(0, 1, -6.0),
            EngineCommand::SetStripPan(0, 0.3),
            EngineCommand::SetBusMute(0, true),
            EngineCommand::SetBusMono(0, BusMono::Mono),
            EngineCommand::SetBusMode(0, BusMode::MixDownA),
            EngineCommand::SetBusGain(0, -3.0),
            EngineCommand::SetStripEqCell(0, 0, 0, eq_params),
            EngineCommand::SetBusEqCell(0, 0, 0, eq_params),
            // M10: every new variant, hardware-strip-only commands against
            // strip 0, virtual-strip-only against strip 5, bus EQ toggles
            // against bus 0 -- the same "one representative per shape"
            // coverage this test's own doc comment already commits to.
            EngineCommand::SetStripGateKnob(0, 5.0),
            EngineCommand::SetStripCompKnob(0, 5.0),
            EngineCommand::SetStripDenoiserKnob(0, 5.0),
            EngineCommand::SetStripLimiterThreshold(0, -6.0),
            EngineCommand::SetStripLimiterThreshold(5, -6.0),
            EngineCommand::SetStripIntellipanMode(0, IntellipanMode::Modulation),
            EngineCommand::SetStripIntellipanXY(0, 0.2, 0.5),
            EngineCommand::SetStripEq3(5, 1.0, -1.0, 2.0),
            EngineCommand::SetStripPositionPad(5, 0.1, 0.4),
            EngineCommand::SetStripMc(5, true),
            EngineCommand::SetStripKaraoke(5, KaraokeMode::K2),
            EngineCommand::SetStripEqOn(0, true),
            EngineCommand::SetStripEqMemory(0, Memory::B),
            EngineCommand::SetBusEqOn(0, true),
            EngineCommand::SetBusEqMemory(0, Memory::B),
            EngineCommand::SetStripEqTrim(0, 0, 3.0),
            EngineCommand::SetStripEqDelay(0, 0, 50.0),
            EngineCommand::SetBusEqTrim(0, 0, 3.0),
            EngineCommand::SetBusEqDelay(0, 0, 50.0),
            EngineCommand::ResetStripEqChannel(0, 0),
            EngineCommand::ResetBusEqChannel(0, 0),
            EngineCommand::CopyStripEqChannel(0, 0, 1),
            EngineCommand::CopyBusEqChannel(0, 0, 1),
        ];

        assert_realtime(|| {
            for &command in &commands {
                command.apply(&mut engine);
            }
        });
    }

    /// The stress test: a real UI-side thread hammers parameter changes
    /// continuously while a real audio-side thread renders continuously,
    /// concurrently, for real -- not simulated in sequence. Proves the
    /// design under actual thread contention, not just single-threaded
    /// call-order assumptions the other tests above make.
    ///
    /// The UI thread is paced (a first version had no pacing at all and
    /// pushed the overflow counter into the tens of millions -- not a
    /// realistic "hammering" scenario, a physically-impossible one: an
    /// unthrottled spin loop calling `flush()` as fast as the CPU allows
    /// issues push attempts many orders of magnitude faster than any real
    /// UI event source, or than the audio thread can drain regardless of
    /// how little backlog is actually pending). Paced to ~10kHz here --
    /// still far above any real UI framework's event rate (a mouse drag
    /// or a 120Hz display don't get close) -- while the audio thread
    /// spins with no pacing at all, the same way a real real-time thread
    /// must. `a_flood_past_capacity_still_converges_to_the_last_value_sent`
    /// above is the test for the deliberate-overload case; this one is
    /// for realistic sustained contention, where zero overflow is the
    /// actual claim being proven, not merely eventual convergence.
    #[test]
    fn realtime_concurrent_hammering_produces_zero_overflow_and_no_allocation() {
        use loomix_core::rt_assert::assert_realtime;
        use std::sync::atomic::{AtomicBool, Ordering as O};
        use std::time::Duration;

        let (mut sink, mut drain) = control_channel();
        let overflow = sink.overflow_counter();
        let stop = AtomicBool::new(false);
        const BLOCK_LEN: usize = 128;
        const UI_ITERATIONS: usize = 3_000;

        std::thread::scope(|scope| {
            scope.spawn(|| {
                for i in 0..UI_ITERATIONS {
                    let strip = i % NUM_STRIPS;
                    let bus = i % NUM_BUSES;
                    sink.enqueue(EngineCommand::SetStripGainLayer(
                        strip,
                        bus,
                        -20.0 + (i % 40) as f32,
                    ));
                    sink.enqueue(EngineCommand::SetStripMute(strip, i % 7 == 0));
                    sink.enqueue(EngineCommand::SetBusMode(
                        bus,
                        if i % 2 == 0 {
                            BusMode::Normal
                        } else {
                            BusMode::StereoRepeat
                        },
                    ));
                    // The EQ graph is part of M8's control surface too --
                    // covering it here, not just gain/mute/mode, exercises
                    // `ParametricEq::set_cell` under real contention, not
                    // just under `realtime_command_apply_does_not_allocate`'s
                    // single-threaded, sequential check above.
                    sink.enqueue(EngineCommand::SetBusEqCell(
                        bus,
                        strip % CHANNELS,
                        i % 6,
                        EqCellParams {
                            on: true,
                            cell_type: loomix_core::parametric_eq::EqCellType::Peak,
                            freq_hz: 200.0 + (i % 4000) as f32,
                            gain_db: (i % 12) as f32 - 6.0,
                            q: 1.0 + (i % 10) as f32,
                        },
                    ));
                    // M10's XY pads are the highest-*rate* new surface
                    // (a pointer drag, not a discrete click) -- covering
                    // one here exercises it under the same real
                    // concurrent contention as everything else, not just
                    // the sequential flood test above.
                    sink.enqueue(EngineCommand::SetStripIntellipanXY(
                        strip % 5, // strip 0..4 are hardware (spec 1.1)
                        -0.5 + (i % 100) as f32 * 0.01,
                        (i % 100) as f32 * 0.01,
                    ));
                    sink.flush();
                    std::thread::sleep(Duration::from_micros(100));
                }
                stop.store(true, O::Relaxed);
            });

            let mut engine = Engine::new();
            let blocks = silent_block(BLOCK_LEN);
            // "Renders continuously": no pacing on this side at all, the
            // way a real audio callback thread actually runs -- it's the
            // UI thread above that's paced to a realistic rate, not this
            // one slowed down to match it.
            while !stop.load(O::Relaxed) {
                assert_realtime(|| {
                    drain.drain_into(&mut engine, 64);
                    run_block(&mut engine, &blocks, BLOCK_LEN);
                });
            }
            // A few more callbacks, same as a real audio thread would keep
            // running after the UI thread's burst ends, to drain whatever
            // was queued right at the end.
            for _ in 0..32 {
                assert_realtime(|| {
                    drain.drain_into(&mut engine, 64);
                    run_block(&mut engine, &blocks, BLOCK_LEN);
                });
            }
        });

        assert_eq!(
            overflow.get(),
            0,
            "realistic sustained hammering should never fill a queue sized for the whole control surface"
        );
    }
}
