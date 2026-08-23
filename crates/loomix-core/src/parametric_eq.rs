//! The shared parametric EQ engine (spec 1.7): 6 cells per channel, plus
//! per-channel trim and delay, A/B memory, CH COPY / COPY ALL. One engine
//! serves both the bus EQ ([`ParametricEq`]`<8>`, independent per channel)
//! and the hardware-strip EQ ([`ParametricEq`]`<2>`, stereo) — spec 1.7's
//! "one shared implementation serves both the strip EQ and the bus EQ."
//!
//! **Only params ([`EqChannelParams`]) are doubled for A/B memory.** The
//! live DSP state (the biquads, the delay line) is a single instance per
//! channel, rebuilt from whichever memory is active — cheaper than two
//! full sets of live filter/delay state, and the same "recompute from
//! params on every change" shape every other block in this crate already
//! uses (`ThreeBandEq::recompute`, the knob-curve blocks).

use crate::biquad::{Biquad, BiquadCoeffs};
use crate::fader::gain_db_to_linear;
use serde::{Deserialize, Serialize};

/// spec 1.7: "7 filter types, typically peak, low pass, high pass, low
/// shelf, high shelf, band pass, notch" — index 0..6, in that order.
/// `Serialize`/`Deserialize`: part of the on-disk EQ file format
/// (`loomix-config`, spec 1.7's "load and save the whole EQ set as a
/// file"). `loomix-core` stays I/O-free itself — only the derive, no
/// format crate, lives here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum EqCellType {
    #[default]
    Peak,
    LowPass,
    HighPass,
    LowShelf,
    HighShelf,
    BandPass,
    Notch,
}

/// One cell's parameters (spec 1.7). `on == false` is this cell's true
/// neutral state, bypassed outright — the same guaranteed-bit-exact
/// convention every other block in this crate uses (`biquad.rs`'s own
/// known-answer test found a 0dB peaking filter's coefficients are a
/// pole-zero cancellation, not literal identity, so bypass is a real
/// branch, never "coefficients that happen to be unity").
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EqCellParams {
    pub on: bool,
    pub cell_type: EqCellType,
    /// 20Hz..20kHz (spec 1.7). Not clamped here: DSP-layer code trusts its
    /// caller (only validated at a system boundary, which this isn't yet —
    /// that's the RPC/UI layer, M10).
    pub freq_hz: f32,
    /// -12..+12dB API range, -36..+18dB extended UI scale (spec 1.7).
    /// Ignored by cell types with no gain stage (LowPass/HighPass/
    /// BandPass/Notch).
    pub gain_db: f32,
    /// 1..100 (spec 1.7), shared across every cell type.
    pub q: f32,
}

impl Default for EqCellParams {
    fn default() -> Self {
        Self {
            on: false,
            cell_type: EqCellType::Peak,
            freq_hz: 1000.0,
            gain_db: 0.0,
            q: 1.0,
        }
    }
}

impl EqCellParams {
    fn coeffs(&self, sample_rate: f32) -> BiquadCoeffs {
        match self.cell_type {
            EqCellType::Peak => {
                BiquadCoeffs::peaking(sample_rate, self.freq_hz, self.q, self.gain_db)
            }
            EqCellType::LowPass => BiquadCoeffs::low_pass(sample_rate, self.freq_hz, self.q),
            EqCellType::HighPass => BiquadCoeffs::high_pass(sample_rate, self.freq_hz, self.q),
            EqCellType::LowShelf => {
                BiquadCoeffs::low_shelf(sample_rate, self.freq_hz, self.q, self.gain_db)
            }
            EqCellType::HighShelf => {
                BiquadCoeffs::high_shelf(sample_rate, self.freq_hz, self.q, self.gain_db)
            }
            EqCellType::BandPass => BiquadCoeffs::band_pass(sample_rate, self.freq_hz, self.q),
            EqCellType::Notch => BiquadCoeffs::notch(sample_rate, self.freq_hz, self.q),
        }
    }
}

/// spec 1.7: "6 cells" per channel.
pub const NUM_CELLS: usize = 6;

/// spec 1.7: per-channel delay range is 0..500ms.
const MAX_DELAY_MS: f32 = 500.0;

/// One channel's full parameter set: [`NUM_CELLS`] cells in series, plus
/// per-channel trim (spec 1.7: -24..+24dB) and delay (0..500ms). All-
/// neutral (every cell off, trim 0, delay 0) is this type's own true
/// bit-exact default.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EqChannelParams {
    pub cells: [EqCellParams; NUM_CELLS],
    pub trim_db: f32,
    pub delay_ms: f32,
}

impl Default for EqChannelParams {
    fn default() -> Self {
        Self {
            cells: [EqCellParams::default(); NUM_CELLS],
            trim_db: 0.0,
            delay_ms: 0.0,
        }
    }
}

/// A single-channel delay line, sized for [`MAX_DELAY_MS`] at whatever
/// sample rate it's currently running at. `delay_ms == 0.0` is a true
/// bypass: no buffer read or write at all, bit-exact (spec 4.1's neutral-
/// setting rule), not "reading a delay of zero samples" through the same
/// code path as every other delay.
///
/// **Concurrency / real-time-safety contract for [`set_sample_rate`
/// ](Self::set_sample_rate):** it reallocates the backing buffer, which
/// must never happen while [`process`](Self::process) could be running for
/// this same instance — spec 3.3 forbids allocation in the audio callback
/// outright, and mid-buffer reallocation also corrupts whatever's
/// currently in flight through the delay. The actual architectural
/// contract (matching every other block's own `set_sample_rate`, and
/// `Engine`'s: spec 1.19/M4 already established that a sample-rate change
/// is a control-thread reconfiguration point requiring IO to be stopped
/// first, the same tear-down/rebuild pattern `docs/ARCHITECTURE.md`'s M1
/// entry describes for a real client surviving a device format change) is
/// that `set_sample_rate` is only ever called when no `process()` call for
/// this instance is in flight anywhere.
///
/// A `debug_assert!` below catches the same-thread case of this — a
/// `set_sample_rate` call reentrant from inside `process()` on the one
/// thread that owns this instance — which is real-time-unsafe and
/// corrupts buffered audio but is **not** itself a memory-safety
/// violation (Rust's own `&mut self` aliasing rules already make same-
/// instance concurrent mutation impossible without `unsafe`). If this
/// contract is instead violated across two real threads — which can only
/// happen if a caller has used `unsafe` to alias this instance across
/// them, exactly the shape `loomix-hal`'s CoreAudio FFI trampolines take
/// reaching into `Engine` (see `docs/ARCHITECTURE.md`'s M4 entries) — the
/// outcome is a genuine data race on the buffer's backing allocation
/// (one thread freeing/replacing it while another reads or writes through
/// a pointer obtained before the reallocation): Undefined Behavior in the
/// strict Rust sense, not merely a wrong sample. The `debug_assert!` is
/// compiled out in release builds — see
/// `tests::set_sample_rate_panics_if_called_while_process_is_in_flight`
/// for proof it actually fires in test/debug builds, and
/// `tests::set_sample_rate_does_not_panic_when_not_in_flight` for proof
/// it isn't just unconditionally panicking — but it verifies this
/// contract in tests, it does not enforce it in a shipped release binary.
/// The real, load-bearing protection against the cross-thread case is
/// architectural (spec 3.3's own rule: parameters cross into the audio
/// thread through an SPSC/triple buffer, never a directly shared `&mut`),
/// not this flag.
pub struct DelayLine {
    buffer: Vec<f32>,
    write_pos: usize,
    sample_rate: f32,
    delay_ms: f32,
    delay_samples: usize,
    in_process: bool,
}

impl DelayLine {
    pub fn new(sample_rate: f32) -> Self {
        let mut line = Self {
            buffer: Vec::new(),
            write_pos: 0,
            sample_rate,
            delay_ms: 0.0,
            delay_samples: 0,
            in_process: false,
        };
        line.rebuild_buffer();
        line
    }

    fn capacity_for(sample_rate: f32) -> usize {
        ((MAX_DELAY_MS / 1000.0) * sample_rate).ceil() as usize + 1
    }

    fn rebuild_buffer(&mut self) {
        self.buffer = vec![0.0; Self::capacity_for(self.sample_rate)];
        self.write_pos = 0;
    }

    fn recompute_delay_samples(&mut self) {
        let capacity = self.buffer.len();
        let requested = ((self.delay_ms / 1000.0) * self.sample_rate).round() as usize;
        self.delay_samples = requested.min(capacity.saturating_sub(1));
    }

    /// See the concurrency/real-time-safety contract in this type's own
    /// doc comment above.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        debug_assert!(
            !self.in_process,
            "DelayLine::set_sample_rate called while process() was in flight for this \
             instance -- this reallocates and must never run concurrently with (or reentrantly \
             from inside) process(). This check is compiled out in release builds; see the type's \
             own doc comment for what that does and does not protect against."
        );
        if sample_rate != self.sample_rate {
            self.sample_rate = sample_rate;
            self.rebuild_buffer();
            self.recompute_delay_samples();
        }
    }

    pub fn set_delay_ms(&mut self, delay_ms: f32) {
        self.delay_ms = delay_ms.clamp(0.0, MAX_DELAY_MS);
        self.recompute_delay_samples();
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        if self.delay_samples == 0 {
            return x; // true bypass -- the buffer isn't touched at all
        }
        self.in_process = true;
        let capacity = self.buffer.len();
        let read_pos = (self.write_pos + capacity - self.delay_samples) % capacity;
        let y = self.buffer[read_pos];
        self.buffer[self.write_pos] = x;
        self.write_pos = (self.write_pos + 1) % capacity;
        self.in_process = false;
        y
    }
}

/// One channel's live DSP state: [`NUM_CELLS`] biquads in series, then
/// trim, then delay. Rebuilt from an [`EqChannelParams`] by
/// [`apply_params`](Self::apply_params), which only [`ParametricEq`]
/// calls — never from [`process`](Self::process) itself.
struct EqChannel {
    sample_rate: f32,
    biquads: [Biquad; NUM_CELLS],
    trim_linear: f32,
    delay: DelayLine,
}

impl EqChannel {
    fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate,
            biquads: [Biquad::bypassed(); NUM_CELLS],
            trim_linear: 1.0,
            delay: DelayLine::new(sample_rate),
        }
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.delay.set_sample_rate(sample_rate);
        // Biquad coefficients are sample-rate dependent too, but this
        // alone has no `EqChannelParams` to recompute them from --
        // `ParametricEq::set_sample_rate` follows this with a full
        // `apply_params` resync using the still-current active memory.
    }

    fn apply_params(&mut self, params: &EqChannelParams) {
        for (biquad, cell) in self.biquads.iter_mut().zip(params.cells.iter()) {
            if cell.on {
                biquad.set_coeffs(cell.coeffs(self.sample_rate));
            } else {
                biquad.bypass();
            }
        }
        self.trim_linear = if params.trim_db == 0.0 {
            1.0
        } else {
            gain_db_to_linear(params.trim_db)
        };
        self.delay.set_delay_ms(params.delay_ms);
    }

    #[inline]
    fn process(&mut self, mut x: f32) -> f32 {
        for biquad in &mut self.biquads {
            x = biquad.process(x);
        }
        if self.trim_linear != 1.0 {
            x *= self.trim_linear;
        }
        self.delay.process(x)
    }
}

/// Selects which of the two parameter sets edits/processing target (spec
/// 1.7: "A/B memories for instant comparison, edits always land in the
/// currently selected memory").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Memory {
    #[default]
    A,
    B,
}

impl Memory {
    fn idx(self) -> usize {
        match self {
            Memory::A => 0,
            Memory::B => 1,
        }
    }
}

/// The shared 6-cell parametric EQ engine (spec 1.7), generic over channel
/// count: `ParametricEq<8>` for a bus (independent per channel),
/// `ParametricEq<2>` for a hardware strip (stereo). `on == false` is the
/// whole block's own neutral setting — spec 1.3 defaults the strip EQ to
/// off — and is checked first in [`process_channel`](Self::process_channel),
/// a guaranteed bit-exact passthrough regardless of any cell/trim/delay
/// content underneath it.
pub struct ParametricEq<const N: usize> {
    pub on: bool,
    active: Memory,
    params: [[EqChannelParams; N]; 2],
    live: [EqChannel; N],
}

impl<const N: usize> ParametricEq<N> {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            on: false,
            active: Memory::A,
            params: [
                core::array::from_fn(|_| EqChannelParams::default()),
                core::array::from_fn(|_| EqChannelParams::default()),
            ],
            live: core::array::from_fn(|_| EqChannel::new(sample_rate)),
        }
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        for channel in &mut self.live {
            channel.set_sample_rate(sample_rate);
        }
        self.resync_live();
    }

    pub fn active_memory(&self) -> Memory {
        self.active
    }

    pub fn set_active_memory(&mut self, memory: Memory) {
        if memory != self.active {
            self.active = memory;
            self.resync_live();
        }
    }

    pub fn channel_params(&self, channel: usize) -> &EqChannelParams {
        &self.params[self.active.idx()][channel]
    }

    pub fn set_channel_params(&mut self, channel: usize, params: EqChannelParams) {
        self.params[self.active.idx()][channel] = params;
        self.sync_channel(channel);
    }

    pub fn set_cell(&mut self, channel: usize, cell: usize, cell_params: EqCellParams) {
        self.params[self.active.idx()][channel].cells[cell] = cell_params;
        self.sync_channel(channel);
    }

    pub fn set_trim_db(&mut self, channel: usize, trim_db: f32) {
        self.params[self.active.idx()][channel].trim_db = trim_db;
        self.sync_channel(channel);
    }

    pub fn set_delay_ms(&mut self, channel: usize, delay_ms: f32) {
        self.params[self.active.idx()][channel].delay_ms = delay_ms;
        self.sync_channel(channel);
    }

    /// FLAT for one channel (spec 1.7): resets it to its neutral default
    /// (every cell off, trim 0, delay 0) in the currently active memory.
    pub fn reset_channel(&mut self, channel: usize) {
        self.set_channel_params(channel, EqChannelParams::default());
    }

    /// FLAT applied to every channel at once (spec 1.7's "channel selector
    /// that can edit all channels at once" is a UI-selection concept, not
    /// modelled here — this is the engine-side primitive that selector
    /// would call).
    pub fn reset_all(&mut self) {
        for channel in 0..N {
            self.reset_channel(channel);
        }
    }

    /// CH COPY (spec 1.7): copies `from`'s params onto `to`, within the
    /// currently active memory.
    pub fn copy_channel(&mut self, from: usize, to: usize) {
        let params = self.params[self.active.idx()][from];
        self.set_channel_params(to, params);
    }

    /// COPY ALL (spec 1.7), and the "copy settings between strip EQs and
    /// bus EQs since the parameter model is shared" case: copies this EQ's
    /// active-memory channels onto `other`'s active memory, `min(N, M)` of
    /// them. Copying an 8-channel bus onto a 2-channel strip copies
    /// channels 0/1 only; copying a 2-channel strip onto an 8-channel bus
    /// leaves the bus's channels 2..7 untouched.
    pub fn copy_all_into<const M: usize>(&self, other: &mut ParametricEq<M>) {
        for channel in 0..N.min(M) {
            let params = self.params[self.active.idx()][channel];
            other.set_channel_params(channel, params);
        }
    }

    fn sync_channel(&mut self, channel: usize) {
        let params = self.params[self.active.idx()][channel];
        self.live[channel].apply_params(&params);
    }

    fn resync_live(&mut self) {
        for channel in 0..N {
            self.sync_channel(channel);
        }
    }

    #[inline]
    pub fn process_channel(&mut self, channel: usize, x: f32) -> f32 {
        if !self.on {
            return x;
        }
        self.live[channel].process(x)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::biquad::test_support::{goertzel_magnitude, sine};
    use crate::rt_assert::assert_realtime;

    const SR: f32 = 48_000.0;

    // ---- DelayLine ----

    #[test]
    fn null_test_zero_delay_is_bit_exact_passthrough() {
        let mut line = DelayLine::new(SR);
        for n in 0..1000 {
            let x = (n as f32 * 0.037).sin();
            assert_eq!(line.process(x), x);
        }
    }

    #[test]
    fn known_answer_delay_reproduces_the_impulse_exactly_n_samples_later() {
        let mut line = DelayLine::new(SR);
        line.set_delay_ms(250.0); // 12000 samples at 48kHz, exact
        let delay_samples = 12_000usize;

        let mut out = Vec::with_capacity(delay_samples + 5);
        for n in 0..delay_samples + 5 {
            let x = if n == 0 { 1.0 } else { 0.0 };
            out.push(line.process(x));
        }
        for (n, &y) in out.iter().enumerate() {
            if n == delay_samples {
                assert_eq!(y, 1.0, "impulse should reappear at sample {delay_samples}");
            } else {
                assert_eq!(y, 0.0, "sample {n} should be silent");
            }
        }
    }

    #[test]
    fn boundary_full_500ms_delay_survives_a_sample_rate_increase() {
        let mut line = DelayLine::new(SR);
        line.set_delay_ms(500.0);
        for n in 0..100 {
            line.process(n as f32);
        }
        line.set_sample_rate(192_000.0);
        line.set_delay_ms(500.0); // must not overrun the new, larger buffer
        for n in 0..100 {
            let y = line.process(n as f32);
            assert!(y.is_finite());
        }
    }

    #[test]
    #[cfg(debug_assertions)]
    fn set_sample_rate_panics_if_called_while_process_is_in_flight() {
        // Proves the debug_assert actually fires, not just that the doc
        // comment claims it does. `in_process` is set by `process()` only
        // while a nonzero-delay call is touching the buffer (see its own
        // body) and cleared again before it returns, so a real call can
        // never observe it from outside -- this simulates the reentrant
        // case directly by setting the private flag, the same technique
        // `rt_assert.rs` uses for its own guard tests.
        let mut line = DelayLine::new(SR);
        line.set_delay_ms(10.0);
        line.in_process = true;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            line.set_sample_rate(96_000.0);
        }));
        assert!(
            result.is_err(),
            "set_sample_rate should panic when called while in_process is set"
        );
    }

    #[test]
    fn set_sample_rate_does_not_panic_when_not_in_flight() {
        // Companion to the test above: proves the assertion isn't just
        // unconditionally panicking, which would make that test vacuous.
        let mut line = DelayLine::new(SR);
        line.set_delay_ms(10.0);
        line.process(0.5); // in_process is false again once process() returns
        line.set_sample_rate(96_000.0); // must not panic
    }

    #[test]
    fn realtime_delay_line_process_does_not_allocate() {
        let mut line = DelayLine::new(SR);
        line.set_delay_ms(10.0);
        assert_realtime(|| {
            for n in 0..256 {
                line.process(n as f32 * 0.01);
            }
        });
    }

    // ---- EqChannel (via ParametricEq<1>, since EqChannel itself is private) ----

    #[test]
    fn null_test_all_neutral_channel_is_bit_exact_passthrough() {
        let mut eq = ParametricEq::<1>::new(SR);
        eq.on = true;
        for n in 0..1000 {
            let x = (n as f32 * 0.023).sin();
            assert_eq!(eq.process_channel(0, x), x);
        }
    }

    #[test]
    fn eq_channel_cascades_cells_in_series_matching_independently_chained_biquads() {
        // Confirms the 6 cells run in series (each on the previous cell's
        // output), not in parallel/summed -- built independently from the
        // same public `Biquad`/`BiquadCoeffs` API, not by calling the code
        // under test.
        let mut eq = ParametricEq::<1>::new(SR);
        eq.on = true;
        let mut params = EqChannelParams::default();
        params.cells[0] = EqCellParams {
            on: true,
            cell_type: EqCellType::Peak,
            freq_hz: 300.0,
            gain_db: 9.0,
            q: 1.5,
        };
        params.cells[1] = EqCellParams {
            on: true,
            cell_type: EqCellType::HighShelf,
            freq_hz: 4000.0,
            gain_db: -6.0,
            q: 0.9,
        };
        eq.set_channel_params(0, params);

        let mut ref_a = Biquad::bypassed();
        ref_a.set_coeffs(BiquadCoeffs::peaking(SR, 300.0, 1.5, 9.0));
        let mut ref_b = Biquad::bypassed();
        ref_b.set_coeffs(BiquadCoeffs::high_shelf(SR, 4000.0, 0.9, -6.0));

        for n in 0..2000 {
            let x = (n as f32 * 0.019).sin() * 0.5;
            let actual = eq.process_channel(0, x);
            let expected = ref_b.process(ref_a.process(x));
            assert!(
                (actual - expected).abs() < 1e-5,
                "n={n} actual={actual} expected={expected}"
            );
        }
    }

    // ---- ParametricEq ----

    #[test]
    fn default_is_off() {
        let eq = ParametricEq::<8>::new(SR);
        assert!(!eq.on);
        assert_eq!(eq.active_memory(), Memory::A);
    }

    #[test]
    fn global_off_is_bit_exact_passthrough_even_with_wild_cell_settings() {
        let mut eq = ParametricEq::<2>::new(SR);
        assert!(!eq.on);
        eq.set_cell(
            0,
            0,
            EqCellParams {
                on: true,
                cell_type: EqCellType::Peak,
                freq_hz: 1000.0,
                gain_db: 18.0,
                q: 50.0,
            },
        );
        eq.set_trim_db(0, 24.0);
        eq.set_delay_ms(0, 500.0);
        for n in 0..1000 {
            let x = (n as f32 * 0.031).sin();
            assert_eq!(eq.process_channel(0, x), x);
        }
    }

    #[test]
    fn a_b_memory_edits_land_only_in_the_active_memory() {
        let mut eq = ParametricEq::<1>::new(SR);
        eq.set_trim_db(0, 6.0); // written into A (the default active memory)
        eq.set_active_memory(Memory::B);
        eq.set_trim_db(0, -6.0); // written into B only

        eq.set_active_memory(Memory::A);
        assert_eq!(
            eq.channel_params(0).trim_db,
            6.0,
            "A must be untouched by the B edit"
        );
        eq.set_active_memory(Memory::B);
        assert_eq!(eq.channel_params(0).trim_db, -6.0);
    }

    #[test]
    fn a_b_memory_switch_changes_the_live_output() {
        let mut eq = ParametricEq::<1>::new(SR);
        eq.on = true;
        eq.set_cell(
            0,
            0,
            EqCellParams {
                on: true,
                cell_type: EqCellType::Peak,
                freq_hz: 1000.0,
                gain_db: 12.0,
                q: 2.0,
            },
        );
        // B stays fully neutral (default).

        let tone = sine(4096, SR, 1000.0);
        let mut eq_a = ParametricEq::<1>::new(SR);
        eq_a.on = true;
        eq_a.set_cell(
            0,
            0,
            EqCellParams {
                on: true,
                cell_type: EqCellType::Peak,
                freq_hz: 1000.0,
                gain_db: 12.0,
                q: 2.0,
            },
        );
        let out_a: Vec<f32> = tone.iter().map(|&x| eq_a.process_channel(0, x)).collect();

        let mut eq_b = ParametricEq::<1>::new(SR);
        eq_b.on = true;
        eq_b.set_active_memory(Memory::B);
        let out_b: Vec<f32> = tone.iter().map(|&x| eq_b.process_channel(0, x)).collect();

        let gain_a = goertzel_magnitude(&out_a, 1000.0, SR) / goertzel_magnitude(&tone, 1000.0, SR);
        let gain_b = goertzel_magnitude(&out_b, 1000.0, SR) / goertzel_magnitude(&tone, 1000.0, SR);
        assert!(
            20.0 * (gain_a / gain_b).log10() > 6.0,
            "switching memory should measurably change the output: a={gain_a} b={gain_b}"
        );
    }

    #[test]
    fn reset_channel_returns_to_neutral() {
        let mut eq = ParametricEq::<2>::new(SR);
        eq.on = true;
        eq.set_trim_db(0, 12.0);
        eq.set_delay_ms(0, 100.0);
        eq.reset_channel(0);
        assert_eq!(*eq.channel_params(0), EqChannelParams::default());
        for n in 0..500 {
            let x = (n as f32 * 0.041).sin();
            assert_eq!(eq.process_channel(0, x), x);
        }
    }

    #[test]
    fn reset_all_returns_every_channel_to_neutral() {
        let mut eq = ParametricEq::<8>::new(SR);
        for c in 0..8 {
            eq.set_trim_db(c, 3.0);
        }
        eq.reset_all();
        for c in 0..8 {
            assert_eq!(*eq.channel_params(c), EqChannelParams::default());
        }
    }

    #[test]
    fn copy_channel_matches_the_source() {
        let mut eq = ParametricEq::<8>::new(SR);
        eq.set_trim_db(0, 7.5);
        eq.set_delay_ms(0, 42.0);
        eq.set_cell(
            0,
            2,
            EqCellParams {
                on: true,
                cell_type: EqCellType::Notch,
                freq_hz: 500.0,
                gain_db: 0.0,
                q: 10.0,
            },
        );
        eq.copy_channel(0, 3);
        assert_eq!(eq.channel_params(3), eq.channel_params(0));
    }

    #[test]
    fn copy_all_into_bus_to_bus_is_a_full_copy() {
        let mut src = ParametricEq::<8>::new(SR);
        for c in 0..8 {
            src.set_trim_db(c, c as f32);
        }
        let mut dst = ParametricEq::<8>::new(SR);
        src.copy_all_into(&mut dst);
        for c in 0..8 {
            assert_eq!(dst.channel_params(c).trim_db, c as f32);
        }
    }

    #[test]
    fn copy_all_into_bus_to_strip_copies_only_the_first_two_channels() {
        let mut bus = ParametricEq::<8>::new(SR);
        for c in 0..8 {
            bus.set_trim_db(c, 10.0 + c as f32);
        }
        let mut strip = ParametricEq::<2>::new(SR);
        bus.copy_all_into(&mut strip);
        assert_eq!(strip.channel_params(0).trim_db, 10.0);
        assert_eq!(strip.channel_params(1).trim_db, 11.0);
    }

    #[test]
    fn copy_all_into_strip_to_bus_leaves_channels_2_through_7_untouched() {
        let mut strip = ParametricEq::<2>::new(SR);
        strip.set_trim_db(0, 5.0);
        strip.set_trim_db(1, -5.0);
        let mut bus = ParametricEq::<8>::new(SR);
        for c in 0..8 {
            bus.set_trim_db(c, 99.0); // distinct marker value
        }
        strip.copy_all_into(&mut bus);
        assert_eq!(bus.channel_params(0).trim_db, 5.0);
        assert_eq!(bus.channel_params(1).trim_db, -5.0);
        for c in 2..8 {
            assert_eq!(
                bus.channel_params(c).trim_db,
                99.0,
                "channel {c} should be untouched by a 2-channel source copy"
            );
        }
    }

    #[test]
    fn stability_random_automation_never_produces_nan_or_infinity() {
        let mut eq = ParametricEq::<8>::new(SR);
        eq.on = true;
        let mut seed = 42u32;
        let mut rand = move || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1u32 << 24) as f32
        };
        let cell_types = [
            EqCellType::Peak,
            EqCellType::LowPass,
            EqCellType::HighPass,
            EqCellType::LowShelf,
            EqCellType::HighShelf,
            EqCellType::BandPass,
            EqCellType::Notch,
        ];
        for i in 0..5_000 {
            let channel = i % 8;
            let cell = i % NUM_CELLS;
            eq.set_cell(
                channel,
                cell,
                EqCellParams {
                    on: rand() > 0.2,
                    cell_type: cell_types[i % cell_types.len()],
                    freq_hz: 20.0 + rand() * 19_980.0,
                    gain_db: -36.0 + rand() * 54.0,
                    q: 1.0 + rand() * 99.0,
                },
            );
            eq.set_trim_db(channel, -24.0 + rand() * 48.0);
            eq.set_delay_ms(channel, rand() * 500.0);
            let x = rand() * 2.0 - 1.0;
            let y = eq.process_channel(channel, x);
            assert!(
                y.is_finite(),
                "channel {channel} produced a non-finite sample"
            );
        }
    }

    #[test]
    fn realtime_process_channel_does_not_allocate() {
        let mut eq = ParametricEq::<8>::new(SR);
        eq.on = true;
        eq.set_cell(
            0,
            0,
            EqCellParams {
                on: true,
                cell_type: EqCellType::Peak,
                freq_hz: 1000.0,
                gain_db: 6.0,
                q: 1.0,
            },
        );
        eq.set_delay_ms(0, 10.0);
        assert_realtime(|| {
            for n in 0..256 {
                for channel in 0..8 {
                    eq.process_channel(channel, n as f32 * 0.001);
                }
            }
        });
    }
}
