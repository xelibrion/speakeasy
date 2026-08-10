//! Turning captured audio into the twelve bars the status widget draws.
//!
//! Nothing here knows about cpal, devices, or threads — it is a pure function
//! of samples in, bar heights out, which is what makes it testable without a
//! microphone.
//!
//! The window is short on purpose: this drives a display refreshed at animation
//! rate, so it has to describe *now*, not the utterance so far.

use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};
use speakeasy_core::SPECTRUM_BANDS;

/// Samples per analysis window. At 48 kHz this is ~21 ms and 47 Hz per bin —
/// fine enough to separate the bands below, short enough to feel immediate.
///
/// Public because the caller slices this many samples off the tail of the
/// capture buffer before handing them over.
pub const WINDOW: usize = 1024;

/// Band edges span the speech range rather than 0..Nyquist: almost nothing a
/// microphone hears above 8 kHz is speech, and a bar that never moves is worse
/// than no bar.
const BAND_LOW_HZ: f32 = 80.0;
const BAND_HIGH_HZ: f32 = 8_000.0;

/// Bars are scaled in decibels, not amplitude. Speech in one band spans roughly
/// three orders of magnitude, and a linear bar spends nearly all of that range
/// in its bottom fifth — which is what "the bars barely move" looks like.
///
/// The scale is relative to the room's own noise, so this is how far above the
/// background a band has to be to fill its bar.
const BAND_RANGE_DB: f32 = 36.0;

/// Headroom above the noise estimate before a band registers at all. Room tone
/// is not steady, and without this its wobble drives the bars on its own.
const NOISE_MARGIN_DB: f32 = 8.0;

/// Where the noise estimate starts, near a typical quiet room. The tracker
/// leaves it within a moment either way; this only decides what the first few
/// frames of an utterance look like.
const INITIAL_NOISE_DB: f32 = -70.0;

/// Below this a band is silence, not signal. Keeps `log10` away from zero.
const SILENCE_DB: f32 = -120.0;

/// Weight kept on the old noise estimate when a band drops below it. Quiet is
/// evidence of the true floor, so this follows it down within a few frames.
const NOISE_FALL: f32 = 0.9;

/// Weight kept when a band sits above the estimate. Deliberately sluggish —
/// several seconds — so a held vowel is never mistaken for the room.
const NOISE_RISE: f32 = 0.995;

/// Per-band energy of the last moment of audio, as bar heights in 0.0..=1.0.
///
/// Holds the state that only makes sense across successive frames — the noise
/// estimate and the decay — so callers can poll it at whatever rate suits them
/// and get something that reads as an equaliser rather than a flicker.
pub struct Analyzer {
    fft: Arc<dyn RealToComplex<f32>>,
    /// Hann coefficients, precomputed. Without a window, the discontinuity at
    /// the edges of the slice smears energy across every band and the bars all
    /// move together.
    window: Vec<f32>,
    input: Vec<f32>,
    output: Vec<Complex<f32>>,
    /// Per-band estimate of the room's noise, in dB.
    ///
    /// Bars measure the distance above this rather than absolute loudness,
    /// which does two jobs at once: a fan or a room tone sits at the estimate
    /// and reads as nothing, and a quiet microphone still fills the bars,
    /// because what matters is how far the voice stands above the noise, not
    /// what the gain knob was set to.
    noise_db: Vec<f32>,
    /// Last heights returned, for the decay in [`Analyzer::smooth`].
    bands: Vec<f32>,
}

impl Analyzer {
    pub fn new() -> Self {
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(WINDOW);
        let input = fft.make_input_vec();
        let output = fft.make_output_vec();
        let window = (0..WINDOW)
            .map(|i| {
                let phase = std::f32::consts::TAU * i as f32 / (WINDOW - 1) as f32;
                0.5 - 0.5 * phase.cos()
            })
            .collect();

        Self {
            fft,
            window,
            input,
            output,
            noise_db: vec![INITIAL_NOISE_DB; SPECTRUM_BANDS],
            bands: Vec::new(),
        }
    }

    /// Forget the room and the current bar heights.
    ///
    /// Called between utterances: rooms change, and a stale noise floor is
    /// worse than the starting guess.
    pub fn reset(&mut self) {
        self.noise_db = vec![INITIAL_NOISE_DB; SPECTRUM_BANDS];
        self.bands = Vec::new();
    }

    /// Bar heights for the tail of `samples`, each in 0.0..=1.0.
    ///
    /// A tail shorter than the window is zero-padded rather than rejected, so
    /// the bars come alive on the first callback instead of after 21 ms of
    /// nothing.
    pub fn analyze(&mut self, samples: &[f32], rate: u32) -> Vec<f32> {
        if rate == 0 || samples.is_empty() {
            return vec![0.0; SPECTRUM_BANDS];
        }

        let Some(decibels) = self.band_decibels(samples, rate) else {
            return vec![0.0; SPECTRUM_BANDS];
        };

        let next = self.above_the_noise(decibels);
        self.bands = self.smooth(next);
        self.bands.clone()
    }

    /// Energy per band, in dB, for one window of samples.
    fn band_decibels(&mut self, samples: &[f32], rate: u32) -> Option<Vec<f32>> {
        let tail = &samples[samples.len().saturating_sub(WINDOW)..];
        let pad = WINDOW - tail.len();
        self.input[..pad].fill(0.0);
        self.input[pad..].copy_from_slice(tail);
        for (sample, coefficient) in self.input.iter_mut().zip(&self.window) {
            *sample *= coefficient;
        }

        self.fft.process(&mut self.input, &mut self.output).ok()?;

        let hz_per_bin = rate as f32 / WINDOW as f32;
        let ratio = BAND_HIGH_HZ / BAND_LOW_HZ;
        let bin_of = |edge: usize| {
            // Log spacing: even steps here are even steps in pitch, which is
            // what makes the bars look like an equaliser rather than a bump at
            // the bottom and eleven flat ones.
            let hz = BAND_LOW_HZ * ratio.powf(edge as f32 / SPECTRUM_BANDS as f32);
            ((hz / hz_per_bin) as usize).min(self.output.len() - 1)
        };

        Some(
            (0..SPECTRUM_BANDS)
                .map(|band| {
                    let start = bin_of(band);
                    // The lowest bands are narrower than one bin; widening to
                    // at least one keeps them from reading as permanent
                    // silence.
                    let end = bin_of(band + 1).max(start + 1).min(self.output.len());
                    let bins = &self.output[start..end];
                    // RMS, not mean magnitude. Log spacing makes the top band
                    // ~50 bins wide against one bin at the bottom, and
                    // averaging magnitudes over that many mostly-empty bins
                    // buries whatever energy is there — the wide bands end up
                    // pinned flat.
                    let power =
                        bins.iter().map(|bin| bin.norm_sqr()).sum::<f32>() / bins.len() as f32;
                    // `norm` scales with the window length, and the Hann window
                    // costs another factor of two; normalise both out so the
                    // result does not depend on WINDOW.
                    let magnitude = power.sqrt() * 4.0 / WINDOW as f32;
                    if magnitude > 0.0 {
                        20.0 * magnitude.log10()
                    } else {
                        SILENCE_DB
                    }
                })
                .collect(),
        )
    }

    /// Convert absolute band levels into distance above the room's own noise,
    /// updating the noise estimate as it goes.
    fn above_the_noise(&mut self, decibels: Vec<f32>) -> Vec<f32> {
        decibels
            .into_iter()
            .zip(&mut self.noise_db)
            .map(|(db, noise)| {
                // Minimum-statistics tracking: fall toward anything quieter
                // quickly, rise only grudgingly. What survives is the level the
                // band keeps returning to between words, which is the room.
                let kept = if db < *noise { NOISE_FALL } else { NOISE_RISE };
                *noise = *noise * kept + db * (1.0 - kept);
                ((db - *noise - NOISE_MARGIN_DB) / BAND_RANGE_DB).clamp(0.0, 1.0)
            })
            .collect()
    }

    /// Fast to rise, slow to fall — the decay is what makes a spectrum readable
    /// at 60 Hz instead of a blur.
    fn smooth(&self, next: Vec<f32>) -> Vec<f32> {
        next.into_iter()
            .enumerate()
            .map(|(band, value)| {
                let was = self.bands.get(band).copied().unwrap_or(0.0);
                if value > was {
                    value
                } else {
                    was * 0.75 + value * 0.25
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    /// `seconds` of a sine at `hz`, peak amplitude `amplitude`.
    fn tone(hz: f32, amplitude: f32, seconds: f32) -> Vec<f32> {
        let frames = (RATE as f32 * seconds) as usize;
        (0..frames)
            .map(|i| {
                let phase = std::f32::consts::TAU * hz * i as f32 / RATE as f32;
                amplitude * phase.sin()
            })
            .collect()
    }

    /// Which band a frequency lands in, by the same log spacing as `bin_of`.
    fn band_of(hz: f32) -> usize {
        let ratio = BAND_HIGH_HZ / BAND_LOW_HZ;
        ((hz / BAND_LOW_HZ).log10() / ratio.log10() * SPECTRUM_BANDS as f32) as usize
    }

    /// Deterministic broadband hiss, standing in for room tone.
    fn hiss(amplitude: f32, seconds: f32) -> Vec<f32> {
        let frames = (RATE as f32 * seconds) as usize;
        let mut state = 0x2545_F491_4F6C_DD1D_u64;
        (0..frames)
            .map(|_| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1);
                let unit = (state >> 40) as f32 / (1 << 24) as f32 - 0.5;
                amplitude * 2.0 * unit
            })
            .collect()
    }

    fn mix(a: &[f32], b: &[f32]) -> Vec<f32> {
        a.iter().zip(b).map(|(x, y)| x + y).collect()
    }

    /// The UI polls at animation rate, and the noise estimate only means
    /// anything after it has seen a few frames — so tests that care about it
    /// have to run the loop rather than call `analyze` once.
    fn settle(analyzer: &mut Analyzer, samples: &[f32], frames: usize) -> Vec<f32> {
        let mut bands = vec![0.0; SPECTRUM_BANDS];
        for _ in 0..frames {
            bands = analyzer.analyze(samples, RATE);
        }
        bands
    }

    #[test]
    fn a_tone_drives_its_own_band_to_full_height() {
        let mut analyzer = Analyzer::new();
        let bands = analyzer.analyze(&tone(1_000.0, 0.2, 0.5), RATE);
        assert!(
            bands[band_of(1_000.0)] > 0.9,
            "expected the 1 kHz band near full height, got {bands:?}"
        );
    }

    /// A steady fan or air conditioner is exactly what the noise tracker is
    /// for: after a moment of it, the bars should be back down.
    #[test]
    fn steady_background_noise_settles_out_of_the_spectrum() {
        let mut analyzer = Analyzer::new();
        let bands = settle(&mut analyzer, &hiss(0.01, 0.5), 120);
        let loudest = bands.iter().copied().fold(0.0, f32::max);
        assert!(
            loudest < 0.1,
            "background still driving the bars: {bands:?}"
        );
    }

    /// What the bars measure is the voice's distance above the room, not the
    /// gain knob — so the same voice on a mic 20 dB quieter reads the same.
    #[test]
    fn the_same_voice_reads_the_same_at_any_microphone_gain() {
        let height = |gain: f32| {
            let mut analyzer = Analyzer::new();
            let room = hiss(0.004 * gain, 0.5);
            // Let the estimate find the room before the voice arrives.
            settle(&mut analyzer, &room, 120);
            let voice = mix(&tone(1_000.0, 0.08 * gain, 0.5), &room);
            analyzer.analyze(&voice, RATE)[band_of(1_000.0)]
        };

        let (loud, quiet) = (height(1.0), height(0.1));
        assert!(
            (loud - quiet).abs() < 0.05,
            "loud mic {loud} vs quiet mic {quiet}"
        );
        assert!(
            loud > 0.3,
            "a voice over the room should be visible: {loud}"
        );
    }

    #[test]
    fn bands_without_energy_stay_down() {
        let mut analyzer = Analyzer::new();
        let bands = analyzer.analyze(&tone(1_000.0, 0.2, 0.5), RATE);
        let band = band_of(1_000.0);
        let others = bands
            .iter()
            .enumerate()
            .filter(|(i, _)| i.abs_diff(band) > 1)
            .map(|(_, &v)| v)
            .fold(0.0, f32::max);
        assert!(others < 0.35, "bands away from the tone: {bands:?}");
    }

    #[test]
    fn silence_reads_as_silence() {
        let mut analyzer = Analyzer::new();
        let bands = analyzer.analyze(&vec![0.0; 4096], RATE);
        assert!(
            bands.iter().all(|&v| v == 0.0),
            "expected all bands at zero, got {bands:?}"
        );
    }

    /// A new utterance should not inherit the last room's noise floor.
    #[test]
    fn reset_forgets_the_previous_room() {
        let mut analyzer = Analyzer::new();
        settle(&mut analyzer, &hiss(0.02, 0.5), 120);
        analyzer.reset();
        assert!(
            analyzer.noise_db.iter().all(|&db| db == INITIAL_NOISE_DB),
            "noise estimate survived a reset: {:?}",
            analyzer.noise_db
        );
    }
}
