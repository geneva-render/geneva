//! What a file's audio measures: its integrated loudness and true peak,
//! as the file is, so a `--for` target can tell whether a copy would
//! already be at the level it asks for.

use std::path::Path;

use geneva_audio::{Meter, TruePeak, to_db};
use geneva_timeline::Ratio;

use crate::MediaError;
use crate::codecs::{AudioReader, probe};

/// The levels of a file's audio.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AudioLevels {
    /// Integrated loudness in LUFS, or `None` for silence.
    pub lufs: Option<f64>,
    /// True peak in dBTP.
    pub true_peak_dbtp: f64,
}

/// Measures the audio of `path` at its own rate and channel count (a
/// mono file as mono, anything wider as the stereo it is decoded to).
/// `None` for a file without audio.
pub fn measure_audio(path: &Path) -> Result<Option<AudioLevels>, MediaError> {
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
    let mut meter = Meter::new(rate, channels);
    let mut peak = TruePeak::new(rate, channels);
    let mut highest = 0f32;
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
                .map(|lr| (lr[0] + lr[1]) * 0.5)
                .collect();
            meter.push(&mono);
            for s in &mono {
                highest = highest.max(peak.push(&[*s]));
            }
        } else {
            meter.push(&block);
            for frame in block.chunks_exact(2) {
                highest = highest.max(peak.push(frame));
            }
        }
    }
    Ok(Some(AudioLevels {
        lufs: meter.integrated(),
        true_peak_dbtp: to_db(f64::from(highest)),
    }))
}
