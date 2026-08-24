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
use loomix_core::parametric_eq::{EqCellParams, NUM_CELLS};
use loomix_core::strip_dsp::StripChain;
use loomix_core::{Engine, Meter, CHANNELS, NUM_BUSES, NUM_STRIPS};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// spec 1.2 step 7: the hardware strip EQ is stereo, unlike the bus EQ's
/// independent [`CHANNELS`] (8).
const STRIP_EQ_CHANNELS: usize = 2;

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
    BusMute(usize),
    BusMono(usize),
    BusMode(usize),
    BusGain(usize),
    StripEqCell(usize, usize, usize),
    BusEqCell(usize, usize, usize),
}

impl EngineCommand {
    fn key(&self) -> ParamKey {
        match *self {
            Self::SetStripMute(s, _) => ParamKey::StripMute(s),
            Self::SetStripSolo(s, _) => ParamKey::StripSolo(s),
            Self::SetStripMono(s, _) => ParamKey::StripMono(s),
            Self::SetStripBusAssign(s, b, _) => ParamKey::StripBusAssign(s, b),
            Self::SetStripGainLayer(s, b, _) => ParamKey::StripGainLayer(s, b),
            Self::SetBusMute(b, _) => ParamKey::BusMute(b),
            Self::SetBusMono(b, _) => ParamKey::BusMono(b),
            Self::SetBusMode(b, _) => ParamKey::BusMode(b),
            Self::SetBusGain(b, _) => ParamKey::BusGain(b),
            Self::SetStripEqCell(s, ch, cell, _) => ParamKey::StripEqCell(s, ch, cell),
            Self::SetBusEqCell(b, ch, cell, _) => ParamKey::BusEqCell(b, ch, cell),
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
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StripSnapshot {
    pub mute: bool,
    pub solo: bool,
    pub mono: bool,
    pub bus_assign: [bool; NUM_BUSES],
    pub gain_layer_db: [f32; NUM_BUSES],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BusSnapshot {
    pub mute: bool,
    pub mono: BusMono,
    pub mode: BusMode,
    pub gain_db: f32,
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
            strips: std::array::from_fn(|s| StripSnapshot {
                mute: engine.strips[s].mute,
                solo: engine.strips[s].solo,
                mono: engine.strips[s].mono,
                bus_assign: engine.strips[s].bus_assign,
                gain_layer_db: std::array::from_fn(|b| engine.strips[s].gain_layer_db(b)),
            }),
            buses: std::array::from_fn(|b| BusSnapshot {
                mute: engine.buses[b].mute,
                mono: engine.buses[b].mono,
                mode: engine.buses[b].mode,
                gain_db: engine.buses[b].gain_db(),
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
#[derive(Debug, Clone, Copy, Default)]
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
            EngineCommand::SetBusMute(b, on) => snapshot.buses[b].mute = on,
            EngineCommand::SetBusMono(b, mono) => snapshot.buses[b].mono = mono,
            EngineCommand::SetBusMode(b, mode) => snapshot.buses[b].mode = mode,
            EngineCommand::SetBusGain(b, db) => snapshot.buses[b].gain_db = db,
            EngineCommand::SetStripEqCell(..) | EngineCommand::SetBusEqCell(..) => {
                // Not part of ControlSnapshot's scope (module doc on
                // ControlSnapshot) -- nothing to mirror.
            }
        }
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
            EngineCommand::SetBusMute(0, true),
            EngineCommand::SetBusMono(0, BusMono::Mono),
            EngineCommand::SetBusMode(0, BusMode::MixDownA),
            EngineCommand::SetBusGain(0, -3.0),
            EngineCommand::SetStripEqCell(0, 0, 0, eq_params),
            EngineCommand::SetBusEqCell(0, 0, 0, eq_params),
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
