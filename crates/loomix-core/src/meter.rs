//! Per-channel peak-hold metering (spec 1.3 input meter, 1.5 output
//! meter). Updated inline during `process_block` — no allocation, no lock,
//! just a running max read directly by the caller, since M3 has no
//! separate UI thread yet to hand it across (spec 3.3's triple-buffer
//! crossing applies once one exists, from M4 on).

use crate::{Frame, CHANNELS};

#[derive(Debug, Clone, Copy, Default)]
pub struct Meter {
    peak_hold: [f32; CHANNELS],
}

impl Meter {
    pub fn peak(&self, channel: usize) -> f32 {
        self.peak_hold[channel]
    }

    /// Clears the peak hold back to digital silence.
    pub fn reset(&mut self) {
        self.peak_hold = [0.0; CHANNELS];
    }

    pub(crate) fn observe(&mut self, block: &[Frame]) {
        for frame in block {
            for (held, &sample) in self.peak_hold.iter_mut().zip(frame.iter()) {
                let level = sample.abs();
                if level > *held {
                    *held = level;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_the_highest_absolute_sample_seen() {
        let mut meter = Meter::default();
        let mut a: Frame = [0.0; CHANNELS];
        a[0] = 0.3;
        let mut b: Frame = [0.0; CHANNELS];
        b[0] = -0.8;
        meter.observe(&[a, b]);
        assert_eq!(meter.peak(0), 0.8);
        assert_eq!(meter.peak(1), 0.0);
    }

    #[test]
    fn holds_across_calls_until_reset() {
        let mut meter = Meter::default();
        let mut loud: Frame = [0.0; CHANNELS];
        loud[0] = 0.9;
        let quiet: Frame = [0.0; CHANNELS];
        meter.observe(&[loud]);
        meter.observe(&[quiet]);
        assert_eq!(meter.peak(0), 0.9);
        meter.reset();
        assert_eq!(meter.peak(0), 0.0);
    }
}
