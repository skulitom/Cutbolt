use crate::{Result, error};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

const MAX: u128 = 9_007_199_254_740_991;

/// Exact nonnegative rational seconds `num/den` (or a rational rate such as frames per second); `{"num":1,"den":25}` is one frame at 25 fps.
#[derive(schemars::JsonSchema, Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Time {
    /// Numerator, 0 to 9007199254740991 (2^53-1).
    pub num: u64,
    /// Positive denominator, at most 9007199254740991 (2^53-1).
    pub den: u64,
}

impl Time {
    pub const ZERO: Self = Self { num: 0, den: 1 };
    pub fn new(num: u64, den: u64) -> Result<Self> {
        Self::reduced(num as u128, den as u128)
    }
    fn reduced(mut num: u128, mut den: u128) -> Result<Self> {
        if den == 0 {
            return Err(error("INVALID_TIME", "Denominator must be positive"));
        }
        let (mut a, mut b) = (num, den);
        while b != 0 {
            (a, b) = (b, a % b);
        }
        num /= a;
        den /= a;
        if num > MAX || den > MAX {
            return Err(error(
                "TIME_OVERFLOW",
                "Time exceeds exact JSON integer range",
            ));
        }
        Ok(Self {
            num: num as u64,
            den: den as u64,
        })
    }
    pub fn validate(self) -> Result<()> {
        if self.num as u128 > MAX || self.den as u128 > MAX || self.den == 0 {
            return Err(error(
                "INVALID_TIME",
                "Time must use safe nonnegative integers and a positive denominator",
            ));
        }
        Ok(())
    }
    pub fn plus(self, other: Self) -> Result<Self> {
        self.validate()?;
        other.validate()?;
        Self::reduced(
            self.num as u128 * other.den as u128 + other.num as u128 * self.den as u128,
            self.den as u128 * other.den as u128,
        )
    }
    pub fn times(self, other: Self) -> Result<Self> {
        self.validate()?;
        other.validate()?;
        Self::reduced(
            self.num as u128 * other.num as u128,
            self.den as u128 * other.den as u128,
        )
    }
    pub fn minus(self, other: Self) -> Result<Self> {
        self.validate()?;
        other.validate()?;
        let a = self.num as u128 * other.den as u128;
        let b = other.num as u128 * self.den as u128;
        if a < b {
            return Err(error("INVALID_RANGE", "Time cannot be negative"));
        }
        Self::reduced(a - b, self.den as u128 * other.den as u128)
    }
    pub fn compare(self, other: Self) -> Result<Ordering> {
        self.validate()?;
        other.validate()?;
        Ok((self.num as u128 * other.den as u128).cmp(&(other.num as u128 * self.den as u128)))
    }
    pub fn units(self, rate: Self) -> Result<u64> {
        self.validate()?;
        rate.validate()?;
        if rate.num == 0 {
            return Err(error("INVALID_TIME", "Rate must be positive"));
        }
        let n = self.num as u128 * rate.num as u128;
        let d = self.den as u128 * rate.den as u128;
        if !n.is_multiple_of(d) {
            let (mut a, mut b) = (n, d);
            while b != 0 {
                (a, b) = (b, a % b);
            }
            return Err(error(
                "UNALIGNED_TIME",
                format!(
                    "Time does not fall on an exact frame/sample boundary: {}/{} s is {}/{} units at {}/{} per second",
                    self.num,
                    self.den,
                    n / a,
                    d / a,
                    rate.num,
                    rate.den
                ),
            ));
        }
        u64::try_from(n / d).map_err(|_| error("TIME_OVERFLOW", "Unit count overflow"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fractional_rates_and_long_sequences_are_exact() {
        let frame = Time::new(1001, 30000).unwrap();
        let rate = Time::new(30000, 1001).unwrap();
        let mut total = Time::ZERO;
        for _ in 0..100_000 {
            total = total.plus(frame).unwrap();
        }
        assert_eq!(total.units(rate).unwrap(), 100_000);
        assert_eq!(
            Time::new(1001, 24000)
                .unwrap()
                .units(Time::new(24000, 1001).unwrap())
                .unwrap(),
            1
        );
    }
    #[test]
    fn rejects_invalid_overflow_and_unaligned_times() {
        assert!(Time::new(1, 0).is_err());
        assert!(Time::ZERO.minus(Time::new(1, 1).unwrap()).is_err());
        assert!(
            Time::new(MAX as u64, 1)
                .unwrap()
                .plus(Time::new(1, 1).unwrap())
                .is_err()
        );
        assert!(
            Time::new(1, 1000)
                .unwrap()
                .units(Time::new(25, 1).unwrap())
                .is_err()
        );
        assert!(
            Time {
                num: u64::MAX,
                den: 1
            }
            .validate()
            .is_err()
        );
        assert_eq!(Time::new(2, 4).unwrap(), Time::new(1, 2).unwrap());
        let max = Time::new(MAX as u64, 1).unwrap();
        assert!(max.times(Time::new(2, 1).unwrap()).is_err());
        assert_eq!(
            max.times(Time::new(1, MAX as u64).unwrap()).unwrap(),
            Time::new(1, 1).unwrap()
        );
    }
}
