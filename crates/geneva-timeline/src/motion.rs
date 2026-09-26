//! Motion of one element inside markup: its `animation` list played at a
//! moment, the way a browser plays it.
//!
//! The clip plays the outermost element's animation by turning it into
//! tracks. An element inside is different: several animations stack on
//! it, each reads the value under it for a keyframe it does not set, and
//! a fill mode decides what is left outside the runs. That is worked out
//! here at each frame rather than expanded into tracks, since the result
//! is a set of style overrides for the layout and a transform for the
//! renderer to composite the element with.

use geneva_anim::Easing;
use geneva_color::{Color, LinearRgba, Transfer};
use geneva_html::{Computed, Extent, Overrides, Shadow, extent_of};

use crate::animation::{Animation, Shift, TextShadow, Values};

/// One animation of the list, with its rule's keyframes by offset.
#[derive(Debug, Clone)]
pub struct Play {
    /// The animation as written.
    pub animation: Animation,
    /// The rule's keyframes, in offset order.
    pub frames: Vec<(f64, Values)>,
}

/// An element's animations.
#[derive(Debug, Clone)]
pub struct NodeMotion {
    /// The element, as a node index in the prepared document.
    pub node: usize,
    /// Its animations, in the order written, later ones on top.
    pub plays: Vec<Play>,
}

/// A transform sampled at a moment, in pixels and degrees, applied about
/// the element's centre.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    /// Pixels moved.
    pub translate: [f64; 2],
    /// Factors.
    pub scale: [f64; 2],
    /// Degrees, clockwise.
    pub rotate: f64,
}

impl Transform {
    /// No change.
    pub const IDENTITY: Self = Self {
        translate: [0.0, 0.0],
        scale: [1.0, 1.0],
        rotate: 0.0,
    };

    /// Whether it changes nothing.
    #[must_use]
    pub fn is_identity(&self) -> bool {
        *self == Self::IDENTITY
    }
}

/// What the element's animations say at one moment.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Sampled {
    /// Style overrides for the layout and paint.
    pub overrides: Overrides,
    /// The transform, when an animation sets one.
    pub transform: Option<Transform>,
}

impl Play {
    /// Where in a run the animation is at `t`, in 0 to 1, or `None` when
    /// it is not playing and its fill mode leaves the element alone.
    fn progress(&self, t: f64) -> Option<f64> {
        let a = &self.animation;
        let run_progress = |run: f64, p: f64| {
            if a.direction.reversed(run.max(0.0) as u32) {
                1.0 - p
            } else {
                p
            }
        };
        if t < a.delay {
            return a.fill.backwards().then(|| run_progress(0.0, 0.0));
        }
        let local = t - a.delay;
        if a.iterations.is_finite() && local >= a.iterations * a.duration {
            if !a.fill.forwards() {
                return None;
            }
            // The end of the last run, which a fractional count cuts short.
            let last = (a.iterations.ceil() - 1.0).max(0.0);
            let p = (a.iterations - last).clamp(0.0, 1.0);
            return Some(run_progress(last, p));
        }
        let run = (local / a.duration).floor();
        Some(run_progress(run, local / a.duration - run))
    }
}

impl NodeMotion {
    /// Whether any of it changes where boxes land.
    #[must_use]
    pub fn moves_layout(&self) -> bool {
        self.plays
            .iter()
            .any(|p| p.frames.iter().any(|(_, v)| v.moves_layout()))
    }

    /// The element's animations at `t` seconds into the clip, over the
    /// style it has without them. `box_size` is the element's border
    /// box, which a percentage in a translation is a share of.
    #[must_use]
    pub fn sample(&self, t: f64, base: &Computed, box_size: (f64, f64)) -> Sampled {
        let plays = &self.plays;
        let ext = |s: Shift| match s {
            Shift::Px(p) => Extent::Px(p),
            Shift::Percent(p) => Extent::Percent(p),
        };
        let overrides = Overrides {
            opacity: stacked(plays, t, base.paint.opacity, |v| v.opacity, lerp),
            blur: stacked(plays, t, base.paint.blur, |v| v.blur, lerp),
            color: stacked(plays, t, base.text.color, |v| v.color, color_lerp),
            text_shadow: stacked(
                plays,
                t,
                base.text.shadow.clone(),
                |v| {
                    v.text_shadow
                        .as_ref()
                        .map(|list| list.iter().copied().map(shadow_of).collect())
                },
                shadows_lerp,
            ),
            letter_spacing: stacked(
                plays,
                t,
                base.text.letter_spacing,
                |v| v.letter_spacing.map(|l| l.to_px(base.text.size)),
                lerp,
            ),
            width: stacked(
                plays,
                t,
                extent_of(base.layout.size.width),
                |v| v.width.map(ext),
                extent_lerp,
            ),
            height: stacked(
                plays,
                t,
                extent_of(base.layout.size.height),
                |v| v.height.map(ext),
                extent_lerp,
            ),
            max_width: stacked(
                plays,
                t,
                extent_of(base.layout.max_size.width),
                |v| v.max_width.map(ext),
                extent_lerp,
            ),
            min_width: stacked(
                plays,
                t,
                extent_of(base.layout.min_size.width),
                |v| v.min_width.map(ext),
                extent_lerp,
            ),
            background_position: stacked(
                plays,
                t,
                base.paint.background_position,
                |v| v.background_position.map(|[x, y]| (ext(x), ext(y))),
                |a, b, u| (extent_lerp(a.0, b.0, u), extent_lerp(a.1, b.1, u)),
            ),
            clip_path: stacked(
                plays,
                t,
                base.paint.clip_path.clone().unwrap_or_default(),
                |v| {
                    v.clip_path
                        .as_ref()
                        .map(|p| p.iter().map(|[x, y]| (ext(*x), ext(*y))).collect())
                },
                polygon_lerp,
            ),
        };
        let transform = stacked(
            plays,
            t,
            Transform::IDENTITY,
            |v| transform_of(v, box_size),
            transform_lerp,
        );
        Sampled {
            overrides,
            transform,
        }
    }
}

/// One property through the stack of animations: each one that is
/// playing reads the value under it for an endpoint its rule leaves
/// out, so a rule with only `to` starts from wherever the ones below it
/// (or the style) left the element. `None` when no animation touches it.
fn stacked<V: Clone>(
    plays: &[Play],
    t: f64,
    base: V,
    pick: impl Fn(&Values) -> Option<V>,
    lerp: impl Fn(V, V, f64) -> V,
) -> Option<V> {
    let mut value = base;
    let mut touched = false;
    for play in plays {
        let Some(p) = play.progress(t) else {
            continue;
        };
        let mut keys: Vec<(f64, V)> = play
            .frames
            .iter()
            .filter_map(|(o, v)| pick(v).map(|x| (*o, x)))
            .collect();
        if keys.is_empty() {
            continue;
        }
        if keys[0].0 > 0.0 {
            keys.insert(0, (0.0, value.clone()));
        }
        if keys[keys.len() - 1].0 < 1.0 {
            keys.push((1.0, value.clone()));
        }
        value = interpolate(&keys, p, &play.animation.easing, &lerp);
        touched = true;
    }
    touched.then_some(value)
}

/// The value at progress `p` between the keyframes around it, eased.
fn interpolate<V: Clone>(
    keys: &[(f64, V)],
    p: f64,
    easing: &Easing,
    lerp: &impl Fn(V, V, f64) -> V,
) -> V {
    if p <= keys[0].0 {
        return keys[0].1.clone();
    }
    for pair in keys.windows(2) {
        let ((a, va), (b, vb)) = (&pair[0], &pair[1]);
        if p <= *b {
            let span = b - a;
            let u = if span <= 0.0 { 1.0 } else { (p - a) / span };
            return lerp(va.clone(), vb.clone(), easing.evaluate(u));
        }
    }
    keys[keys.len() - 1].1.clone()
}

/// Polygons with the same number of points mix point by point; any other
/// pair, or `none` against a polygon, is a step, as CSS makes it.
fn polygon_lerp(
    a: Vec<(Extent, Extent)>,
    b: Vec<(Extent, Extent)>,
    u: f64,
) -> Vec<(Extent, Extent)> {
    if a.len() != b.len() || a.is_empty() {
        return step(a, b, u);
    }
    a.iter()
        .zip(&b)
        .map(|(p, q)| (extent_lerp(p.0, q.0, u), extent_lerp(p.1, q.1, u)))
        .collect()
}

fn lerp(a: f64, b: f64, u: f64) -> f64 {
    a + (b - a) * u
}

/// Colours mix in linear light, as everything else here does.
fn color_lerp(a: Color, b: Color, u: f64) -> Color {
    let (la, lb) = (a.to_linear(), b.to_linear());
    let k = u as f32;
    let m = LinearRgba {
        r: la.r + (lb.r - la.r) * k,
        g: la.g + (lb.g - la.g) * k,
        b: la.b + (lb.b - la.b) * k,
        a: la.a + (lb.a - la.a) * k,
    };
    if m.a <= 0.0 {
        return Color::TRANSPARENT;
    }
    let enc = |v: f32| Transfer::Srgb.from_linear(f64::from(v / m.a)) as f32;
    Color {
        r: enc(m.r),
        g: enc(m.g),
        b: enc(m.b),
        a: m.a,
    }
}

fn shadow_of(s: TextShadow) -> Shadow {
    Shadow {
        x: s.x,
        y: s.y,
        blur: s.blur,
        color: s.color,
    }
}

/// Two shadow lists mix shadow by shadow. The shorter list is padded
/// with shadows of no offset, no blur and a transparent colour, as CSS
/// pads it, so a glow that appears mid-rule fades in rather than pops.
fn shadows_lerp(mut a: Vec<Shadow>, mut b: Vec<Shadow>, u: f64) -> Vec<Shadow> {
    let none = Shadow {
        x: 0.0,
        y: 0.0,
        blur: 0.0,
        color: Color::TRANSPARENT,
    };
    let n = a.len().max(b.len());
    a.resize(n, none);
    b.resize(n, none);
    a.into_iter()
        .zip(b)
        .map(|(a, b)| Shadow {
            x: lerp(a.x, b.x, u),
            y: lerp(a.y, b.y, u),
            blur: lerp(a.blur, b.blur, u).max(0.0),
            color: color_lerp(a.color, b.color, u),
        })
        .collect()
}

/// Lengths of one kind mix; a length and a percentage, or either and
/// `auto`, cannot, so the change is a step. Zero is zero in any unit, so
/// it mixes with either.
fn extent_lerp(a: Extent, b: Extent, u: f64) -> Extent {
    match (a, b) {
        (Extent::Px(x), Extent::Px(y)) => Extent::Px(lerp(x, y, u)),
        (Extent::Percent(x), Extent::Percent(y)) => Extent::Percent(lerp(x, y, u)),
        (Extent::Px(0.0), Extent::Percent(y)) => Extent::Percent(lerp(0.0, y, u)),
        (Extent::Percent(x), Extent::Px(0.0)) => Extent::Percent(lerp(x, 0.0, u)),
        _ => step(a, b, u),
    }
}

/// What CSS does with two values that cannot be mixed: the first for the
/// first half, the second for the rest.
fn step<V>(a: V, b: V, u: f64) -> V {
    if u < 0.5 { a } else { b }
}

/// A keyframe's transform, complete: a function it does not name is the
/// identity, since `transform` is one property.
fn transform_of(v: &Values, box_size: (f64, f64)) -> Option<Transform> {
    if v.translate.is_none() && v.scale.is_none() && v.rotate.is_none() {
        return None;
    }
    let px = |s: Shift, of: f64| s.to_px(Some(of)).unwrap_or(0.0);
    Some(Transform {
        translate: v
            .translate
            .map_or([0.0, 0.0], |[x, y]| [px(x, box_size.0), px(y, box_size.1)]),
        scale: v.scale.unwrap_or([1.0, 1.0]),
        rotate: v.rotate.unwrap_or(0.0),
    })
}

fn transform_lerp(a: Transform, b: Transform, u: f64) -> Transform {
    Transform {
        translate: [
            lerp(a.translate[0], b.translate[0], u),
            lerp(a.translate[1], b.translate[1], u),
        ],
        scale: [
            lerp(a.scale[0], b.scale[0], u),
            lerp(a.scale[1], b.scale[1], u),
        ],
        rotate: lerp(a.rotate, b.rotate, u),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::animation::{Direction, Fill};

    fn play(delay: f64, duration: f64, fill: Fill, frames: Vec<(f64, Values)>) -> Play {
        Play {
            animation: Animation {
                name: "a".to_owned(),
                duration,
                delay,
                easing: Easing::default(),
                iterations: 1.0,
                direction: Direction::Normal,
                fill,
            },
            frames,
        }
    }

    fn opacity(v: f64) -> Values {
        Values {
            opacity: Some(v),
            ..Values::default()
        }
    }

    fn at(motion: &NodeMotion, t: f64) -> Option<f64> {
        motion
            .sample(t, &Computed::default(), (100.0, 50.0))
            .overrides
            .opacity
    }

    #[test]
    fn a_rule_with_only_to_starts_from_the_style() {
        let mut base = Computed::default();
        base.paint.opacity = 0.0;
        let m = NodeMotion {
            node: 0,
            plays: vec![play(1.0, 2.0, Fill::Forwards, vec![(1.0, opacity(1.0))])],
        };
        let o = |t: f64| m.sample(t, &base, (1.0, 1.0)).overrides.opacity;
        assert_eq!(o(0.5), None, "before the delay, no fill backwards");
        assert!(
            (o(2.0).unwrap() - 0.5).abs() < 1e-9,
            "halfway up from the style's 0"
        );
        assert!((o(10.0).unwrap() - 1.0).abs() < 1e-9, "held after the end");
    }

    #[test]
    fn a_later_animation_reads_the_one_under_it() {
        // show takes it to 1 and holds; hide, later in the list, then
        // takes it from that 1 down to 0.
        let m = NodeMotion {
            node: 0,
            plays: vec![
                play(0.0, 0.1, Fill::Forwards, vec![(1.0, opacity(1.0))]),
                play(1.0, 1.0, Fill::Forwards, vec![(1.0, opacity(0.0))]),
            ],
        };
        assert!((at(&m, 0.5).unwrap() - 1.0).abs() < 1e-9);
        assert!((at(&m, 1.5).unwrap() - 0.5).abs() < 1e-9);
        assert!((at(&m, 3.0).unwrap() - 0.0).abs() < 1e-9);
    }

    #[test]
    fn fill_none_lets_go_after_the_last_run() {
        let m = NodeMotion {
            node: 0,
            plays: vec![play(
                0.0,
                1.0,
                Fill::None,
                vec![(0.0, opacity(0.0)), (1.0, opacity(1.0))],
            )],
        };
        assert!((at(&m, 0.25).unwrap() - 0.25).abs() < 1e-9);
        assert_eq!(at(&m, 2.0), None);
    }

    #[test]
    fn a_transform_names_every_function_it_does_not_set() {
        let v = Values {
            scale: Some([0.5, 0.5]),
            ..Values::default()
        };
        let m = NodeMotion {
            node: 0,
            plays: vec![play(0.0, 1.0, Fill::Both, vec![(1.0, v)])],
        };
        let s = m.sample(0.5, &Computed::default(), (100.0, 50.0));
        let t = s.transform.expect("a transform");
        assert!((t.scale[0] - 0.75).abs() < 1e-9);
        assert_eq!(t.translate, [0.0, 0.0]);
        assert_eq!(t.rotate, 0.0);
    }

    #[test]
    fn a_shadow_that_joins_the_list_fades_in() {
        // From one shadow to two: the second is padded from a transparent
        // shadow of no size, so halfway it is half its final alpha and
        // half its blur.
        let one = |blur: f64, a: f32| TextShadow {
            x: 0.0,
            y: 0.0,
            blur,
            color: Color {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a,
            },
        };
        let from = Values {
            text_shadow: Some(vec![one(4.0, 1.0)]),
            ..Values::default()
        };
        let to = Values {
            text_shadow: Some(vec![one(4.0, 1.0), one(12.0, 0.5)]),
            ..Values::default()
        };
        let m = NodeMotion {
            node: 0,
            plays: vec![play(0.0, 1.0, Fill::Both, vec![(0.0, from), (1.0, to)])],
        };
        let s = m.sample(0.5, &Computed::default(), (1.0, 1.0));
        let list = s.overrides.text_shadow.expect("shadows");
        assert_eq!(list.len(), 2);
        assert!((list[1].blur - 6.0).abs() < 1e-9);
        assert!((list[1].color.a - 0.25).abs() < 1e-6, "{:?}", list[1].color);
    }

    #[test]
    fn a_width_animates_from_the_style_it_had() {
        // A style with `width: 0`, read the way the renderer reads one.
        let p = geneva_html::prepare(
            "<style>div { width: 0 }</style><div></div>",
            "",
            &std::collections::BTreeMap::new(),
        )
        .unwrap();
        let base = p.styles[p.doc.children(p.doc.root)[0]].clone();
        let v = Values {
            width: Some(Shift::Px(40.0)),
            ..Values::default()
        };
        let m = NodeMotion {
            node: 0,
            plays: vec![play(0.0, 1.0, Fill::Forwards, vec![(1.0, v)])],
        };
        assert_eq!(
            m.sample(0.5, &base, (1.0, 1.0)).overrides.width,
            Some(Extent::Px(20.0))
        );
        assert!(m.moves_layout());
    }
}
