use std::cmp::Ordering;

use crate::Easing;

/// A value that can be linearly interpolated.
pub trait Interpolate: Clone {
    /// Returns the value `t` of the way from `self` to `other`.
    ///
    /// `t` may lie outside `[0, 1]` for easing curves that overshoot;
    /// implementations should extrapolate rather than clamp.
    fn lerp(&self, other: &Self, t: f64) -> Self;
}

impl Interpolate for f64 {
    fn lerp(&self, other: &Self, t: f64) -> Self {
        self + (other - self) * t
    }
}

impl Interpolate for f32 {
    fn lerp(&self, other: &Self, t: f64) -> Self {
        (f64::from(*self) + (f64::from(*other) - f64::from(*self)) * t) as f32
    }
}

impl<const N: usize> Interpolate for [f64; N] {
    fn lerp(&self, other: &Self, t: f64) -> Self {
        let mut out = *self;
        for (o, (a, b)) in out.iter_mut().zip(self.iter().zip(other.iter())) {
            *o = a.lerp(b, t);
        }
        out
    }
}

impl<const N: usize> Interpolate for [f32; N] {
    fn lerp(&self, other: &Self, t: f64) -> Self {
        let mut out = *self;
        for (o, (a, b)) in out.iter_mut().zip(self.iter().zip(other.iter())) {
            *o = a.lerp(b, t);
        }
        out
    }
}

/// A keyframe: a value at a time, with the easing used to reach the next one.
#[derive(Debug, Clone, PartialEq)]
pub struct Keyframe<T> {
    /// Time in seconds, relative to the track's origin.
    pub time: f64,
    /// The value at `time`.
    pub value: T,
    /// Easing applied between this keyframe and the following one.
    pub easing: Easing,
}

/// An ordered list of keyframes that can be sampled at any time.
///
/// Before the first keyframe the track holds the first value; after the last
/// it holds the last. Tracks are immutable once built, so sampling never
/// depends on previous samples.
#[derive(Debug, Clone, PartialEq)]
pub struct Track<T> {
    keys: Vec<Keyframe<T>>,
}

impl<T: Interpolate> Track<T> {
    /// Builds a track from keyframes.
    ///
    /// Keyframes must be sorted by strictly increasing time; the caller is
    /// expected to have validated that and reported a diagnostic otherwise.
    /// Returns `None` if the list is empty or not strictly sorted.
    pub fn new(keys: Vec<Keyframe<T>>) -> Option<Self> {
        if keys.is_empty() {
            return None;
        }
        if keys
            .windows(2)
            .any(|w| w[1].time.partial_cmp(&w[0].time) != Some(Ordering::Greater))
        {
            return None;
        }
        Some(Self { keys })
    }

    /// A track holding a single constant value.
    pub fn constant(value: T) -> Self {
        Self {
            keys: vec![Keyframe {
                time: 0.0,
                value,
                easing: Easing::default(),
            }],
        }
    }

    /// The keyframes in time order.
    pub fn keyframes(&self) -> &[Keyframe<T>] {
        &self.keys
    }

    /// Returns true when the track never changes value.
    pub fn is_constant(&self) -> bool {
        self.keys.len() == 1
    }

    /// Samples the track at `time` seconds.
    pub fn sample(&self, time: f64) -> T {
        let first = &self.keys[0];
        if time <= first.time {
            return first.value.clone();
        }
        let last = &self.keys[self.keys.len() - 1];
        if time >= last.time {
            return last.value.clone();
        }
        // `partition_point` finds the first keyframe strictly after `time`;
        // the segment of interest starts at the keyframe just before it.
        let next = self.keys.partition_point(|k| k.time <= time);
        let a = &self.keys[next - 1];
        let b = &self.keys[next];
        let u = (time - a.time) / (b.time - a.time);
        a.value.lerp(&b.value, a.easing.evaluate(u))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NamedEasing;

    fn key(time: f64, value: f64, easing: Easing) -> Keyframe<f64> {
        Keyframe {
            time,
            value,
            easing,
        }
    }

    #[test]
    fn holds_outside_range() {
        let t = Track::new(vec![
            key(1.0, 10.0, Easing::default()),
            key(2.0, 20.0, Easing::default()),
        ])
        .unwrap();
        assert_eq!(t.sample(0.0), 10.0);
        assert_eq!(t.sample(1.0), 10.0);
        assert_eq!(t.sample(2.0), 20.0);
        assert_eq!(t.sample(5.0), 20.0);
    }

    #[test]
    fn interpolates_linearly() {
        let t = Track::new(vec![
            key(0.0, 0.0, Easing::default()),
            key(4.0, 8.0, Easing::default()),
        ])
        .unwrap();
        assert_eq!(t.sample(1.0), 2.0);
        assert_eq!(t.sample(3.0), 6.0);
    }

    #[test]
    fn easing_applies_to_the_segment_leaving_a_keyframe() {
        let t = Track::new(vec![
            key(0.0, 0.0, Easing::Named(NamedEasing::Hold)),
            key(1.0, 1.0, Easing::default()),
            key(2.0, 2.0, Easing::default()),
        ])
        .unwrap();
        assert_eq!(t.sample(0.5), 0.0);
        assert_eq!(t.sample(0.999), 0.0);
        assert_eq!(t.sample(1.0), 1.0);
        assert_eq!(t.sample(1.5), 1.5);
    }

    #[test]
    fn rejects_unsorted_or_empty() {
        assert!(Track::<f64>::new(vec![]).is_none());
        assert!(
            Track::new(vec![
                key(1.0, 0.0, Easing::default()),
                key(1.0, 1.0, Easing::default())
            ])
            .is_none()
        );
        assert!(
            Track::new(vec![
                key(2.0, 0.0, Easing::default()),
                key(1.0, 1.0, Easing::default())
            ])
            .is_none()
        );
    }

    #[test]
    fn arrays_interpolate_componentwise() {
        let t = Track::new(vec![
            Keyframe {
                time: 0.0,
                value: [0.0, 10.0],
                easing: Easing::default(),
            },
            Keyframe {
                time: 2.0,
                value: [2.0, 0.0],
                easing: Easing::default(),
            },
        ])
        .unwrap();
        assert_eq!(t.sample(1.0), [1.0, 5.0]);
    }

    #[test]
    fn sampling_is_pure() {
        let t = Track::new(vec![
            key(0.0, 0.0, Easing::Named(NamedEasing::EaseInOut)),
            key(1.0, 1.0, Easing::default()),
        ])
        .unwrap();
        let a = t.sample(0.37);
        let _ = t.sample(0.9);
        let b = t.sample(0.37);
        assert_eq!(a.to_bits(), b.to_bits());
    }
}
