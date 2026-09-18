//! What transitions do to a frame at a time: the gain they put on a
//! clip's opacity, and the color a fade dips the whole picture through.
//! Shared by the CPU reference renderer and the GPU renderer.

use geneva_color::LinearRgba;
use geneva_timeline::{Ratio, ResolvedLayer, ResolvedTransition};

/// What a clip's transitions do to its opacity at output time `t`: its
/// own transition brings it in, and the transition on the clip after it
/// takes it out. A crossfade leaves the outgoing clip alone, since the
/// one arriving covers it; a fade takes it down to the dip color.
pub fn transition_gain(layer: &ResolvedLayer, i: usize, t: Ratio) -> f64 {
    let clip = &layer.clips[i];
    let mut gain = 1.0;
    if let Some(tr) = &clip.transition_in {
        gain *= tr.incoming(t - clip.start);
    }
    if let Some(tr) = layer
        .clips
        .get(i + 1)
        .and_then(|next| next.transition_in.as_ref())
    {
        gain *= tr.outgoing(clip.end - t);
    }
    // Nothing follows, so the clip closes the layer on its own terms.
    if let Some(tr) = &clip.transition_out {
        gain *= tr.outgoing(clip.end - t);
    }
    gain
}

/// The dip color showing at `t`, if any clip is mid-fade, and how much of
/// it: the frame is blended toward the color by that fraction after the
/// layers are drawn ([`crate::Frame::veil`]). The strongest one wins, so
/// overlapping fades do not cancel each other out.
pub fn fade_veil(layers: &[ResolvedLayer], t: Ratio) -> Option<(LinearRgba, f64)> {
    let mut found: Option<(LinearRgba, f64)> = None;
    let mut strongest = |tr: Option<&ResolvedTransition>, local: Ratio| {
        let Some(tr) = tr else { return };
        if local < Ratio::ZERO || local >= tr.duration {
            return;
        }
        let a = tr.veil(local);
        if a > 0.0 && found.is_none_or(|(_, best)| a > best) {
            found = Some((tr.color, a));
        }
    };
    for layer in layers {
        for clip in &layer.clips {
            strongest(clip.transition_in.as_ref(), t - clip.start);
            // A closing transition is measured back from the clip's end.
            strongest(clip.transition_out.as_ref(), clip.end - t);
        }
    }
    found
}
