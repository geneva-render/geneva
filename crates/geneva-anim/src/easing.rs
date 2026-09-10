use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Cubic-bezier control points for the CSS `ease` preset.
const EASE: [f64; 4] = [0.25, 0.1, 0.25, 1.0];
const EASE_IN: [f64; 4] = [0.42, 0.0, 1.0, 1.0];
const EASE_OUT: [f64; 4] = [0.0, 0.0, 0.58, 1.0];
const EASE_IN_OUT: [f64; 4] = [0.42, 0.0, 0.58, 1.0];

/// A named easing preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum NamedEasing {
    /// Constant speed.
    Linear,
    /// CSS `ease`: gentle acceleration, longer deceleration.
    Ease,
    /// CSS `ease-in`: starts slowly.
    EaseIn,
    /// CSS `ease-out`: ends slowly.
    EaseOut,
    /// CSS `ease-in-out`: slow at both ends.
    EaseInOut,
    /// Holds the starting value for the whole segment, then jumps.
    Hold,
}

/// Parameters of a damped spring.
///
/// The spring is solved analytically and rescaled so that it settles exactly
/// at the end of the keyframe segment; the parameters control the shape of
/// the motion (overshoot and bounce), not its duration.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Spring {
    /// Spring constant. Higher values make a snappier motion.
    #[serde(default = "Spring::default_stiffness")]
    pub stiffness: f64,
    /// Damping coefficient. Lower values bounce more.
    #[serde(default = "Spring::default_damping")]
    pub damping: f64,
    /// Attached mass. Higher values make a slower, heavier motion.
    #[serde(default = "Spring::default_mass")]
    pub mass: f64,
}

impl Spring {
    fn default_stiffness() -> f64 {
        170.0
    }

    fn default_damping() -> f64 {
        26.0
    }

    fn default_mass() -> f64 {
        1.0
    }

    /// Returns true when every parameter is finite and positive.
    pub fn is_valid(&self) -> bool {
        [self.stiffness, self.damping, self.mass]
            .iter()
            .all(|v| v.is_finite() && *v > 0.0)
    }

    /// Displacement of the spring at physical time `t`, starting at rest at 0
    /// and targeting 1.
    fn displacement(&self, t: f64) -> f64 {
        let omega0 = (self.stiffness / self.mass).sqrt();
        let zeta = self.damping / (2.0 * (self.stiffness * self.mass).sqrt());
        if zeta < 1.0 {
            let omega_d = omega0 * (1.0 - zeta * zeta).sqrt();
            let decay = (-zeta * omega0 * t).exp();
            1.0 - decay * ((omega_d * t).cos() + (zeta * omega0 / omega_d) * (omega_d * t).sin())
        } else if (zeta - 1.0).abs() < 1e-9 {
            1.0 - (-omega0 * t).exp() * (1.0 + omega0 * t)
        } else {
            let root = omega0 * (zeta * zeta - 1.0).sqrt();
            let r1 = -zeta * omega0 + root;
            let r2 = -zeta * omega0 - root;
            let c2 = r1 / (r1 - r2);
            let c1 = 1.0 - c2;
            1.0 - c1 * (r1 * t).exp() - c2 * (r2 * t).exp()
        }
    }

    /// Physical time after which the envelope of the motion stays within
    /// 0.1% of the target.
    fn settle_time(&self) -> f64 {
        let omega0 = (self.stiffness / self.mass).sqrt();
        let zeta = self.damping / (2.0 * (self.stiffness * self.mass).sqrt());
        let rate = if zeta < 1.0 {
            zeta * omega0
        } else {
            // Slowest decaying mode of the (over|critically) damped solution.
            omega0 * (zeta - (zeta * zeta - 1.0).max(0.0).sqrt())
        };
        -(1e-3_f64).ln() / rate
    }
}

impl Default for Spring {
    fn default() -> Self {
        Self {
            stiffness: Self::default_stiffness(),
            damping: Self::default_damping(),
            mass: Self::default_mass(),
        }
    }
}

/// An easing curve applied to the segment that starts at a keyframe.
///
/// In JSON an easing is either a preset name (`"ease-out"`), an object with a
/// `cubic-bezier` array, or an object with a `spring` block.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Easing {
    /// A named preset.
    Named(NamedEasing),
    /// A cubic bezier with control points `[x1, y1, x2, y2]`, as in CSS.
    CubicBezier {
        /// Control points; `x1` and `x2` must lie in `[0, 1]`.
        #[serde(rename = "cubic-bezier")]
        cubic_bezier: [f64; 4],
    },
    /// A damped spring, rescaled to settle at the end of the segment.
    Spring {
        /// Spring parameters.
        spring: Spring,
    },
}

impl Default for Easing {
    fn default() -> Self {
        Self::Named(NamedEasing::Linear)
    }
}

impl Easing {
    /// Returns `None` when the curve is well formed, otherwise a short reason.
    pub fn validate(&self) -> Option<String> {
        match self {
            Self::Named(_) => None,
            Self::CubicBezier {
                cubic_bezier: [x1, _, x2, _],
            } => {
                if !(0.0..=1.0).contains(x1) || !(0.0..=1.0).contains(x2) {
                    Some("cubic-bezier x control points must be between 0 and 1".to_owned())
                } else {
                    None
                }
            }
            Self::Spring { spring } => {
                if spring.is_valid() {
                    None
                } else {
                    Some("spring stiffness, damping and mass must be positive".to_owned())
                }
            }
        }
    }

    /// Maps normalized progress `u` in `[0, 1]` to eased progress.
    ///
    /// The result is 0 at `u = 0` and 1 at `u = 1`; between them it may leave
    /// `[0, 1]` for curves that overshoot. Inputs outside `[0, 1]` are clamped.
    pub fn evaluate(&self, u: f64) -> f64 {
        let u = u.clamp(0.0, 1.0);
        match self {
            Self::Named(NamedEasing::Linear) => u,
            Self::Named(NamedEasing::Ease) => cubic_bezier(EASE, u),
            Self::Named(NamedEasing::EaseIn) => cubic_bezier(EASE_IN, u),
            Self::Named(NamedEasing::EaseOut) => cubic_bezier(EASE_OUT, u),
            Self::Named(NamedEasing::EaseInOut) => cubic_bezier(EASE_IN_OUT, u),
            Self::Named(NamedEasing::Hold) => {
                if u >= 1.0 {
                    1.0
                } else {
                    0.0
                }
            }
            Self::CubicBezier { cubic_bezier } => cubic_bezier_fn(*cubic_bezier, u),
            Self::Spring { spring } => {
                if u >= 1.0 {
                    1.0
                } else {
                    spring.displacement(u * spring.settle_time())
                }
            }
        }
    }
}

fn cubic_bezier(points: [f64; 4], u: f64) -> f64 {
    cubic_bezier_fn(points, u)
}

/// Evaluates a CSS-style cubic bezier timing function.
///
/// The curve is parametric; the input `u` is an x coordinate, so the
/// parameter is found by bisection before the y coordinate is evaluated.
fn cubic_bezier_fn([x1, y1, x2, y2]: [f64; 4], u: f64) -> f64 {
    if u <= 0.0 {
        return 0.0;
    }
    if u >= 1.0 {
        return 1.0;
    }
    let bezier = |p1: f64, p2: f64, t: f64| {
        let mt = 1.0 - t;
        3.0 * mt * mt * t * p1 + 3.0 * mt * t * t * p2 + t * t * t
    };
    let (mut lo, mut hi) = (0.0_f64, 1.0_f64);
    let mut t = u;
    for _ in 0..48 {
        let x = bezier(x1, x2, t);
        if (x - u).abs() < 1e-9 {
            break;
        }
        if x < u {
            lo = t;
        } else {
            hi = t;
        }
        t = 0.5 * (lo + hi);
    }
    bezier(y1, y2, t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_are_exact() {
        let curves = [
            Easing::Named(NamedEasing::Linear),
            Easing::Named(NamedEasing::Ease),
            Easing::Named(NamedEasing::EaseInOut),
            Easing::Named(NamedEasing::Hold),
            Easing::CubicBezier {
                cubic_bezier: [0.3, -0.5, 0.7, 1.5],
            },
            Easing::Spring {
                spring: Spring::default(),
            },
        ];
        for curve in curves {
            assert_eq!(curve.evaluate(0.0), 0.0, "{curve:?}");
            assert_eq!(curve.evaluate(1.0), 1.0, "{curve:?}");
        }
    }

    #[test]
    fn linear_is_identity() {
        let e = Easing::Named(NamedEasing::Linear);
        for i in 0..=10 {
            let u = f64::from(i) / 10.0;
            assert!((e.evaluate(u) - u).abs() < 1e-12);
        }
    }

    #[test]
    fn ease_in_out_is_symmetric() {
        let e = Easing::Named(NamedEasing::EaseInOut);
        for i in 0..=10 {
            let u = f64::from(i) / 10.0;
            assert!((e.evaluate(u) + e.evaluate(1.0 - u) - 1.0).abs() < 1e-6);
        }
        assert!(e.evaluate(0.25) < 0.25);
        assert!(e.evaluate(0.75) > 0.75);
    }

    #[test]
    fn bezier_matches_reference_values() {
        // CSS `ease` at u = 0.5 is 0.8024 (rounded), a widely published value.
        let e = Easing::Named(NamedEasing::Ease);
        assert!((e.evaluate(0.5) - 0.8024).abs() < 1e-3);
    }

    #[test]
    fn underdamped_spring_overshoots_and_settles() {
        let e = Easing::Spring {
            spring: Spring {
                stiffness: 300.0,
                damping: 10.0,
                mass: 1.0,
            },
        };
        let max = (1..100)
            .map(|i| e.evaluate(f64::from(i) / 100.0))
            .fold(0.0, f64::max);
        assert!(max > 1.05, "expected overshoot, got max {max}");
        assert!((e.evaluate(0.999) - 1.0).abs() < 2e-3);
    }

    #[test]
    fn overdamped_spring_is_monotonic() {
        let e = Easing::Spring {
            spring: Spring {
                stiffness: 100.0,
                damping: 40.0,
                mass: 1.0,
            },
        };
        let mut prev = 0.0;
        for i in 1..=100 {
            let v = e.evaluate(f64::from(i) / 100.0);
            assert!(v >= prev - 1e-9, "not monotonic at {i}");
            prev = v;
        }
    }

    #[test]
    fn serde_round_trip() {
        let json =
            r#"["linear","ease-out",{"cubic-bezier":[0.1,0.2,0.3,0.4]},{"spring":{"damping":12}}]"#;
        let parsed: Vec<Easing> = serde_json::from_str(json).unwrap();
        assert_eq!(parsed[0], Easing::Named(NamedEasing::Linear));
        assert_eq!(parsed[1], Easing::Named(NamedEasing::EaseOut));
        assert_eq!(
            parsed[2],
            Easing::CubicBezier {
                cubic_bezier: [0.1, 0.2, 0.3, 0.4]
            }
        );
        assert_eq!(
            parsed[3],
            Easing::Spring {
                spring: Spring {
                    damping: 12.0,
                    ..Spring::default()
                }
            }
        );
        let back = serde_json::to_string(&parsed).unwrap();
        let again: Vec<Easing> = serde_json::from_str(&back).unwrap();
        assert_eq!(parsed, again);
    }

    #[test]
    fn validation_rejects_bad_curves() {
        assert!(
            Easing::CubicBezier {
                cubic_bezier: [1.5, 0.0, 0.5, 1.0]
            }
            .validate()
            .is_some()
        );
        assert!(
            Easing::Spring {
                spring: Spring {
                    stiffness: 0.0,
                    ..Spring::default()
                }
            }
            .validate()
            .is_some()
        );
        assert!(Easing::Named(NamedEasing::Ease).validate().is_none());
    }
}
