//! Signed source timestamps only. Project/timeline `Time` remains nonnegative.
use super::Time;
use crate::{Result, error};
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Stamp {
    num: i64,
    den: u64,
}
impl Stamp {
    pub const ZERO: Self = Self { num: 0, den: 1 };
    fn parts(negative: bool, magnitude: Time) -> Result<Self> {
        magnitude.validate()?;
        Ok(Self {
            num: if negative {
                -(magnitude.num as i64)
            } else {
                magnitude.num as i64
            },
            den: magnitude.den,
        })
    }
    fn magnitude(self) -> Time {
        Time {
            num: self.num.unsigned_abs(),
            den: self.den,
        }
    }
    pub fn time(value: Time) -> Result<Self> {
        Self::parts(false, value)
    }
    pub fn ticks(value: i64, time_base: Time) -> Result<Self> {
        time_base.validate()?;
        Self::parts(
            value < 0,
            Time::reduced(
                value.unsigned_abs() as u128 * time_base.num as u128,
                time_base.den as u128,
            )?,
        )
    }
    pub fn plus(self, other: Self) -> Result<Self> {
        let (a, b) = (self.magnitude(), other.magnitude());
        if (self.num < 0) == (other.num < 0) {
            Self::parts(self.num < 0, a.plus(b)?)
        } else if a.compare(b)?.is_lt() {
            Self::parts(other.num < 0, b.minus(a)?)
        } else {
            Self::parts(self.num < 0, a.minus(b)?)
        }
    }
    pub fn minus(self, other: Self) -> Result<Self> {
        self.plus(Self {
            num: -other.num,
            ..other
        })
    }
    pub fn times(self, rate: Time) -> Result<Self> {
        Self::parts(self.num < 0, self.magnitude().times(rate)?)
    }
    pub fn nonnegative(self) -> Result<Time> {
        if self.num < 0 {
            return Err(error(
                "UNSUPPORTED_MEDIA",
                "Source timestamps move backwards before their origin",
            ));
        }
        Ok(self.magnitude())
    }
    pub fn compare(self, other: Self) -> Result<std::cmp::Ordering> {
        Ok(self.minus(other)?.num.cmp(&0))
    }
    /// FFmpeg's native PCM clock uses nearest-integer rescaling, with ties away from zero.
    pub fn nearest_integer(self) -> i64 {
        let magnitude = (self.num.unsigned_abs() + self.den / 2) / self.den;
        if self.num < 0 {
            -(magnitude as i64)
        } else {
            magnitude as i64
        }
    }
    /// Signed integer position plus a nonnegative fractional remainder; negative positions
    /// floor toward minus infinity, as required when interpolating on a shifted audio clock.
    pub fn sample(self) -> (i128, i128, i128) {
        let (n, d) = (self.num as i128, self.den as i128);
        (n.div_euclid(d), n.rem_euclid(d), d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shifted_sample_clocks_keep_signed_fractional_origins_exact() {
        let video = Stamp::ticks(400, Time::new(1, 1000).unwrap()).unwrap();
        let audio = Stamp::ticks(378, Time::new(1, 1000).unwrap()).unwrap();
        let offset = video
            .minus(audio)
            .unwrap()
            .times(Time::new(44100, 1).unwrap())
            .unwrap();
        assert_eq!(offset.sample(), (970, 1, 5));
        assert_eq!(
            audio
                .minus(video)
                .unwrap()
                .times(Time::new(44100, 1).unwrap())
                .unwrap()
                .sample(),
            (-971, 4, 5)
        );
        assert_eq!(offset.minus(offset).unwrap(), Stamp::ZERO);
        assert!(audio.minus(video).unwrap().nonnegative().is_err());
        let negative = Stamp::ticks(-1024, Time::new(1, 48000).unwrap()).unwrap();
        assert_eq!(
            negative
                .plus(Stamp::time(Time::new(8, 375).unwrap()).unwrap())
                .unwrap(),
            Stamp::ZERO
        );
        assert!(Stamp::ticks(i64::MIN, Time::new(1, 1).unwrap()).is_err());
        assert_eq!(
            Stamp::ticks(-25, Time::new(1, 1000).unwrap())
                .unwrap()
                .times(Time::new(44100, 1).unwrap())
                .unwrap()
                .nearest_integer(),
            -1103
        );
    }
}
