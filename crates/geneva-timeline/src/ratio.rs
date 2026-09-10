use std::cmp::Ordering;
use std::fmt;
use std::ops::{Add, Div, Mul, Sub};

/// An exact rational number used for all timeline arithmetic.
///
/// Frame boundaries at rates like 30000/1001 cannot be represented in binary
/// floating point; keeping times rational makes "which frame does this clip
/// start on" an exact question with an exact answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Ratio {
    num: i64,
    den: i64,
}

fn gcd(mut a: i64, mut b: i64) -> i64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a.abs()
}

impl Ratio {
    /// Zero.
    pub const ZERO: Self = Self { num: 0, den: 1 };
    /// One.
    pub const ONE: Self = Self { num: 1, den: 1 };

    /// Builds a reduced ratio. Panics if `den` is zero.
    pub fn new(num: i64, den: i64) -> Self {
        assert!(den != 0, "ratio denominator must not be zero");
        let g = gcd(num, den).max(1);
        let sign = if den < 0 { -1 } else { 1 };
        Self {
            num: sign * num / g,
            den: sign * den / g,
        }
    }

    /// Builds a ratio from a 128-bit intermediate, reducing before narrowing.
    fn from_wide(num: i128, den: i128) -> Self {
        debug_assert!(den != 0);
        let mut a = num.abs();
        let mut b = den.abs();
        while b != 0 {
            (a, b) = (b, a % b);
        }
        let g = a.max(1);
        let sign = if den < 0 { -1 } else { 1 };
        let num = sign * num / g;
        let den = sign * den / g;
        Self {
            num: i64::try_from(num).expect("ratio numerator overflow"),
            den: i64::try_from(den).expect("ratio denominator overflow"),
        }
    }

    /// An integer value.
    pub const fn from_int(v: i64) -> Self {
        Self { num: v, den: 1 }
    }

    /// Numerator of the reduced form.
    pub const fn numer(self) -> i64 {
        self.num
    }

    /// Denominator of the reduced form; always positive.
    pub const fn denom(self) -> i64 {
        self.den
    }

    /// Approximate value as `f64`.
    pub fn to_f64(self) -> f64 {
        self.num as f64 / self.den as f64
    }

    /// Returns true when the value is an integer.
    pub const fn is_integer(self) -> bool {
        self.den == 1
    }

    /// Largest integer not greater than the value.
    pub fn floor(self) -> i64 {
        self.num.div_euclid(self.den)
    }

    /// Smallest integer not less than the value.
    pub fn ceil(self) -> i64 {
        -((-self.num).div_euclid(self.den))
    }

    /// Nearest integer, rounding halves up.
    pub fn round(self) -> i64 {
        (self + Self::new(1, 2)).floor()
    }

    /// Reciprocal. Panics on zero.
    pub fn recip(self) -> Self {
        Self::new(self.den, self.num)
    }

    /// Returns true when the value is negative.
    pub const fn is_negative(self) -> bool {
        self.num < 0
    }

    /// Returns true when the value is zero.
    pub const fn is_zero(self) -> bool {
        self.num == 0
    }

    /// The larger of two values.
    pub fn max(self, other: Self) -> Self {
        if self >= other { self } else { other }
    }

    /// The smaller of two values.
    pub fn min(self, other: Self) -> Self {
        if self <= other { self } else { other }
    }
}

impl Add for Ratio {
    type Output = Self;
    fn add(self, o: Self) -> Self {
        Self::from_wide(
            i128::from(self.num) * i128::from(o.den) + i128::from(o.num) * i128::from(self.den),
            i128::from(self.den) * i128::from(o.den),
        )
    }
}

impl Sub for Ratio {
    type Output = Self;
    fn sub(self, o: Self) -> Self {
        Self::from_wide(
            i128::from(self.num) * i128::from(o.den) - i128::from(o.num) * i128::from(self.den),
            i128::from(self.den) * i128::from(o.den),
        )
    }
}

impl Mul for Ratio {
    type Output = Self;
    fn mul(self, o: Self) -> Self {
        Self::from_wide(
            i128::from(self.num) * i128::from(o.num),
            i128::from(self.den) * i128::from(o.den),
        )
    }
}

impl Div for Ratio {
    type Output = Self;
    fn div(self, o: Self) -> Self {
        assert!(o.num != 0, "division by zero ratio");
        Self::from_wide(
            i128::from(self.num) * i128::from(o.den),
            i128::from(self.den) * i128::from(o.num),
        )
    }
}

impl PartialOrd for Ratio {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Ratio {
    fn cmp(&self, other: &Self) -> Ordering {
        (i128::from(self.num) * i128::from(other.den))
            .cmp(&(i128::from(other.num) * i128::from(self.den)))
    }
}

impl serde::Serialize for Ratio {
    /// Serializes as a JSON number in seconds; exactness is not preserved
    /// in this direction, which is fine for reporting.
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_f64(self.to_f64())
    }
}

impl fmt::Display for Ratio {
    /// Formats as seconds with up to six decimals, trimming trailing zeros,
    /// so that `3/2` prints as `1.5` and `1/3` as `0.333333`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.den == 1 {
            return write!(f, "{}", self.num);
        }
        let s = format!("{:.6}", self.to_f64());
        let s = s.trim_end_matches('0').trim_end_matches('.');
        f.write_str(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reduces_and_normalizes_sign() {
        assert_eq!(Ratio::new(6, 4), Ratio::new(3, 2));
        assert_eq!(Ratio::new(3, -2), Ratio::new(-3, 2));
        assert_eq!(Ratio::new(0, -5), Ratio::ZERO);
    }

    #[test]
    fn arithmetic_is_exact() {
        let frame = Ratio::new(1001, 30000);
        let mut t = Ratio::ZERO;
        for _ in 0..30000 {
            t = t + frame;
        }
        assert_eq!(t, Ratio::from_int(1001));
        assert_eq!(Ratio::new(1, 3) * Ratio::from_int(3), Ratio::ONE);
        assert_eq!(Ratio::ONE / Ratio::new(1, 3), Ratio::from_int(3));
    }

    #[test]
    fn rounding() {
        assert_eq!(Ratio::new(-1, 2).floor(), -1);
        assert_eq!(Ratio::new(-1, 2).ceil(), 0);
        assert_eq!(Ratio::new(5, 2).round(), 3);
        assert_eq!(Ratio::new(-5, 2).round(), -2);
    }

    #[test]
    fn ordering() {
        assert!(Ratio::new(1, 3) < Ratio::new(1, 2));
        assert!(Ratio::new(-1, 3) < Ratio::ZERO);
        assert_eq!(Ratio::new(2, 4).cmp(&Ratio::new(1, 2)), Ordering::Equal);
    }

    #[test]
    fn display_is_compact() {
        assert_eq!(Ratio::new(3, 2).to_string(), "1.5");
        assert_eq!(Ratio::from_int(4).to_string(), "4");
        assert_eq!(Ratio::new(1, 3).to_string(), "0.333333");
    }
}
