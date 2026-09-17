//! Audio references: a WAV per case, compared on numbers rather than
//! bytes, since float filters come out a little different on every
//! machine.

use serde::{Deserialize, Serialize};

/// Interleaved samples with their layout.
#[derive(Debug, Clone, PartialEq)]
pub struct Samples {
    /// Sample rate in Hz.
    pub rate: u32,
    /// Interleaved channels.
    pub channels: u8,
    /// The samples, `channels` per frame, full scale at 1.0.
    pub data: Vec<f32>,
}

/// What counts as the same audio.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AudioTolerance {
    /// Largest difference between any two samples, at full scale 1.0.
    pub max_sample: f32,
    /// Largest difference in level over the whole track, in dB.
    pub rms_db: f64,
    /// Largest difference in level over any 100 ms window, in dB, so a
    /// short break cannot hide in a good average.
    pub window_db: f64,
    /// How far the measured loudness may be from the reference's, and
    /// from the case's stated loudness if it has one, in LU.
    pub lufs: f64,
}

impl Default for AudioTolerance {
    /// A sixteenth of a bit at 16 bits, a tenth of a decibel, and a
    /// tenth of a loudness unit: rounding, not a change.
    fn default() -> Self {
        Self {
            max_sample: 0.002,
            rms_db: 0.1,
            window_db: 0.5,
            lufs: 0.1,
        }
    }
}

/// The measured difference between two tracks.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AudioComparison {
    /// Whether rate, channels and length matched; nothing else is
    /// meaningful when false.
    pub same_shape: bool,
    /// Largest difference between corresponding samples.
    pub max_sample: f32,
    /// Level difference over the whole track, actual against expected,
    /// in dB.
    pub rms_db: f64,
    /// The largest level difference over any 100 ms window, in dB.
    pub worst_window_db: f64,
    /// Integrated loudness of the actual track, where there is any.
    pub lufs_actual: Option<f64>,
    /// Integrated loudness of the reference, where there is any.
    pub lufs_expected: Option<f64>,
}

impl AudioComparison {
    /// Whether the difference is within `tol`.
    #[must_use]
    pub fn passes(&self, tol: &AudioTolerance) -> bool {
        self.same_shape
            && self.max_sample <= tol.max_sample
            && self.rms_db.abs() <= tol.rms_db
            && self.worst_window_db.abs() <= tol.window_db
            && match (self.lufs_actual, self.lufs_expected) {
                (Some(a), Some(e)) => (a - e).abs() <= tol.lufs,
                (None, None) => true,
                _ => false,
            }
    }

    /// One line for a log.
    #[must_use]
    pub fn summary(&self) -> String {
        if !self.same_shape {
            return "rate, channels or length differ".to_owned();
        }
        let lufs = |l: Option<f64>| l.map_or("silent".to_owned(), |v| format!("{v:.2} LUFS"));
        format!(
            "max sample diff {:.5}, level {:+.3} dB, worst window {:+.3} dB, loudness {} vs {}",
            self.max_sample,
            self.rms_db,
            self.worst_window_db,
            lufs(self.lufs_actual),
            lufs(self.lufs_expected)
        )
    }
}

fn db_ratio(a: f64, b: f64) -> f64 {
    let floor = 1e-12;
    10.0 * ((a.max(floor)) / (b.max(floor))).log10()
}

/// Compares `actual` with `expected` sample by sample, by level over the
/// whole and over 100 ms windows, and by loudness.
#[must_use]
pub fn compare_audio(actual: &Samples, expected: &Samples) -> AudioComparison {
    let same_shape = actual.rate == expected.rate
        && actual.channels == expected.channels
        && actual.data.len() == expected.data.len();
    if !same_shape {
        return AudioComparison {
            same_shape,
            max_sample: f32::INFINITY,
            rms_db: f64::INFINITY,
            worst_window_db: f64::INFINITY,
            lufs_actual: None,
            lufs_expected: None,
        };
    }
    let max_sample = actual
        .data
        .iter()
        .zip(&expected.data)
        .map(|(a, e)| (a - e).abs())
        .fold(0.0f32, f32::max);
    let power = |s: &[f32]| s.iter().map(|v| f64::from(*v) * f64::from(*v)).sum::<f64>();
    let rms_db = db_ratio(power(&actual.data), power(&expected.data));
    let window = (actual.rate as usize / 10).max(1) * usize::from(actual.channels.max(1));
    let worst_window_db = actual
        .data
        .chunks(window)
        .zip(expected.data.chunks(window))
        .map(|(a, e)| db_ratio(power(a), power(e)))
        // A window that is silent in both is no difference; one silent
        // on one side only is caught by the samples.
        .filter(|d| d.is_finite())
        .fold(0.0f64, |m, d| if d.abs() > m.abs() { d } else { m });
    let channels = usize::from(actual.channels.max(1));
    AudioComparison {
        same_shape,
        max_sample,
        rms_db,
        worst_window_db,
        lufs_actual: geneva_audio::integrated(actual.rate, channels, &actual.data),
        lufs_expected: geneva_audio::integrated(expected.rate, channels, &expected.data),
    }
}

/// Encodes samples as a 16-bit PCM WAV, which is what a reference is
/// kept as: small, and readable by anything.
#[must_use]
pub fn to_wav(samples: &Samples) -> Vec<u8> {
    let channels = u16::from(samples.channels.max(1));
    let data_len = (samples.data.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&samples.rate.to_le_bytes());
    out.extend_from_slice(&(samples.rate * u32::from(channels) * 2).to_le_bytes());
    out.extend_from_slice(&(channels * 2).to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in &samples.data {
        let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// Decodes a 16-bit PCM WAV as written by [`to_wav`].
pub fn from_wav(bytes: &[u8]) -> Result<Samples, String> {
    let u16_at = |i: usize| -> Result<u16, String> {
        bytes
            .get(i..i + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .ok_or_else(|| "truncated WAV header".to_owned())
    };
    let u32_at = |i: usize| -> Result<u32, String> {
        bytes
            .get(i..i + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .ok_or_else(|| "truncated WAV header".to_owned())
    };
    if bytes.get(0..4) != Some(b"RIFF") || bytes.get(8..12) != Some(b"WAVE") {
        return Err("not a WAV file".to_owned());
    }
    let mut pos = 12;
    let mut format: Option<(u16, u32)> = None;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let len = u32_at(pos + 4)? as usize;
        let body = pos + 8;
        match id {
            b"fmt " => {
                if u16_at(body)? != 1 || u16_at(body + 14)? != 16 {
                    return Err("only 16-bit PCM WAV is read".to_owned());
                }
                format = Some((u16_at(body + 2)?, u32_at(body + 4)?));
            }
            b"data" => {
                let (channels, rate) = format.ok_or("data before fmt")?;
                let end = (body + len).min(bytes.len());
                let data = bytes[body..end]
                    .chunks_exact(2)
                    .map(|b| f32::from(i16::from_le_bytes([b[0], b[1]])) / 32767.0)
                    .collect();
                return Ok(Samples {
                    rate,
                    channels: u8::try_from(channels).map_err(|_| "too many channels")?,
                    data,
                });
            }
            _ => {}
        }
        pos = body + len + (len & 1);
    }
    Err("no data chunk".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wav_round_trips_to_a_bit() {
        let s = Samples {
            rate: 16_000,
            channels: 2,
            data: (0..1000).map(|i| (i as f32 * 0.01).sin() * 0.9).collect(),
        };
        let back = from_wav(&to_wav(&s)).unwrap();
        assert_eq!(back.rate, 16_000);
        assert_eq!(back.channels, 2);
        assert_eq!(back.data.len(), 1000);
        for (a, b) in back.data.iter().zip(&s.data) {
            assert!((a - b).abs() <= 1.0 / 32767.0);
        }
        let c = compare_audio(&back, &s);
        assert!(c.passes(&AudioTolerance::default()), "{}", c.summary());
    }

    #[test]
    fn a_level_change_is_caught_by_the_window_even_when_short() {
        let mut a = Samples {
            rate: 16_000,
            channels: 1,
            data: (0..32_000).map(|i| (i as f32 * 0.4).sin() * 0.5).collect(),
        };
        let e = a.clone();
        // 100 ms 1 dB down inside two seconds: 0.2 dB over the whole.
        for s in &mut a.data[8_000..9_600] {
            *s *= 0.891;
        }
        let c = compare_audio(&a, &e);
        assert!(c.rms_db.abs() < 0.1, "{}", c.summary());
        assert!(c.worst_window_db.abs() > 0.9, "{}", c.summary());
        assert!(!c.passes(&AudioTolerance::default()));
    }
}
