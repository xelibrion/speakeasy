//! Rate conversion from whatever the device offers to what whisper expects.

use rubato::{FftFixedIn, Resampler};
use speakeasy_core::{Error, Result};

/// Resample mono f32 from `from_rate` to `to_rate`.
///
/// Band-limited rather than naive decimation: dropping samples from 48 kHz
/// aliases speech energy back down into the range whisper cares about.
pub fn resample(samples: &[f32], from_rate: u32, to_rate: u32) -> Result<Vec<f32>> {
    const CHUNK: usize = 1024;

    let mut resampler = FftFixedIn::<f32>::new(from_rate as usize, to_rate as usize, CHUNK, 2, 1)
        .map_err(|err| {
        Error::AudioUnavailable(format!("could not build the resampler: {err}"))
    })?;

    let mut out = Vec::with_capacity(samples.len() * to_rate as usize / from_rate as usize + CHUNK);
    let mut cursor = 0;

    while cursor + CHUNK <= samples.len() {
        let frames = resampler
            .process(&[&samples[cursor..cursor + CHUNK]], None)
            .map_err(|err| Error::AudioUnavailable(format!("resampling failed: {err}")))?;
        out.extend_from_slice(&frames[0]);
        cursor += CHUNK;
    }

    // Flush the tail, which is shorter than a chunk.
    if cursor < samples.len() {
        let frames = resampler
            .process_partial(Some(&[&samples[cursor..]]), None)
            .map_err(|err| Error::AudioUnavailable(format!("resampling the tail failed: {err}")))?;
        out.extend_from_slice(&frames[0]);
    }

    Ok(out)
}
