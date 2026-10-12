//! Chunked encoding: the output cut into stretches encoded at the same
//! time, each encoder on a share of the cores, joined afterwards by
//! stream copy. It fills the cores an encoder leaves idle, so the gain is
//! large for encoders that barely scale (DNxHD, OpenH264) and, on big
//! machines, for those that stop scaling before the core count (x264,
//! VP9, AV1). Hardware encoders use no cores; what chunking parallelizes
//! for them is the decoding and compositing in front of them.

use std::ops::Range;

use geneva_timeline::schema::{AutoChunks, Chunks, HardwarePolicy, VideoCodec};
use geneva_timeline::{Composition, Ratio};

/// The shortest stretch worth its own encoder, in seconds.
const MIN_CHUNK_SECS: f64 = 2.0;

/// The most stretches encoded at once.
const MAX_CHUNKS: u32 = 16;

/// How many cores one pipeline with this encoder keeps busy before more
/// stop helping: the point past which chunking pays. `u32::MAX` means
/// `auto` never chunks for it. Only two encoders have been measured to
/// gain: VP9 (a fifth from two stretches on four cores) and OpenH264 (a
/// tenth). AV1 gained nothing; x264 and DNxHD lost on four cores, since
/// one pipeline already fills the machine; and a hardware encoder lost
/// on an eight-core M1 (VideoToolbox is the bottleneck, and two sessions
/// contend for it). The rest stay off until measured otherwise; a count
/// in `chunks` forces them.
pub fn scaling_ceiling(codec: VideoCodec, hardware: HardwarePolicy) -> u32 {
    let hardware_possible = hardware != HardwarePolicy::Never && cfg!(target_os = "macos");
    #[cfg(feature = "media")]
    let has_x264 = crate::system_x264().is_some();
    #[cfg(not(feature = "media"))]
    let has_x264 = false;
    match codec {
        VideoCodec::Vp9 => 2,
        VideoCodec::H264 if !hardware_possible && !has_x264 => 2,
        VideoCodec::H264
        | VideoCodec::H265
        | VideoCodec::Av1
        | VideoCodec::Dnxhd
        | VideoCodec::Prores
        | VideoCodec::Mjpeg
        | VideoCodec::Png => u32::MAX,
    }
}

/// The stretches to encode, in output frames, and the cores for each.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkPlan {
    /// Frame ranges, in order, covering the whole output.
    pub ranges: Vec<Range<u64>>,
    /// Encoder threads for each stretch.
    pub threads_each: u32,
}

impl ChunkPlan {
    /// One run over everything: no chunking.
    pub fn single(frames: u64) -> Self {
        Self {
            ranges: std::iter::once(0..frames).collect(),
            threads_each: 0,
        }
    }

    /// Whether the plan encodes in more than one stretch.
    pub fn is_chunked(&self) -> bool {
        self.ranges.len() > 1
    }
}

/// Plans the stretches for `comp` under `request` on a machine with
/// `cores`. Boundaries fall on the composition's own cuts (clip starts)
/// when one is near, else on the keyframe grid when there is one, so a
/// stretch starts where a keyframe would be anyway.
pub fn plan_chunks(
    comp: &Composition,
    codec: VideoCodec,
    hardware: HardwarePolicy,
    keyframe_interval: Option<f64>,
    request: Option<Chunks>,
    cores: u32,
) -> ChunkPlan {
    let frames = comp.frame_count();
    let fps = comp.fps.to_f64();
    let min_frames = (MIN_CHUNK_SECS * fps).ceil().max(1.0) as u64;
    let most = (frames / min_frames).min(u64::from(MAX_CHUNKS)) as u32;
    let count = match request {
        Some(Chunks::Count(n)) => n.max(1),
        Some(Chunks::Auto(AutoChunks::Auto)) | None => {
            let ceiling = scaling_ceiling(codec, hardware);
            if ceiling == u32::MAX {
                1
            } else {
                (cores / ceiling).max(1)
            }
        }
    };
    let count = count.min(most.max(1));
    let ranges = split_frames(comp, keyframe_interval, count);
    if ranges.len() <= 1 {
        return ChunkPlan::single(frames);
    }
    ChunkPlan {
        threads_each: (cores / ranges.len() as u32).max(1),
        ranges,
    }
}

/// Cuts the output of `comp` into `count` stretches of about equal
/// length, in output frames, each at least [`MIN_CHUNK_SECS`] long, so
/// fewer than `count` when the output is short. Boundaries fall on the
/// composition's own cuts (clip starts) when one is within a quarter of
/// a stretch, else on the keyframe grid when there is one, else where
/// the even split puts them. With no cap on the count, this is also the
/// plan for rendering the stretches on separate machines.
pub fn split_frames(
    comp: &Composition,
    keyframe_interval: Option<f64>,
    count: u32,
) -> Vec<Range<u64>> {
    let frames = comp.frame_count();
    let fps = comp.fps.to_f64();
    let min_frames = (MIN_CHUNK_SECS * fps).ceil().max(1.0) as u64;
    let count = u64::from(count.max(1)).min((frames / min_frames).max(1)) as u32;
    if count <= 1 || frames == 0 {
        return std::iter::once(0..frames).collect();
    }
    // Where a boundary may fall for free: clip starts, and the keyframe
    // grid.
    let frame_at = |t: Ratio| (t * comp.fps).round().max(0) as u64;
    let mut cuts: Vec<u64> = comp
        .layers
        .iter()
        .flat_map(|l| l.clips.iter().map(|c| frame_at(c.start)))
        .filter(|&f| f > 0 && f < frames)
        .collect();
    if let Some(secs) = keyframe_interval {
        let step = (secs * fps).round().max(1.0) as u64;
        cuts.extend((1..).map(|k| k * step).take_while(|&f| f < frames));
    }
    cuts.sort_unstable();
    cuts.dedup();
    let length = frames as f64 / f64::from(count);
    let slack = (length * 0.25) as u64;
    let mut bounds = vec![0u64];
    for k in 1..count {
        let ideal = (f64::from(k) * length).round() as u64;
        let near = cuts
            .iter()
            .copied()
            .filter(|&c| c.abs_diff(ideal) <= slack)
            .min_by_key(|&c| c.abs_diff(ideal));
        let at = near.unwrap_or(ideal);
        let last = *bounds.last().expect("starts with zero");
        if at > last + min_frames.min(1) && frames - at >= 1 {
            bounds.push(at);
        }
    }
    bounds.push(frames);
    bounds.windows(2).map(|w| w[0]..w[1]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use geneva_timeline::load;

    fn comp(duration: &str, clips: &str) -> Composition {
        let text = format!(
            r#"{{"geneva":"1.0","output":{{"width":64,"height":64,"fps":24,"duration":"{duration}"}},
                "layers":[{{"clips":[{clips}]}}]}}"#
        );
        load(&text).composition.unwrap()
    }

    const SOLID: &str = r#"{"source":{"kind":"solid","color":"red"}}"#;

    #[test]
    fn a_count_splits_evenly_and_shares_the_cores() {
        let c = comp("12s", SOLID);
        let plan = plan_chunks(
            &c,
            VideoCodec::Vp9,
            HardwarePolicy::Never,
            None,
            Some(Chunks::Count(3)),
            12,
        );
        assert_eq!(plan.ranges, vec![0..96, 96..192, 192..288]);
        assert_eq!(plan.threads_each, 4);
    }

    #[test]
    fn boundaries_move_to_nearby_clip_starts() {
        // Clips at 0, 5.5 s and 8 s; two chunks of a 12 s output: the
        // midpoint (6 s) moves to the cut at 5.5 s.
        let clips = r#"{"source":{"kind":"solid","color":"red"},"duration":"5.5s"},{"source":{"kind":"solid","color":"blue"},"start":"5.5s","duration":"2.5s"},{"source":{"kind":"solid","color":"green"},"start":"8s","duration":"4s"}"#;
        let c = comp("12s", clips);
        let plan = plan_chunks(
            &c,
            VideoCodec::Vp9,
            HardwarePolicy::Never,
            None,
            Some(Chunks::Count(2)),
            8,
        );
        assert_eq!(plan.ranges, vec![0..132, 132..288]);
    }

    #[test]
    fn boundaries_fall_on_the_keyframe_grid() {
        let c = comp("12s", SOLID);
        let plan = plan_chunks(
            &c,
            VideoCodec::Vp9,
            HardwarePolicy::Never,
            Some(2.0),
            Some(Chunks::Count(3)),
            8,
        );
        // Ideal boundaries at 96 and 192 are on the 48-frame grid already.
        assert_eq!(plan.ranges, vec![0..96, 96..192, 192..288]);
        let plan = plan_chunks(
            &c,
            VideoCodec::Vp9,
            HardwarePolicy::Never,
            Some(2.5),
            Some(Chunks::Count(3)),
            8,
        );
        // A 60-frame grid: 96 moves to 120 and 192 to 180, both within a
        // quarter of a stretch.
        assert_eq!(plan.ranges, vec![0..120, 120..180, 180..288]);
    }

    #[test]
    fn auto_follows_the_encoders_ceiling_and_the_cores() {
        let c = comp("60s", SOLID);
        let auto = Some(Chunks::Auto(AutoChunks::Auto));
        let n = |codec, cores| {
            plan_chunks(&c, codec, HardwarePolicy::Never, None, auto, cores)
                .ranges
                .len()
        };
        assert_eq!(n(VideoCodec::Vp9, 4), 2);
        assert_eq!(n(VideoCodec::Vp9, 16), 8);
        // Not measured to gain: one run, whatever the machine.
        assert_eq!(n(VideoCodec::Dnxhd, 8), 1);
        assert_eq!(n(VideoCodec::Av1, 32), 1);
        assert_eq!(n(VideoCodec::H265, 8), 1);
        assert_eq!(n(VideoCodec::Prores, 64), 1);
    }

    #[test]
    fn split_frames_has_no_cap_but_the_two_second_minimum() {
        // A 10-minute output in 200 parts: more than chunked encoding's
        // 16, each 3 s.
        let c = comp("600s", SOLID);
        let ranges = split_frames(&c, None, 200);
        assert_eq!(ranges.len(), 200);
        assert!(ranges.iter().all(|r| r.end - r.start == 72));
        assert_eq!(ranges.last().unwrap().end, 600 * 24);
        // A 5 s output holds two parts of at least 2 s, whatever is asked.
        let c = comp("5s", SOLID);
        assert_eq!(split_frames(&c, None, 50), vec![0..60, 60..120]);
    }

    #[test]
    fn short_outputs_and_one_are_a_single_run() {
        let c = comp("3s", SOLID);
        let plan = plan_chunks(
            &c,
            VideoCodec::Dnxhd,
            HardwarePolicy::Never,
            None,
            Some(Chunks::Count(4)),
            8,
        );
        assert!(!plan.is_chunked());
        let c = comp("60s", SOLID);
        let plan = plan_chunks(
            &c,
            VideoCodec::Dnxhd,
            HardwarePolicy::Never,
            None,
            Some(Chunks::Count(1)),
            8,
        );
        assert!(!plan.is_chunked());
    }
}
