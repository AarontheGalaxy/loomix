//! Cross-language verification fixture for the parametric EQ's frequency
//! response (spec 1.7, M6): a fixed set of cell configurations and their
//! magnitude response at spec-relevant frequencies, computed here through
//! the real `ParametricEq` code path and checked into
//! `testdata/fixtures/eq_response_reference.json`. `ui/src/eqResponse.ts`
//! implements the same per-cell-type math as an independent second
//! implementation and is tested against this same file -- not against
//! numbers copied by hand, and not trusting this fixture as ground truth
//! on faith: a graph that lies about what the engine is actually doing is
//! worse than no graph.
//!
//! Regenerate deliberately, review the diff -- same convention as
//! `testdata/golden/` and `testdata/bench-baseline/` (spec 4.1 layer 4):
//! `cargo test -p loomix-core --test eq_response_fixture -- --ignored --nocapture`
//! and copy the printed JSON object into the fixture file.

use loomix_core::parametric_eq::{EqCellParams, EqCellType, EqChannelParams, ParametricEq};
use loomix_core::render::sine_tone;
use serde_json::json;
use std::path::PathBuf;

const SAMPLE_RATE: f32 = 48_000.0;
// 32768 samples gives a ~1.46Hz bin width at 48kHz -- fine enough that
// snapping a probe frequency to its nearest bin (below) barely moves it,
// while still keeping every probe an exact whole number of cycles in the
// analysis window (see `probe_frequencies`'s doc comment for why that
// matters).
const NUM_SAMPLES: usize = 32_768;

/// 24 log-spaced points across spec 1.7's 20Hz..20kHz cell range, each
/// snapped to the nearest bin `render::goertzel_magnitude` will actually
/// analyze at `NUM_SAMPLES` (i.e. `k * SAMPLE_RATE / NUM_SAMPLES` for
/// integer `k`). Without this, a tone generated at a non-bin-aligned
/// frequency leaks across the whole spectrum in a finite window, and a
/// steep filter (a notch center, a band-pass's skirt, a high-pass's deep
/// stopband) amplifies that leakage into a large, spurious dB error --
/// found empirically, not anticipated in advance: the first version of
/// this fixture used un-snapped log-spaced frequencies directly and
/// `ui/src/eqResponse.test.ts`'s cross-check against it failed by over
/// 1dB at exactly the three deep-attenuation cases (`high_pass` at 20Hz,
/// `band_pass` at 20Hz, `notch` at its own center) this snapping fixes.
/// Snapping to an exact bin makes the Goertzel measurement and this
/// file's analytic TS counterpart evaluate literally the same frequency,
/// not two nearby ones.
fn probe_frequencies() -> Vec<f32> {
    let n = 24;
    let bin_hz = SAMPLE_RATE / NUM_SAMPLES as f32;
    (0..n)
        .map(|i| {
            let t = i as f32 / (n - 1) as f32;
            let nominal = 20.0 * (20_000.0f32 / 20.0).powf(t);
            let k = (nominal / bin_hz).round().max(1.0);
            k * bin_hz
        })
        .collect()
}

fn one_cell(cell_type: EqCellType, freq_hz: f32, gain_db: f32, q: f32) -> EqChannelParams {
    let mut params = EqChannelParams::default();
    params.cells[0] = EqCellParams {
        on: true,
        cell_type,
        freq_hz,
        gain_db,
        q,
    };
    params
}

fn cases() -> Vec<(&'static str, EqChannelParams)> {
    vec![
        ("peak_boost", one_cell(EqCellType::Peak, 1000.0, 9.0, 1.0)),
        (
            "low_pass",
            one_cell(EqCellType::LowPass, 1000.0, 0.0, 0.707),
        ),
        (
            "high_pass",
            one_cell(EqCellType::HighPass, 1000.0, 0.0, 0.707),
        ),
        (
            "low_shelf",
            one_cell(EqCellType::LowShelf, 300.0, 6.0, 0.707),
        ),
        (
            "high_shelf",
            one_cell(EqCellType::HighShelf, 5000.0, -6.0, 0.707),
        ),
        (
            "band_pass",
            one_cell(EqCellType::BandPass, 1000.0, 0.0, 2.0),
        ),
        ("notch", one_cell(EqCellType::Notch, 1000.0, 0.0, 4.0)),
        ("multi_cell_with_trim", {
            let mut params = EqChannelParams::default();
            params.cells[0] = EqCellParams {
                on: true,
                cell_type: EqCellType::Peak,
                freq_hz: 500.0,
                gain_db: 6.0,
                q: 1.0,
            };
            params.cells[1] = EqCellParams {
                on: true,
                cell_type: EqCellType::HighShelf,
                freq_hz: 6000.0,
                gain_db: -4.0,
                q: 0.9,
            };
            params.trim_db = 3.0;
            params
        }),
    ]
}

/// `render::goertzel_magnitude` accumulates in `f32`, which is precise
/// enough for the routing-truth-table/frequency-response tests it was
/// built for (short blocks, moderate attenuation) but not for this
/// fixture: `NUM_SAMPLES` is deliberately long (for fine bin resolution
/// at low frequencies, see `probe_frequencies`), and at deep attenuation
/// (a notch's own center, a steep filter's stopband) `f32`'s accumulated
/// rounding error over that many recursive steps was large enough in dB
/// terms to fail `ui/src/eqResponse.test.ts`'s cross-check even after
/// fixing the bin-alignment leakage above -- the same class of problem
/// `docs/ARCHITECTURE.md`'s M4 drift-simulation entry describes for a
/// different long-running accumulator, and the same fix: accumulate in
/// `f64`, only the final small ratio needs to shed precision.
fn goertzel_magnitude_f64(samples: &[f32], freq_hz: f32, sample_rate: f32) -> f64 {
    let n = samples.len() as f64;
    let k = (0.5 + (n * freq_hz as f64) / sample_rate as f64).floor();
    let omega = (2.0 * std::f64::consts::PI / n) * k;
    let coeff = 2.0 * omega.cos();
    let (mut q1, mut q2) = (0.0f64, 0.0f64);
    for &x in samples {
        let q0 = coeff * q1 - q2 + x as f64;
        q2 = q1;
        q1 = q0;
    }
    (q1 * q1 + q2 * q2 - q1 * q2 * coeff).sqrt()
}

fn measure_response(params: &EqChannelParams) -> Vec<serde_json::Value> {
    probe_frequencies()
        .into_iter()
        .map(|freq_hz| {
            let mut eq = ParametricEq::<1>::new(SAMPLE_RATE);
            eq.on = true;
            eq.set_channel_params(0, *params);

            let tone = sine_tone(NUM_SAMPLES, SAMPLE_RATE, freq_hz, 0);
            let tone_ch0: Vec<f32> = tone.iter().map(|f| f[0]).collect();
            let out_ch0: Vec<f32> = tone.iter().map(|f| eq.process_channel(0, f[0])).collect();

            let ref_mag = goertzel_magnitude_f64(&tone_ch0, freq_hz, SAMPLE_RATE);
            let out_mag = goertzel_magnitude_f64(&out_ch0, freq_hz, SAMPLE_RATE);
            let db = 20.0 * (out_mag / ref_mag).log10();
            json!({ "freq_hz": freq_hz, "db": db })
        })
        .collect()
}

fn build_fixture() -> serde_json::Value {
    let cases: Vec<serde_json::Value> = cases()
        .into_iter()
        .map(|(name, params)| {
            json!({
                "name": name,
                "channel": params,
                "points": measure_response(&params),
            })
        })
        .collect();
    json!({ "sample_rate": SAMPLE_RATE, "cases": cases })
}

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/fixtures/eq_response_reference.json")
}

#[test]
#[ignore] // one-shot fixture generator, not part of the regular suite
fn print_fixture_for_hand_review() {
    println!(
        "{}",
        serde_json::to_string_pretty(&build_fixture()).unwrap()
    );
}

#[test]
fn checked_in_fixture_matches_the_current_engine() {
    let fresh = build_fixture();
    let path = fixture_path();
    let checked_in: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())),
    )
    .unwrap();

    // Numeric comparison within tolerance, not exact JSON equality, so
    // float formatting differences don't cause a false failure -- but any
    // real drift in the actual computed response (the point of this
    // test) still fails it, exactly like `testdata/golden/`'s renders.
    let fresh_cases = fresh["cases"].as_array().unwrap();
    let checked_cases = checked_in["cases"].as_array().unwrap();
    assert_eq!(
        fresh_cases.len(),
        checked_cases.len(),
        "case count changed -- regenerate the fixture deliberately"
    );
    for (fresh_case, checked_case) in fresh_cases.iter().zip(checked_cases.iter()) {
        assert_eq!(fresh_case["name"], checked_case["name"]);
        let fresh_points = fresh_case["points"].as_array().unwrap();
        let checked_points = checked_case["points"].as_array().unwrap();
        assert_eq!(fresh_points.len(), checked_points.len());
        for (fp, cp) in fresh_points.iter().zip(checked_points.iter()) {
            let fresh_db = fp["db"].as_f64().unwrap();
            let checked_db = cp["db"].as_f64().unwrap();
            assert!(
                (fresh_db - checked_db).abs() < 0.05,
                "case {:?} freq {}: fresh={fresh_db}dB checked-in={checked_db}dB -- the EQ's \
                 filter math changed without regenerating the fixture",
                fresh_case["name"],
                fp["freq_hz"]
            );
        }
    }
}
