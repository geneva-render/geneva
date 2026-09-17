//! What a file's audio measures, as the file is, for a report before a
//! render and for a `--for` target to tell whether a copy would already
//! be at the level it asks for.

use std::path::Path;

use geneva_audio::{Analysis, Report};
use geneva_timeline::Ratio;

use crate::MediaError;
use crate::codecs::{AudioReader, probe};

/// Measures the audio of `path` at its own rate and channel count (a
/// mono file as mono, anything wider as the stereo it is decoded to).
/// `None` for a file without audio.
///
/// The decoder hands a mono file out as a stereo pair 3 dB down, the
/// constant-power upmix, so the measurement takes that back: what is
/// reported is the file as a player or a platform would measure it.
pub fn measure_audio(path: &Path) -> Result<Option<Report>, MediaError> {
    let info = probe(path)?;
    let Some(audio) = info.audio else {
        return Ok(None);
    };
    let Some(duration) = audio.duration.or(info.duration) else {
        return Ok(None);
    };
    let rate = audio.sample_rate.max(1000);
    let channels = usize::from(audio.channels.clamp(1, 2));
    let mut stream = AudioReader::open(path)?.into_stream(Ratio::ZERO, rate)?;
    let mut analysis = Analysis::new(rate, channels);
    let mut left = (duration.to_f64() * f64::from(rate)).round().max(0.0) as usize;
    while left > 0 {
        let block = stream.read(left.min(rate as usize))?;
        if block.is_empty() {
            break;
        }
        left -= block.len() / 2;
        if channels == 1 {
            let mono: Vec<f32> = block
                .chunks_exact(2)
                .map(|lr| (lr[0] + lr[1]) * std::f32::consts::FRAC_1_SQRT_2)
                .collect();
            analysis.push(&mono);
        } else {
            analysis.push(&block);
        }
    }
    Ok(Some(analysis.report()))
}
