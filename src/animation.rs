//! Original bounded rational property sampling for pixel scenes.
use crate::{Result, error, time::Time};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

/// Interpolation from a key to the next: `hold` keeps the value, `linear`, or quadratic `ease_in`, `ease_out`, `ease_in_out` without overshoot.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Interpolation {
    Hold,
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
}

/// One curve key.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Keyframe {
    /// Layer-local rational seconds, 0..layer duration; reduced denominator at most 1,000,000.
    pub time: Time,
    /// Integer property value within the animated property's range.
    pub value: i32,
    /// Interpolation toward the next key; unused on the last key.
    pub interpolation: Interpolation,
}

/// Keyframed integer property on the layer-local clock. Endpoint values hold outside the keys; samples round to nearest.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Curve {
    /// 1..128 keys in any order; equal times, including equivalent fractions, are rejected.
    pub keys: Vec<Keyframe>,
    /// Optional delay, speed and direction for evaluating the keys; omit to use key times as authored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retime: Option<Retime>,
}

/// Changes when and how fast a curve's keys play, without moving the layer or retiming source frames.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Retime {
    /// Layer-local rational seconds, at most the layer duration; until then the first key holds (last when reversed).
    pub start: Time,
    /// Positive rational speed multiplier, 1/16..16; denominators reduce to at most 1,000,000.
    pub rate: Time,
    /// Play the keys from last to first; default false.
    #[serde(default)]
    pub reverse: bool,
}

pub(crate) struct Sampler {
    keys: Vec<Keyframe>,
    retime: Option<Retime>,
}

fn invalid(message: &str) -> crate::Error {
    error("INVALID_ANIMATION", message)
}

impl Curve {
    pub(crate) fn prepare(&self, duration: Time, minimum: i32, maximum: i32) -> Result<Sampler> {
        if self.keys.is_empty() || self.keys.len() > 128 {
            return Err(invalid("Each curve requires 1-128 keys"));
        }
        let mut keys = self.keys.clone();
        for key in &mut keys {
            key.time.validate()?;
            key.time = Time::new(key.time.num, key.time.den)?;
            if key.time.den > 1_000_000
                || key.time.compare(duration)? == Ordering::Greater
                || !(minimum..=maximum).contains(&key.value)
            {
                return Err(invalid(
                    "Keys must lie within layer duration, use reduced denominators <=1000000 and bounded property values",
                ));
            }
        }
        keys.sort_by(|a, b| a.time.compare(b.time).expect("validated key times"));
        if keys.windows(2).any(|pair| pair[0].time == pair[1].time) {
            return Err(invalid(
                "Equal-time keys are ambiguous, including equivalent rational times",
            ));
        }
        if let Some(retime) = self.retime {
            for time in [retime.start, retime.rate] {
                time.validate()?;
                if Time::new(time.num, time.den)?.den > 1_000_000 {
                    return Err(invalid("Retime denominators must reduce to <=1000000"));
                }
            }
            if retime.start.compare(duration)? == Ordering::Greater
                || retime.rate.compare(Time::new(1, 16)?)? == Ordering::Less
                || retime.rate.compare(Time::new(16, 1)?)? == Ordering::Greater
            {
                return Err(invalid(
                    "Retime start must fit the layer and rate must be 1/16..16",
                ));
            }
        }
        Ok(Sampler {
            keys,
            retime: self.retime,
        })
    }
}

fn overflow() -> crate::Error {
    error(
        "TIME_OVERFLOW",
        "Property evaluation exceeds exact integer precision; use coarser rational times",
    )
}

fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

fn scaled_time(time: Time, rate: Time) -> Result<Time> {
    let n = time.num as u128 * rate.num as u128;
    let d = time.den as u128 * rate.den as u128;
    let g = gcd(n, d);
    Time::new(
        u64::try_from(n / g).map_err(|_| overflow())?,
        u64::try_from(d / g).map_err(|_| overflow())?,
    )
}

/// Quadratic easing keeps weights bounded, with no overshoot or floating-point state.
fn ease(n: i128, d: i128, mode: Interpolation) -> Result<(i128, i128)> {
    if matches!(mode, Interpolation::Linear) {
        return Ok((n, d));
    }
    let square = |v: i128| v.checked_mul(v).ok_or_else(overflow);
    let denominator = square(d)?;
    let numerator = match mode {
        Interpolation::EaseIn => square(n)?,
        Interpolation::EaseOut => denominator - square(d - n)?,
        Interpolation::EaseInOut if n <= d / 2 => square(n)?.checked_mul(2).ok_or_else(overflow)?,
        Interpolation::EaseInOut => {
            denominator - square(d - n)?.checked_mul(2).ok_or_else(overflow)?
        }
        _ => unreachable!("hold is sampled before easing"),
    };
    let g = gcd(numerator as u128, denominator as u128) as i128;
    Ok((numerator / g, denominator / g))
}

impl Sampler {
    pub(crate) fn sample(&self, time: Time) -> Result<i32> {
        time.validate()?;
        let first = &self.keys[0];
        let last = &self.keys[self.keys.len() - 1];
        let time = if let Some(retime) = self.retime {
            if time.compare(retime.start)? != Ordering::Greater {
                return Ok(if retime.reverse {
                    last.value
                } else {
                    first.value
                });
            }
            let elapsed = scaled_time(time.minus(retime.start)?, retime.rate)?;
            if elapsed.compare(last.time.minus(first.time)?)? != Ordering::Less {
                return Ok(if retime.reverse {
                    first.value
                } else {
                    last.value
                });
            }
            if retime.reverse {
                last.time.minus(elapsed)?
            } else {
                first.time.plus(elapsed)?
            }
        } else {
            time
        };
        if time.compare(first.time)? != Ordering::Greater {
            return Ok(first.value);
        }
        if time.compare(last.time)? != Ordering::Less {
            return Ok(last.value);
        }
        let right = self.keys.partition_point(|key| {
            key.time.compare(time).expect("validated sample time") != Ordering::Greater
        });
        let a = &self.keys[right - 1];
        let b = &self.keys[right];
        if matches!(a.interpolation, Interpolation::Hold) {
            return Ok(a.value);
        }
        let elapsed = time.minus(a.time)?;
        let span = b.time.minus(a.time)?;
        let numerator = elapsed.num as i128 * span.den as i128;
        let denominator = elapsed.den as i128 * span.num as i128;
        let g = gcd(numerator as u128, denominator as u128) as i128;
        let (numerator, denominator) = ease(numerator / g, denominator / g, a.interpolation)?;
        let weighted = (a.value as i128)
            .checked_mul(denominator - numerator)
            .and_then(|left| {
                (b.value as i128)
                    .checked_mul(numerator)
                    .and_then(|right| left.checked_add(right))
            })
            .ok_or_else(overflow)?;
        let rounded = weighted
            .checked_abs()
            .ok_or_else(overflow)?
            .checked_add(denominator / 2)
            .ok_or_else(overflow)?
            / denominator
            * weighted.signum();
        i32::try_from(rounded).map_err(|_| overflow())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(n: u64, d: u64, value: i32, interpolation: Interpolation) -> Keyframe {
        Keyframe {
            time: Time::new(n, d).unwrap(),
            value,
            interpolation,
        }
    }

    #[test]
    fn unordered_keys_boundaries_holds_and_random_access() {
        let curve = Curve {
            retime: None,
            keys: vec![
                key(3, 4, 10, Interpolation::Linear),
                key(1, 4, -10, Interpolation::Hold),
                key(1, 2, 0, Interpolation::Linear),
            ],
        }
        .prepare(Time::new(1, 1).unwrap(), -32768, 32768)
        .unwrap();
        for (n, d, value) in [
            (5, 8, 5),
            (0, 1, -10),
            (1, 1, 10),
            (1, 2, 0),
            (3, 8, -10),
            (5, 8, 5),
        ] {
            assert_eq!(curve.sample(Time::new(n, d).unwrap()).unwrap(), value);
        }
    }

    #[test]
    fn signed_half_ties_round_away_from_zero() {
        for (a, b, expected) in [(-1, 0, -1), (0, 1, 1), (1, 0, 1), (0, -1, -1)] {
            let curve = Curve {
                retime: None,
                keys: vec![
                    key(0, 1, a, Interpolation::Linear),
                    key(2, 25, b, Interpolation::Hold),
                ],
            }
            .prepare(Time::new(1, 1).unwrap(), -32768, 32768)
            .unwrap();
            assert_eq!(curve.sample(Time::new(1, 25).unwrap()).unwrap(), expected);
        }
    }

    #[test]
    fn rejects_ambiguous_or_unbounded_curves() {
        for keys in [
            vec![],
            vec![
                key(1, 2, 1, Interpolation::Hold),
                key(2, 4, 2, Interpolation::Linear),
            ],
            vec![key(2, 1, 1, Interpolation::Hold)],
            vec![key(0, 1, 256, Interpolation::Hold)],
            vec![key(1, 1_000_001, 1, Interpolation::Hold)],
            vec![key(0, 1, 1, Interpolation::Hold); 129],
        ] {
            assert!(
                Curve { keys, retime: None }
                    .prepare(Time::new(1, 1).unwrap(), 0, 255)
                    .is_err()
            );
        }
    }

    #[test]
    fn quadratic_easing_has_exact_landmarks_and_no_overshoot() {
        for (mode, expected) in [
            (Interpolation::EaseIn, [0, 10, 40, 90, 160]),
            (Interpolation::EaseOut, [0, 70, 120, 150, 160]),
            (Interpolation::EaseInOut, [0, 20, 80, 140, 160]),
        ] {
            for sign in [-1, 1] {
                let sampler = Curve {
                    keys: vec![
                        key(0, 1, 0, mode),
                        key(1, 1, sign * 160, Interpolation::Hold),
                    ],
                    retime: None,
                }
                .prepare(Time::new(1, 1).unwrap(), -32768, 32768)
                .unwrap();
                // Deliberately shuffled, repeated reads expose accidental evaluation state.
                for n in [4, 1, 3, 0, 2, 1] {
                    assert_eq!(
                        sampler.sample(Time::new(n, 4).unwrap()).unwrap(),
                        sign * expected[n as usize]
                    );
                }
            }
        }
    }

    #[test]
    fn retiming_preserves_reverse_hold_boundaries_and_endpoint_clamps() {
        let sampler = Curve {
            keys: vec![
                key(1, 5, 10, Interpolation::Hold),
                key(3, 5, 50, Interpolation::Linear),
                key(1, 1, 90, Interpolation::Hold),
            ],
            retime: Some(Retime {
                start: Time::new(1, 5).unwrap(),
                rate: Time::new(2, 1).unwrap(),
                reverse: true,
            }),
        }
        .prepare(Time::new(2, 1).unwrap(), 0, 255)
        .unwrap();
        for (n, d, expected) in [
            (0, 1, 90),
            (1, 5, 90),
            (3, 10, 70),
            (2, 5, 50),
            (41, 100, 10),
            (3, 5, 10),
            (2, 1, 10),
        ] {
            assert_eq!(sampler.sample(Time::new(n, d).unwrap()).unwrap(), expected);
        }
        let constant = Curve {
            keys: vec![key(1, 2, 34, Interpolation::EaseInOut)],
            retime: Some(Retime {
                start: Time::ZERO,
                rate: Time::new(1, 16).unwrap(),
                reverse: false,
            }),
        }
        .prepare(Time::new(2, 1).unwrap(), 0, 255)
        .unwrap();
        assert_eq!(constant.sample(Time::new(1, 1).unwrap()).unwrap(), 34);
    }

    #[test]
    fn retiming_rates_and_precision_fail_explicitly() {
        for (start, rate) in [
            (Time::ZERO, Time::ZERO),
            (Time::ZERO, Time::new(1, 17).unwrap()),
            (Time::ZERO, Time::new(17, 1).unwrap()),
            (Time::new(3, 1).unwrap(), Time::new(1, 1).unwrap()),
            (Time::new(1, 1_000_001).unwrap(), Time::new(1, 1).unwrap()),
        ] {
            assert!(
                Curve {
                    keys: vec![key(0, 1, 0, Interpolation::Linear)],
                    retime: Some(Retime {
                        start,
                        rate,
                        reverse: false
                    }),
                }
                .prepare(Time::new(2, 1).unwrap(), 0, 255)
                .is_err()
            );
        }
        assert!(ease(1, i128::MAX, Interpolation::EaseIn).is_err());
        // Valid JSON rationals can still exceed the exact evaluator's derived precision.
        let fine = Curve {
            keys: vec![
                key(1, 999983, 0, Interpolation::EaseIn),
                key(1, 1, 255, Interpolation::Hold),
            ],
            retime: Some(Retime {
                start: Time::new(1, 999979).unwrap(),
                rate: Time::new(999961, 999959).unwrap(),
                reverse: false,
            }),
        }
        .prepare(Time::new(2, 1).unwrap(), 0, 255)
        .unwrap();
        assert_eq!(
            fine.sample(Time::new(1, 25).unwrap()).unwrap_err().code,
            "TIME_OVERFLOW"
        );
    }
}
