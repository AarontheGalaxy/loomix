//! Manual soak harness (spec 3.4 M4's acceptance criterion: "a 30 minute
//! soak test on two devices with different clocks shows no dropouts and
//! bounded drift"). Everything this milestone actually built --
//! enumeration, clock master selection, drift correction, the resampler,
//! the ring-buffer wiring in `loomix-app::engine_io` -- gets exercised
//! together here, against real hardware, for real. Nothing here is
//! itself unit-tested (there's no synthetic stand-in for "does this
//! actually work end to end on a real machine", which is the entire
//! point of running it), so keep it thin: wire two devices, poll the
//! monitoring handles those crates already expose, print a verdict.
//!
//! Two *output* devices, not an output and an input: spec 1.19's own
//! example of what needs drift correction is exactly this ("outputs A1
//! through A5 are not sample synchronous with each other when they run on
//! different physical devices"), and unlike a capture device, an output
//! device needs no macOS microphone permission for this process to use it
//! -- confirmed the hard way, a first version defaulting to the system
//! input device measured 100% underrun (every sample) against a real
//! microphone from an unapproved terminal, which is a TCC permission gate
//! on this specific machine and process, not a bug in the wiring.
//!
//! `nightly.yml`'s `soak` job already looks for this package by name and
//! runs it with `--duration 2h`; that leg is M9's (with the recorder
//! folded in), not exercised by this binary's current two-device-only
//! shape.
#![forbid(unsafe_code)]

use loomix_app::engine_io::{
    attach_master_device, attach_render_device, select_clock_master, EngineIoDriver,
};
use loomix_core::Engine;
use loomix_hal::clock::{ClockSource, DeviceId};
use loomix_hal::device::{
    channel_count, default_output_device, device_name, device_uid, list_device_ids, CoreAudioError,
    Direction,
};
use loomix_hal::drift::{DriftCorrector, PiController};
use loomix_hal::master_clock::MasterClock;
use std::sync::Arc;
use std::time::{Duration, Instant};

const RING_CAPACITY: usize = 1 << 16;
const MAX_BLOCK_FRAMES: usize = 2048; // spec 1.11's upper buffer-size bound
const BUS_B: usize = 1; // the second device's bus (A2); MASTER_BUS (A1) is always 0

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|a| a == "--list-devices") {
        list_devices();
        return;
    }

    let mut duration = Duration::from_secs(30 * 60);
    let mut device_a_uid: Option<String> = None;
    let mut device_b_uid: Option<String> = None;
    let mut allow_same_device = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--duration" => {
                i += 1;
                duration = parse_duration(args.get(i).unwrap_or_else(|| {
                    eprintln!("--duration needs a value, e.g. 30m, 1800s, 2h");
                    std::process::exit(2);
                }));
            }
            "--device-a" => {
                i += 1;
                device_a_uid = Some(args.get(i).cloned().unwrap_or_else(|| {
                    eprintln!("--device-a needs a device UID (see --list-devices)");
                    std::process::exit(2);
                }));
            }
            "--device-b" => {
                i += 1;
                device_b_uid = Some(args.get(i).cloned().unwrap_or_else(|| {
                    eprintln!("--device-b needs a device UID (see --list-devices)");
                    std::process::exit(2);
                }));
            }
            "--allow-same-device" => allow_same_device = true,
            other => {
                eprintln!("unrecognised argument: {other}");
                print_usage();
                std::process::exit(2);
            }
        }
        i += 1;
    }

    if let Err(e) = run(duration, device_a_uid, device_b_uid, allow_same_device) {
        eprintln!("soak failed to start: CoreAudio error {e}");
        std::process::exit(1);
    }
}

fn print_usage() {
    eprintln!(
        "usage: loomix-soak [--duration 30m] [--device-a UID] [--device-b UID] \
         [--allow-same-device] [--list-devices]"
    );
}

fn list_devices() {
    let ids = match list_device_ids() {
        Ok(ids) => ids,
        Err(e) => {
            eprintln!("failed to enumerate devices: CoreAudio error {e}");
            std::process::exit(1);
        }
    };
    println!(
        "{:<8} {:<38} {:>3} in  {:>3} out  name",
        "id", "uid", "", ""
    );
    for id in ids {
        let uid = device_uid(id).unwrap_or_default();
        let name = device_name(id).unwrap_or_default();
        let inputs = channel_count(id, Direction::Input).unwrap_or(0);
        let outputs = channel_count(id, Direction::Output).unwrap_or(0);
        println!("{id:<8} {uid:<38} {inputs:>3}      {outputs:>3}      {name}");
    }
}

fn resolve_by_uid(uid: &str) -> Result<DeviceId, CoreAudioError> {
    for id in list_device_ids()? {
        if device_uid(id).map(|u| u == uid).unwrap_or(false) {
            return Ok(id);
        }
    }
    eprintln!("no device with UID '{uid}' found (see --list-devices)");
    std::process::exit(2);
}

/// The first output-capable device found that isn't `exclude` -- the
/// default for device B when the caller didn't name one, so a bare
/// `loomix-soak` has a real shot at working out of the box on a machine
/// with more than one output device (BlackHole, an installed Loomix
/// driver, an external interface, ...), while still printing exactly what
/// it picked so a misleading default is easy to spot and override.
fn first_other_output_device(exclude: DeviceId) -> Result<DeviceId, CoreAudioError> {
    for id in list_device_ids()? {
        if id == exclude {
            continue;
        }
        if channel_count(id, Direction::Output).unwrap_or(0) > 0 {
            return Ok(id);
        }
    }
    eprintln!(
        "only one output device is available on this machine; pass --device-b to name a \
         second one explicitly (see --list-devices)"
    );
    std::process::exit(2);
}

fn run(
    duration: Duration,
    device_a_uid: Option<String>,
    device_b_uid: Option<String>,
    allow_same_device: bool,
) -> Result<(), CoreAudioError> {
    let device_a = match device_a_uid {
        Some(uid) => resolve_by_uid(&uid)?,
        None => default_output_device()?,
    };
    let device_b = match device_b_uid {
        Some(uid) => resolve_by_uid(&uid)?,
        None => first_other_output_device(device_a)?,
    };

    let a_name = device_name(device_a)?;
    let a_uid = device_uid(device_a)?;
    let b_name = device_name(device_b)?;
    let b_uid = device_uid(device_b)?;

    if a_uid == b_uid && !allow_same_device {
        eprintln!(
            "device A and device B resolved to the same device ({a_name}, {a_uid}) -- that has \
             one clock, not two, so it can't demonstrate drift correction at all. Pass \
             --device-a/--device-b to pick two different devices, or --allow-same-device if you \
             really mean it."
        );
        std::process::exit(2);
    }

    match select_clock_master(Some(device_a))? {
        ClockSource::Device(id) if id == device_a => {}
        other => {
            eprintln!(
                "expected {a_name} ({a_uid}) to resolve as clock master, got {other:?} -- is \
                 the device still connected?"
            );
            std::process::exit(2);
        }
    }

    let a_channels = channel_count(device_a, Direction::Output)?;
    if a_channels == 0 {
        eprintln!("{a_name} ({a_uid}) has no output channels");
        std::process::exit(2);
    }
    let b_channels = channel_count(device_b, Direction::Output)?;
    if b_channels == 0 {
        eprintln!("{b_name} ({b_uid}) has no output channels");
        std::process::exit(2);
    }

    println!("device A (master, bus A1): {a_name}  [{a_uid}]  {a_channels}ch");
    println!("device B (drift-corrected, bus A2): {b_name}  [{b_uid}]  {b_channels}ch");
    println!("duration: {duration:?}");
    println!("both buses carry silence -- this measures timing and dropouts, not audio content");

    let master_clock = Arc::new(MasterClock::default());
    let mut driver =
        EngineIoDriver::new(Engine::new(), master_clock.clone(), None, MAX_BLOCK_FRAMES);

    let corrector = DriftCorrector::new(PiController::new(2e-5, 5e-7, 0.01), 500.0);
    let render_b = attach_render_device(
        &mut driver,
        BUS_B,
        device_b,
        b_channels,
        master_clock.clone(),
        corrector,
        RING_CAPACITY,
    )?;

    // attach_master_device takes `driver` by value and starts it running
    // immediately -- everything above must be wired first.
    let _master = attach_master_device(device_a, driver)?;

    println!("running -- one line every 10s: elapsed, drift ratio, dropouts");
    let start = Instant::now();
    let mut max_ratio_deviation = 0.0_f32;
    let poll_interval = Duration::from_secs(10);
    let mut next_poll = start + poll_interval;

    while start.elapsed() < duration {
        std::thread::sleep(Duration::from_millis(200));
        if Instant::now() < next_poll {
            continue;
        }
        next_poll += poll_interval;
        let ratio = render_b.ratio.get();
        max_ratio_deviation = max_ratio_deviation.max((ratio - 1.0).abs());
        println!(
            "  {:>6.0}s  ratio={:.6}  dropouts={}",
            start.elapsed().as_secs_f32(),
            ratio,
            render_b.dropouts.get()
        );
    }

    let dropouts = render_b.dropouts.get();
    let saturated = max_ratio_deviation >= 0.01; // matches the corrector's own max_correction above
    println!();
    println!("elapsed: {:?}", start.elapsed());
    println!("dropouts: {dropouts}");
    println!("max drift ratio deviation from 1.0: {max_ratio_deviation:.6}");
    if dropouts == 0 && !saturated {
        println!("PASS: no dropouts, drift stayed bounded (never saturated the corrector)");
    } else {
        println!(
            "FAIL: {}{}",
            if dropouts > 0 {
                format!("{dropouts} dropout(s) observed. ")
            } else {
                String::new()
            },
            if saturated {
                "drift correction saturated its clamp at least once."
            } else {
                ""
            }
        );
        std::process::exit(1);
    }
    Ok(())
}

fn parse_duration(s: &str) -> Duration {
    let (num, unit) = s.split_at(s.find(|c: char| c.is_alphabetic()).unwrap_or(s.len()));
    let value: f64 = num.parse().unwrap_or_else(|_| {
        eprintln!("bad duration '{s}', expected e.g. 30m, 1800s, 2h, 0.5h");
        std::process::exit(2);
    });
    let seconds = match unit {
        "" | "s" => value,
        "m" => value * 60.0,
        "h" => value * 3600.0,
        other => {
            eprintln!("unknown duration unit '{other}', expected s, m or h");
            std::process::exit(2);
        }
    };
    Duration::from_secs_f64(seconds)
}
