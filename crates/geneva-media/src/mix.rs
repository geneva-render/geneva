//! Audio mixing: turns the audio clips of a composition into one stereo
//! stream.
//!
//! Every clip is decoded, resampled to the output rate, shaped by its gain
//! track and fades, and summed. The mix is a pure function of the
//! composition and the files, like the video.

use std::path::Path;

use geneva_anim::Track;
use geneva_timeline::schema::TransitionKind;
use geneva_timeline::{Composition, Ratio, ResolvedLayer, ResolvedSource};

use crate::MediaError;
use crate::codecs::{AudioReader, AudioStream};

/// Gain is sampled once per block of this many frames.
const GAIN_BLOCK: usize = 64;

/// The shape of a voice's fades.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FadeShape {
    /// Gain moves with time. Right for a fade to or from silence, where
    /// the ear follows the amplitude down to nothing.
    Linear,
    /// Gain is the square root of the linear ramp, so two voices crossing
    /// with opposite ramps sum to constant power. Right for a crossfade,
    /// where a linear pair would dip about 3 dB in the middle on material
    /// that is not correlated.
    EqualPower,
    /// Silent for half the ramp and linear over the other half. Right for
    /// a `fade`, where the two clips never sound together: one reaches
    /// silence before the other leaves it.
    Dip,
}

/// One thing to mix.
struct Voice {
    src: String,
    in_: Ratio,
    start: Ratio,
    end: Ratio,
    gain_db: Track<f64>,
    fade_in: Ratio,
    fade_out: Ratio,
    shape: FadeShape,
    /// How fast the source plays; the pitch follows.
    speed: Ratio,
}

impl FadeShape {
    /// Maps a ramp that runs 0 to 1 onto the gain to apply.
    fn gain(self, ramp: f64) -> f64 {
        let ramp = ramp.clamp(0.0, 1.0);
        match self {
            Self::Linear => ramp,
            Self::EqualPower => ramp.sqrt(),
            Self::Dip => ((ramp - 0.5) * 2.0).clamp(0.0, 1.0),
        }
    }
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
        // Where the previous clip of this layer put its voice, so that a
        // transition can fade it out while the next one fades in. A clip
        // that makes no sound leaves this empty, and the clip after it
        // has nothing to cross with.
        let mut previous: Option<usize> = None;
        for clip in &layer.clips {
            let start = clip.start / speed + offset;
            let end = (clip.end / speed + offset).min(limit);
            if end <= start {
                previous = None;
                continue;
            }
            // A transition overlaps this clip with the one before it, so
            // the pair crosses: this one up, that one down, over the same
            // stretch.
            let (cross, shape) = match &clip.transition_in {
                Some(tr) if tr.duration > Ratio::ZERO => (
                    tr.duration / speed,
                    match tr.kind {
                        TransitionKind::Crossfade => FadeShape::EqualPower,
                        TransitionKind::Fade => FadeShape::Dip,
                    },
                ),
                _ => (Ratio::ZERO, FadeShape::Linear),
            };
            if cross > Ratio::ZERO {
                if let Some(prev) = previous {
                    out[prev].fade_out = cross;
                    out[prev].shape = shape;
                }
            }
            let mut voiced = None;
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
                        src,
                        in_: *in_,
                        start,
                        end,
                        gain_db: Track::constant(0.0),
                        fade_in: cross,
                        fade_out: Ratio::ZERO,
                        shape,
                        speed: clip.speed * speed,
                    });
                    // A clip that closes the layer takes its own sound
                    // down; a pair is handled by the clip after it.
                    if let Some(tr) = &clip.transition_out {
                        if tr.duration > Ratio::ZERO {
                            let last = out.last_mut().expect("just pushed");
                            // Either kind ends in silence here, with no
                            // second voice to cross against, so following
                            // the amplitude is what the ear expects.
                            last.fade_out = tr.duration / speed;
                            last.shape = FadeShape::Linear;
                        }
                    }
                    voiced = Some(out.len() - 1);
                }
                ResolvedSource::Composition(nested) => {
                    walk(comp, &nested.layers, start, clip.speed * speed, end, out);
                }
                _ => {}
            }
            previous = voiced;
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
                src,
                in_: c.in_,
                start: c.start,
                end: c.end,
                gain_db: c.gain_db.clone(),
                fade_in: c.fade_in,
                fade_out: c.fade_out,
                shape: FadeShape::Linear,
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

/// One voice while it is being mixed.
struct Live {
    voice: Voice,
    /// First and one-past-last output frame of the voice.
    first: usize,
    last: usize,
    /// Its stream, opened on first use; `None` again once it is done.
    stream: Option<AudioStream>,
    /// True when the file has no audio stream: the voice is silence.
    silent: bool,
    opened: bool,
}

/// Mixes a composition block by block, so that only a block of the output
/// is ever held: every voice is read forward as the blocks advance. The
/// mix is a pure function of the composition and the files, like the
/// video, whatever the block size.
pub struct Mixer {
    root: std::path::PathBuf,
    rate: u32,
    total_frames: usize,
    position: usize,
    voices: Vec<Live>,
}

impl Mixer {
    /// Prepares the mix of `comp` at `rate`; files are opened as their
    /// voices come up.
    pub fn new(comp: &Composition, root: &Path, rate: u32) -> Self {
        let total_frames = (comp.duration.to_f64() * f64::from(rate)).round() as usize;
        let frame_at = |t: Ratio| (t.to_f64() * f64::from(rate)).round().max(0.0) as usize;
        let voices = voices(comp)
            .into_iter()
            .filter(|v| v.end > v.start)
            .map(|voice| {
                let first = frame_at(voice.start);
                let last = (first + frame_at(voice.end - voice.start)).min(total_frames);
                Live {
                    voice,
                    first,
                    last,
                    stream: None,
                    silent: false,
                    opened: false,
                }
            })
            .collect();
        Self {
            root: root.to_path_buf(),
            rate,
            total_frames,
            position: 0,
            voices,
        }
    }

    /// Frames in the whole mix: `round(duration × rate)`.
    pub fn total_frames(&self) -> usize {
        self.total_frames
    }

    /// The next block of at most `frames` frames, or `None` after the
    /// last one. Samples are summed without limiting; the encoder clamps
    /// to full scale.
    pub fn next_block(&mut self, frames: usize) -> Result<Option<Vec<f32>>, MediaError> {
        if self.position >= self.total_frames || frames == 0 {
            return Ok(None);
        }
        let b0 = self.position;
        let b1 = (b0 + frames).min(self.total_frames);
        let mut out = vec![0f32; (b1 - b0) * 2];
        let rate = self.rate;
        for live in &mut self.voices {
            if live.last <= b0 {
                live.stream = None;
                continue;
            }
            if live.first >= b1 || live.silent {
                continue;
            }
            let o0 = b0.max(live.first);
            let o1 = b1.min(live.last);
            let offset = o0 - live.first;
            let count = o1 - o0;
            if !live.opened {
                live.opened = true;
                // A sped-up source is read at a proportionally lower rate
                // and played at the output rate: the pitch follows, as on
                // a varispeed deck.
                let read_rate = if live.voice.speed == Ratio::ONE {
                    rate
                } else {
                    (f64::from(rate) / live.voice.speed.to_f64())
                        .round()
                        .max(1000.0) as u32
                };
                match AudioReader::open(&self.root.join(&live.voice.src)) {
                    Ok(r) => {
                        let mut stream = r.into_stream(live.voice.in_, read_rate)?;
                        // Blocks are sequential, so a voice is first met at
                        // its start; anything before is passed over.
                        if offset > 0 {
                            stream.read(offset)?;
                        }
                        live.stream = Some(stream);
                    }
                    // A video without an audio stream contributes silence.
                    Err(MediaError::NoStream { .. }) => {
                        live.silent = true;
                        continue;
                    }
                    Err(err) => return Err(err),
                }
            }
            let Some(stream) = live.stream.as_mut() else {
                continue;
            };
            let samples = stream.read(count)?;
            let voice = &live.voice;
            let fade_in = voice.fade_in.to_f64();
            let fade_out = voice.fade_out.to_f64();
            let len_secs = (voice.end - voice.start).to_f64();
            let mut block_gain = 1.0f32;
            for i in 0..count {
                let k = offset + i;
                // Gain is sampled once per block of frames, always at the
                // block's own start, so the mix does not depend on where
                // the output blocks fall.
                if k % GAIN_BLOCK == 0 || i == 0 {
                    let t = (k - k % GAIN_BLOCK) as f64 / f64::from(rate);
                    let db = voice.gain_db.sample(t);
                    let mut g = 10f64.powf(db / 20.0);
                    if fade_in > 0.0 && t < fade_in {
                        g *= voice.shape.gain(t / fade_in);
                    }
                    if fade_out > 0.0 && t > len_secs - fade_out {
                        g *= voice.shape.gain((len_secs - t) / fade_out);
                    }
                    block_gain = g as f32;
                }
                let dst = (o0 - b0 + i) * 2;
                out[dst] += samples[i * 2] * block_gain;
                out[dst + 1] += samples[i * 2 + 1] * block_gain;
            }
        }
        self.position = b1;
        Ok(Some(out))
    }
}

/// Mixes the whole composition into interleaved stereo `f32` at `rate`,
/// a second at a time through [`Mixer`].
///
/// The result has exactly `round(duration × rate)` frames.
pub fn mix(comp: &Composition, root: &Path, rate: u32) -> Result<Vec<f32>, MediaError> {
    let mut mixer = Mixer::new(comp, root, rate);
    let mut out = Vec::with_capacity(mixer.total_frames() * 2);
    while let Some(block) = mixer.next_block(rate as usize)? {
        out.extend_from_slice(&block);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_equal_power_pair_sums_to_constant_power() {
        // Two voices crossing with opposite ramps. Their gains squared
        // add to one at every point, so uncorrelated material keeps its
        // level across the overlap instead of rising in the middle.
        for i in 0..=100 {
            let x = f64::from(i) / 100.0;
            let up = FadeShape::EqualPower.gain(x);
            let down = FadeShape::EqualPower.gain(1.0 - x);
            assert!(
                (up * up + down * down - 1.0).abs() < 1e-12,
                "at {x}: {up}^2 + {down}^2 is not 1"
            );
        }
    }

    #[test]
    fn a_linear_fade_still_moves_with_time() {
        // Explicit fades on an audio clip go to silence, where following
        // the amplitude is what the ear expects.
        assert!((FadeShape::Linear.gain(0.5) - 0.5).abs() < 1e-12);
        assert_eq!(FadeShape::Linear.gain(0.0), 0.0);
        assert_eq!(FadeShape::Linear.gain(1.0), 1.0);
        // Out of range on either side is clamped, not extrapolated.
        assert_eq!(FadeShape::EqualPower.gain(-0.5), 0.0);
        assert_eq!(FadeShape::EqualPower.gain(2.0), 1.0);
    }
}
