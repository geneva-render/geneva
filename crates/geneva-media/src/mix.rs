//! Audio mixing: turns the audio clips of a composition into one stereo
//! stream.
//!
//! Every clip is decoded, resampled to the output rate, shaped by its gain
//! track and fades, and summed. The mix is a pure function of the
//! composition and the files, like the video.

use std::path::Path;

use geneva_anim::Track;
use geneva_audio::{Hum, HumDetector, Hygiene, Limiter, Meter};
use geneva_timeline::schema::{Loudness, TransitionKind};
use geneva_timeline::{Composition, Ratio, ResolvedLayer, ResolvedSource};

use crate::MediaError;
use crate::codecs::{AudioReader, AudioSettings, AudioStream};

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
#[derive(Debug, Clone)]
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

impl Live {
    /// A voice placed on the output's frame grid, its file not yet open.
    fn fresh(voice: Voice, rate: u32, total_frames: usize) -> Self {
        let frame_at = |t: Ratio| (t.to_f64() * f64::from(rate)).round().max(0.0) as usize;
        let first = frame_at(voice.start);
        let last = (first + frame_at(voice.end - voice.start)).min(total_frames);
        Self {
            voice,
            first,
            last,
            stream: None,
            silent: false,
            opened: false,
        }
    }
}

/// What a loudness target did to the mix.
#[derive(Debug, Clone, PartialEq)]
pub struct LoudnessReport {
    /// The target, in LUFS.
    pub target_lufs: f64,
    /// The true-peak ceiling, in dBTP.
    pub ceiling_dbtp: f64,
    /// The mix as its sources add up (after hygiene, where it is on),
    /// or `None` for silence (nothing above the meter's
    /// absolute gate), which is left as it is.
    pub measured_lufs: Option<f64>,
    /// The gain applied, in dB, before the limiter.
    pub gain_db: f64,
    /// The mix as written, known once the last block is out. The
    /// limiter can leave it a little under the target on peaky material.
    pub result_lufs: Option<f64>,
}

/// What the treatment of the mix measured and did.
#[derive(Debug, Clone, PartialEq)]
pub struct TreatmentReport {
    /// The loudness target, where there was one.
    pub loudness: Option<LoudnessReport>,
    /// Whether the rumble high-pass ran.
    pub high_pass: bool,
    /// The hum found and notched, where hygiene was on and there was
    /// any.
    pub hum: Option<Hum>,
}

/// The treatment of the mix: measured in one or two passes over it,
/// then applied block by block on the way out.
struct Treatment {
    loudness: Option<Loudness>,
    hygiene: bool,
    /// Channels the encoder writes; a mono output is measured as the
    /// downmix the encoder makes.
    channels: usize,
    prepared: bool,
    filters: Option<Hygiene>,
    limiter: Option<Limiter>,
    meter: Meter,
    pending: Vec<f32>,
    exhausted: bool,
    report: TreatmentReport,
}

/// A plain mixer over `template`, for a pass that runs while the
/// treatment itself is borrowed.
fn self_raw_copy(root: &Path, rate: u32, total_frames: usize, template: &[Voice]) -> Mixer {
    Mixer {
        root: root.to_path_buf(),
        rate,
        total_frames,
        position: 0,
        voices: template
            .iter()
            .cloned()
            .map(|voice| Live::fresh(voice, rate, total_frames))
            .collect(),
        template: Vec::new(),
        treatment: None,
    }
}

/// Feeds a stereo block to a meter as the output carries it.
fn meter_push(meter: &mut Meter, block: &[f32], channels: usize) {
    if channels == 1 {
        let mono: Vec<f32> = block
            .chunks_exact(2)
            .map(|lr| f32::midpoint(lr[0], lr[1]))
            .collect();
        meter.push(&mono);
    } else {
        meter.push(block);
    }
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
    /// The voices as placed, kept only for a treatment's passes over
    /// the mix.
    template: Vec<Voice>,
    treatment: Option<Treatment>,
}

impl Mixer {
    /// Prepares the mix of `comp` at `rate`; files are opened as their
    /// voices come up.
    pub fn new(comp: &Composition, root: &Path, rate: u32) -> Self {
        let total_frames = (comp.duration.to_f64() * f64::from(rate)).round() as usize;
        let voices = voices(comp)
            .into_iter()
            .filter(|v| v.end > v.start)
            .map(|voice| Live::fresh(voice, rate, total_frames))
            .collect();
        Self {
            root: root.to_path_buf(),
            rate,
            total_frames,
            position: 0,
            voices,
            template: Vec::new(),
            treatment: None,
        }
    }

    /// The mix for an output as its audio settings describe it: at its
    /// rate, cleaned and brought to its loudness target where they ask
    /// for that. A treatment reads the whole mix once to
    /// measure it before the first block goes out (twice when hum is
    /// found under a loudness target), which decodes every audio
    /// source again each time.
    pub fn for_output(comp: &Composition, root: &Path, audio: &AudioSettings) -> Self {
        let mut mixer = Self::new(comp, root, audio.sample_rate);
        if audio.loudness.is_some() || audio.hygiene {
            let channels = usize::from(audio.channels.clamp(1, 2));
            mixer.template = mixer.voices.iter().map(|l| l.voice.clone()).collect();
            mixer.treatment = Some(Treatment {
                loudness: audio.loudness.clone(),
                hygiene: audio.hygiene,
                channels,
                prepared: false,
                filters: None,
                limiter: None,
                meter: Meter::new(audio.sample_rate, channels),
                pending: Vec::new(),
                exhausted: false,
                report: TreatmentReport {
                    loudness: None,
                    high_pass: audio.hygiene,
                    hum: None,
                },
            });
        }
        mixer
    }

    /// Frames in the whole mix: `round(duration x rate)`.
    #[must_use]
    pub fn total_frames(&self) -> usize {
        self.total_frames
    }

    /// What the treatment measured and did; `None` without one, or
    /// before the first block. The loudness result is filled in after
    /// the last.
    #[must_use]
    pub fn report(&self) -> Option<&TreatmentReport> {
        self.treatment
            .as_ref()
            .filter(|t| t.prepared)
            .map(|t| &t.report)
    }

    /// The same mix from the start, without the treatment.
    fn raw_copy(&self) -> Self {
        Self {
            root: self.root.clone(),
            rate: self.rate,
            total_frames: self.total_frames,
            position: 0,
            voices: self
                .template
                .iter()
                .cloned()
                .map(|voice| Live::fresh(voice, self.rate, self.total_frames))
                .collect(),
            template: Vec::new(),
            treatment: None,
        }
    }

    /// The measuring passes: the mix through the hum detector, the
    /// high-pass and the meter; then, when hum was found under a
    /// loudness target, the notched mix through the meter again, since
    /// the notches take level with them. From those, the filters, the
    /// gain and the limiter the writing pass applies.
    fn prepare(&mut self) -> Result<(), MediaError> {
        let rate = self.rate;
        let mut raw = self.raw_copy();
        let Some(t) = self.treatment.as_mut() else {
            return Ok(());
        };
        let mut high_pass = t.hygiene.then(|| Hygiene::new(rate, 2, None));
        let mut detector = t.hygiene.then(|| HumDetector::new(rate, 2));
        let mut meter = t.loudness.as_ref().map(|_| Meter::new(rate, t.channels));
        let channels = t.channels;
        let mut feed = |block: &mut Vec<f32>| {
            // Hum is looked for before the high-pass, which would take
            // 9 dB off a 60 Hz fundamental first.
            if let Some(d) = detector.as_mut() {
                d.push(block);
            }
            if let Some(hp) = high_pass.as_mut() {
                hp.process(block);
            }
            if let Some(m) = meter.as_mut() {
                meter_push(m, block, channels);
            }
        };
        while let Some(mut block) = raw.raw_block(rate as usize)? {
            feed(&mut block);
        }
        let hum = detector.and_then(|d| d.hum());
        if hum.is_some() && meter.is_some() {
            let mut raw = self_raw_copy(&self.root, rate, self.total_frames, &self.template);
            let mut chain = Hygiene::new(rate, 2, hum.as_ref());
            let mut again = Meter::new(rate, t.channels);
            while let Some(mut block) = raw.raw_block(rate as usize)? {
                chain.process(&mut block);
                meter_push(&mut again, &block, t.channels);
            }
            meter = Some(again);
        }
        t.filters = t.hygiene.then(|| Hygiene::new(rate, 2, hum.as_ref()));
        if let (Some(spec), Some(meter)) = (&t.loudness, meter) {
            let measured = meter.integrated();
            let ceiling = spec.true_peak_dbtp.unwrap_or(-1.0);
            let gain_db = measured.map_or(0.0, |m| spec.target_lufs - m);
            t.limiter = Some(Limiter::new(rate, 2, gain_db, ceiling));
            t.report.loudness = Some(LoudnessReport {
                target_lufs: spec.target_lufs,
                ceiling_dbtp: ceiling,
                measured_lufs: measured,
                gain_db,
                result_lufs: None,
            });
        }
        t.report.hum = hum;
        t.prepared = true;
        Ok(())
    }

    /// The next block of at most `frames` frames, or `None` after the
    /// last one. Without a treatment, samples are summed without
    /// limiting and the encoder clamps to full scale; with one, they are
    /// filtered, gained to the target and the limiter holds the true
    /// peak under its ceiling. The blocks are the same whatever size
    /// they are asked for in.
    pub fn next_block(&mut self, frames: usize) -> Result<Option<Vec<f32>>, MediaError> {
        if self.treatment.is_none() {
            return self.raw_block(frames);
        }
        if self.treatment.as_ref().is_some_and(|t| !t.prepared) {
            self.prepare()?;
        }
        let wanted = frames * 2;
        loop {
            let (enough, exhausted) = self
                .treatment
                .as_ref()
                .map_or((true, true), |t| (t.pending.len() >= wanted, t.exhausted));
            if enough || exhausted {
                break;
            }
            let raw = self.raw_block(frames)?;
            let Some(t) = self.treatment.as_mut() else {
                break;
            };
            let out = match raw {
                Some(mut block) => {
                    if let Some(f) = t.filters.as_mut() {
                        f.process(&mut block);
                    }
                    match t.limiter.as_mut() {
                        Some(l) => l.push(&block),
                        None => block,
                    }
                }
                None => {
                    t.exhausted = true;
                    t.limiter.as_mut().map_or_else(Vec::new, Limiter::finish)
                }
            };
            meter_push(&mut t.meter, &out, t.channels);
            t.pending.extend(out);
            if t.exhausted {
                let result = t.meter.integrated();
                if let Some(report) = t.report.loudness.as_mut() {
                    report.result_lufs = result;
                }
            }
        }
        let Some(t) = self.treatment.as_mut() else {
            return Ok(None);
        };
        if t.pending.is_empty() {
            return Ok(None);
        }
        let n = wanted.min(t.pending.len());
        Ok(Some(t.pending.drain(..n).collect()))
    }

    /// The next block of the plain mix.
    fn raw_block(&mut self, frames: usize) -> Result<Option<Vec<f32>>, MediaError> {
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
