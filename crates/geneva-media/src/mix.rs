//! Audio mixing: turns the audio clips of a composition into one stereo
//! stream.
//!
//! Every clip is decoded, resampled to the output rate, shaped by its gain
//! track and fades, and summed. The mix is a pure function of the
//! composition and the files, like the video.

use std::path::Path;

use geneva_anim::Track;
use geneva_timeline::{Composition, Ratio, ResolvedLayer, ResolvedSource};

use crate::MediaError;
use crate::codecs::AudioReader;

/// Gain is sampled once per block of this many frames.
const GAIN_BLOCK: usize = 64;

/// One thing to mix.
struct Voice {
    asset: String,
    src: String,
    in_: Ratio,
    start: Ratio,
    end: Ratio,
    gain_db: Track<f64>,
    fade_in: Ratio,
    fade_out: Ratio,
    /// How fast the source plays; the pitch follows.
    speed: Ratio,
}

/// Adds the audible video clips of `layers`, recursing into compositions.
/// `offset` and `speed` map the layers' own time to the output: a
/// nested composition shown at speed 2 has its clips run twice as fast.
fn walk(
    comp: &Composition,
    layers: &[ResolvedLayer],
    offset: Ratio,
    speed: Ratio,
    limit: Ratio,
    out: &mut Vec<Voice>,
) {
    for layer in layers {
        for clip in &layer.clips {
            let start = clip.start / speed + offset;
            let end = (clip.end / speed + offset).min(limit);
            if end <= start {
                continue;
            }
            match &clip.source {
                ResolvedSource::Video {
                    asset,
                    in_,
                    audio: true,
                } => {
                    let src = comp
                        .assets
                        .get(asset)
                        .map(|a| a.src.clone())
                        .unwrap_or_default();
                    out.push(Voice {
                        asset: asset.clone(),
                        src,
                        in_: *in_,
                        start,
                        end,
                        gain_db: Track::constant(0.0),
                        fade_in: Ratio::ZERO,
                        fade_out: Ratio::ZERO,
                        speed: clip.speed * speed,
                    });
                }
                ResolvedSource::Composition(nested) => {
                    walk(comp, &nested.layers, start, clip.speed * speed, end, out);
                }
                _ => {}
            }
        }
    }
}

/// Collects every audible clip: audio tracks, plus video clips whose audio
/// is enabled, including those inside nested compositions.
fn voices(comp: &Composition) -> Vec<Voice> {
    let mut out = Vec::new();
    for track in &comp.audio {
        for c in &track.clips {
            let src = comp
                .assets
                .get(&c.asset)
                .map(|a| a.src.clone())
                .unwrap_or_default();
            out.push(Voice {
                asset: c.asset.clone(),
                src,
                in_: c.in_,
                start: c.start,
                end: c.end,
                gain_db: c.gain_db.clone(),
                fade_in: c.fade_in,
                fade_out: c.fade_out,
                speed: c.speed,
            });
        }
    }
    walk(
        comp,
        &comp.layers,
        Ratio::ZERO,
        Ratio::ONE,
        comp.duration,
        &mut out,
    );
    out
}

/// Mixes the whole composition into interleaved stereo `f32` at `rate`.
///
/// The result has exactly `round(duration × rate)` frames. Samples are
/// summed without limiting; the encoder clamps to full scale.
pub fn mix(comp: &Composition, root: &Path, rate: u32) -> Result<Vec<f32>, MediaError> {
    let total_frames = (comp.duration.to_f64() * f64::from(rate)).round() as usize;
    let mut out = vec![0f32; total_frames * 2];
    let mut readers: std::collections::HashMap<String, AudioReader> =
        std::collections::HashMap::new();
    for voice in voices(comp) {
        let length = voice.end - voice.start;
        if length <= Ratio::ZERO {
            continue;
        }
        let reader = match readers.entry(voice.asset.clone()) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(e) => {
                match AudioReader::open(&root.join(&voice.src)) {
                    Ok(r) => e.insert(r),
                    // A video without an audio stream simply contributes silence.
                    Err(MediaError::NoStream { .. }) => continue,
                    Err(err) => return Err(err),
                }
            }
        };
        // A sped-up source: that much more of it, resampled to that
        // much lower a rate, then played at the output rate (the pitch
        // follows, as on a varispeed deck).
        let samples = if voice.speed == Ratio::ONE {
            reader.read(voice.in_, length, rate)?
        } else {
            let read_rate = (f64::from(rate) / voice.speed.to_f64()).round().max(1000.0) as u32;
            let mut s = reader.read(voice.in_, length * voice.speed, read_rate)?;
            s.resize(
                ((length.to_f64() * f64::from(rate)).round() as usize) * 2,
                0.0,
            );
            s
        };
        let offset = (voice.start.to_f64() * f64::from(rate)).round() as usize;
        let frames = samples.len() / 2;
        let fade_in = voice.fade_in.to_f64();
        let fade_out = voice.fade_out.to_f64();
        let len_secs = length.to_f64();
        let mut block_gain = 1.0f32;
        for i in 0..frames {
            if i % GAIN_BLOCK == 0 {
                let t = i as f64 / f64::from(rate);
                let db = voice.gain_db.sample(t);
                let mut g = 10f64.powf(db / 20.0);
                if fade_in > 0.0 && t < fade_in {
                    g *= t / fade_in;
                }
                if fade_out > 0.0 && t > len_secs - fade_out {
                    g *= ((len_secs - t) / fade_out).max(0.0);
                }
                block_gain = g as f32;
            }
            let dst = offset + i;
            if dst >= total_frames {
                break;
            }
            out[dst * 2] += samples[i * 2] * block_gain;
            out[dst * 2 + 1] += samples[i * 2 + 1] * block_gain;
        }
    }
    Ok(out)
}
