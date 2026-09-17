//! One second-order section, and the two shapes the crate uses it in.

/// A biquad in direct form I, coefficients from the usual cookbook.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Biquad {
    pub(crate) b: [f64; 3],
    pub(crate) a: [f64; 3],
    x: [f64; 2],
    y: [f64; 2],
}

impl Biquad {
    pub(crate) fn new(b: [f64; 3], a: [f64; 3]) -> Self {
        Self {
            b,
            a,
            x: [0.0; 2],
            y: [0.0; 2],
        }
    }

    /// A second-order high-pass at `f0` Hz with quality `q` (0.7071 is
    /// Butterworth: flat above, -3 dB at `f0`, 12 dB an octave below).
    pub(crate) fn high_pass(rate: f64, f0: f64, q: f64) -> Self {
        let w = 2.0 * std::f64::consts::PI * f0 / rate;
        let (sin, cos) = w.sin_cos();
        let alpha = sin / (2.0 * q);
        let a0 = 1.0 + alpha;
        Self::new(
            [
                f64::midpoint(1.0, cos) / a0,
                -(1.0 + cos) / a0,
                f64::midpoint(1.0, cos) / a0,
            ],
            [1.0, -2.0 * cos / a0, (1.0 - alpha) / a0],
        )
    }

    /// A notch at `f0` Hz, `bandwidth` Hz wide between its -3 dB points.
    pub(crate) fn notch(rate: f64, f0: f64, bandwidth: f64) -> Self {
        let w = 2.0 * std::f64::consts::PI * f0 / rate;
        let (sin, cos) = w.sin_cos();
        let q = f0 / bandwidth.max(1e-3);
        let alpha = sin / (2.0 * q);
        let a0 = 1.0 + alpha;
        Self::new(
            [1.0 / a0, -2.0 * cos / a0, 1.0 / a0],
            [1.0, -2.0 * cos / a0, (1.0 - alpha) / a0],
        )
    }

    pub(crate) fn run(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.b[1] * self.x[0] + self.b[2] * self.x[1]
            - self.a[1] * self.y[0]
            - self.a[2] * self.y[1];
        self.x = [x, self.x[0]];
        self.y = [y, self.y[0]];
        y
    }
}
