use std::fmt;

/// An exact fraction, used for time bases. Always stored reduced with a
/// positive denominator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rational {
    num: i64,
    den: i64,
}

fn gcd(mut a: i64, mut b: i64) -> i64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a.abs()
}

impl Rational {
    /// Panics if `den` is zero.
    pub fn new(num: i64, den: i64) -> Self {
        assert!(den != 0, "Rational with zero denominator");
        let g = gcd(num, den).max(1);
        let sign = if den < 0 { -1 } else { 1 };
        Self {
            num: sign * num / g,
            den: sign * den / g,
        }
    }

    pub fn num(self) -> i64 {
        self.num
    }

    pub fn den(self) -> i64 {
        self.den
    }

    pub fn as_f64(self) -> f64 {
        self.num as f64 / self.den as f64
    }

    /// Converts `value` expressed in `from` units to `to` units, rounding to
    /// nearest with ties away from zero (FFmpeg's `av_rescale_q` default).
    /// Saturates instead of overflowing.
    pub fn rescale(value: i64, from: Rational, to: Rational) -> i64 {
        let n = value as i128 * from.num as i128 * to.den as i128;
        let d = from.den as i128 * to.num as i128;
        let half = d.abs() / 2;
        let q = if (n < 0) == (d < 0) {
            (n.abs() + half) / d.abs()
        } else {
            -((n.abs() + half) / d.abs())
        };
        q.clamp(i64::MIN as i128, i64::MAX as i128) as i64
    }
}

impl fmt::Display for Rational {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.num, self.den)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reduces_and_normalises_sign() {
        assert_eq!(
            Rational::new(1_000_000, 1_000_000_000),
            Rational::new(1, 1000)
        );
        assert_eq!(Rational::new(3, -6).to_string(), "-1/2");
    }

    #[test]
    fn rescale_rounds_half_away_from_zero() {
        let ns = Rational::new(1, 1_000_000_000);
        let ms = Rational::new(1, 1000);
        assert_eq!(Rational::rescale(6_500_000, ns, ms), 7);
        assert_eq!(Rational::rescale(-6_500_000, ns, ms), -7);
        assert_eq!(Rational::rescale(6_499_999, ns, ms), 6);
    }
}
