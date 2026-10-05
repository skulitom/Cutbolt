use crate::{Result, error};
use serde::{Deserialize, Deserializer, Serialize, de};
use std::{borrow::Cow, cmp::Ordering, fmt};

const MAX: u128 = 9_007_199_254_740_991;

/// Every form a time may be written in; rejections quote it.
const EXPECTED: &str = "an exact nonnegative time: whole seconds (3), a decimal (2.5), a string \"5/2\", \"2.5\" or \"3\", or {\"num\":5,\"den\":2}, with the reduced numerator and positive denominator at most 9007199254740991";

/// Exact nonnegative rational seconds `num/den` (or a rational rate such as frames per second);
/// `{"num":1,"den":25}` is one frame at 25 fps. Always serialized as `{"num":N,"den":D}`.
/// Deserialization also takes the exact literals in [`EXPECTED`], which arrive reduced and in
/// range; the object form is kept as written and validated where it is used.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
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

impl<'de> Deserialize<'de> for Time {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        deserializer.deserialize_any(TimeVisitor)
    }
}

struct TimeVisitor;

impl<'de> de::Visitor<'de> for TimeVisitor {
    type Value = Time;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(EXPECTED)
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<Time, E> {
        Time::reduced(v.into(), 1).map_err(|_| E::invalid_value(de::Unexpected::Unsigned(v), &self))
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> std::result::Result<Time, E> {
        let whole =
            u64::try_from(v).map_err(|_| E::invalid_value(de::Unexpected::Signed(v), &self))?;
        self.visit_u64(whole)
    }

    /// A JSON number stands for the shortest decimal that round-trips its `f64` (`0.1` reads as
    /// "0.1", so 1/10), taken exactly; anything that decimal cannot hold exactly is rejected.
    /// Only up to 15 significant digits survive the `f64` itself; strings are always exact.
    fn visit_f64<E: de::Error>(self, v: f64) -> std::result::Result<Time, E> {
        // `abs` only turns -0 into 0; negative values fail the sign check first.
        let exact = if v.is_finite() && v >= 0.0 {
            decimal(&v.abs().to_string())
        } else {
            None
        };
        exact.ok_or_else(|| E::invalid_value(de::Unexpected::Float(v), &self))
    }

    fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Time, E> {
        literal(v).ok_or_else(|| E::invalid_value(de::Unexpected::Str(v), &self))
    }

    /// The object form, accepted exactly as the derived implementation did.
    fn visit_map<A: de::MapAccess<'de>>(self, mut map: A) -> std::result::Result<Time, A::Error> {
        let (mut num, mut den) = (None, None);
        while let Some(key) = map.next_key::<Cow<'_, str>>()? {
            let (field, slot) = match key.as_ref() {
                "num" => ("num", &mut num),
                "den" => ("den", &mut den),
                other => return Err(de::Error::unknown_field(other, &["num", "den"])),
            };
            if slot.is_some() {
                return Err(de::Error::duplicate_field(field));
            }
            *slot = Some(map.next_value::<u64>()?);
        }
        Ok(Time {
            num: num.ok_or_else(|| de::Error::missing_field("num"))?,
            den: den.ok_or_else(|| de::Error::missing_field("den"))?,
        })
    }
}

/// Exact value of a string literal: a rational `"5/2"`, a decimal `"2.5"` or an integer `"3"`.
fn literal(text: &str) -> Option<Time> {
    match text.split_once('/') {
        Some((num, den)) if digits(num) && digits(den) => {
            Time::reduced(num.parse().ok()?, den.parse().ok()?).ok()
        }
        Some(_) => None,
        None => decimal(text),
    }
}

/// Exact value of `digits` with an optional `.digits` fraction, reduced. Without trailing zeros
/// the `k` fraction digits `f` share factors of 2 or of 5 with `10^k`, never both, so `f/10^k`
/// keeps a denominator of at least `2^k`: past 52 fraction digits nothing is in range.
fn decimal(text: &str) -> Option<Time> {
    let (whole, fraction) = text.split_once('.').unwrap_or((text, "0"));
    if !digits(whole) || !digits(fraction) {
        return None;
    }
    let whole: u128 = whole.parse().ok().filter(|w| *w <= MAX)?;
    let fraction = fraction.trim_end_matches('0');
    let k = u32::try_from(fraction.len()).ok().filter(|k| *k <= 52)?;
    let (twos, fives) = (multiplicity(fraction, 2, k), multiplicity(fraction, 5, k));
    let den = 2u128
        .pow(k - twos)
        .checked_mul(5u128.pow(k - fives))
        .filter(|d| *d <= MAX)?;
    let (part, _) = divide(fraction, 2u128.pow(twos) * 5u128.pow(fives));
    Time::reduced(whole * den + part, den).ok()
}

/// Nonempty ASCII digits only: no sign, space, exponent or other script's digits.
fn digits(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())
}

/// Largest `j <= k` such that `prime^j` divides the decimal integer `digits`.
fn multiplicity(digits: &str, prime: u128, k: u32) -> u32 {
    let (_, remainder) = divide(digits, prime.pow(k));
    (0..k)
        .take_while(|j| remainder % prime.pow(j + 1) == 0)
        .count() as u32
}

/// Quotient and remainder of the decimal integer `digits` (at most 52 of them) by `divisor`,
/// by long division. Callers keep the quotient below 5^52 and the divisor at most 5^52, so
/// nothing here exceeds u128.
fn divide(digits: &str, divisor: u128) -> (u128, u128) {
    digits.bytes().fold((0, 0), |(quotient, remainder), digit| {
        let partial = remainder * 10 + u128::from(digit - b'0');
        (quotient * 10 + partial / divisor, partial % divisor)
    })
}

impl schemars::JsonSchema for Time {
    fn schema_name() -> Cow<'static, str> {
        "Time".into()
    }

    fn schema_id() -> Cow<'static, str> {
        concat!(module_path!(), "::Time").into()
    }

    /// One compact schema for every accepted form, since most tool listings embed it.
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "description": "Exact seconds or rate: 2.5, \"30000/1001\" or {\"num\":5,\"den\":2}.",
            "type": ["object", "integer", "number", "string"],
            "minimum": 0,
            "pattern": "^[0-9]+([./][0-9]+)?$",
            "properties": {
                "num": {"type": "integer", "minimum": 0, "description": "Numerator."},
                "den": {"type": "integer", "minimum": 1, "description": "Positive denominator."}
            },
            "required": ["num", "den"],
            "additionalProperties": false
        })
    }
}

/// Reduced exact value for messages: `18` or `49/25`; callers add the unit.
impl std::fmt::Display for Time {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (mut a, mut b) = (self.num, self.den);
        while b != 0 {
            (a, b) = (b, a % b);
        }
        let (num, den) = match a {
            0 => (self.num, self.den),
            _ => (self.num / a, self.den / a),
        };
        if den == 1 {
            write!(f, "{num}")
        } else {
            write!(f, "{num}/{den}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::de::IntoDeserializer;
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
        assert_eq!(Time { num: 36, den: 2 }.to_string(), "18");
        assert_eq!(Time { num: 98, den: 50 }.to_string(), "49/25");
        assert_eq!(Time { num: 0, den: 7 }.to_string(), "0");
        let max = Time::new(MAX as u64, 1).unwrap();
        assert!(max.times(Time::new(2, 1).unwrap()).is_err());
        assert_eq!(
            max.times(Time::new(1, MAX as u64).unwrap()).unwrap(),
            Time::new(1, 1).unwrap()
        );
    }

    /// Reads `json` directly, through a `Value`, and through the buffered content of a tagged
    /// enum (as operations are), requiring all three to agree.
    fn read(json: &str) -> std::result::Result<Time, String> {
        #[derive(Deserialize)]
        #[serde(tag = "kind")]
        enum Tagged {
            At { at: Time },
        }
        let direct = serde_json::from_str::<Time>(json).map_err(|e| e.to_string());
        let value: serde_json::Value = serde_json::from_str(json).unwrap();
        let via_value = serde_json::from_value::<Time>(value.clone()).map_err(|e| e.to_string());
        let tagged =
            serde_json::from_value::<Tagged>(serde_json::json!({"kind": "At", "at": value}))
                .map(|Tagged::At { at }| at)
                .map_err(|e| e.to_string());
        assert_eq!(direct.as_ref().ok(), via_value.as_ref().ok(), "{json}");
        assert_eq!(direct.as_ref().ok(), tagged.as_ref().ok(), "{json}");
        direct
    }

    #[test]
    fn accepts_exact_literals_and_the_object_form() {
        let t = |num, den| Time { num, den };
        let max = MAX as u64;
        for (json, expected) in [
            // Whole seconds and JSON numbers, via their shortest round-trip decimal.
            ("3", t(3, 1)),
            ("0", t(0, 1)),
            ("9007199254740991", t(max, 1)),
            ("2.5", t(5, 2)),
            ("0.1", t(1, 10)),
            ("3.0", t(3, 1)),
            ("1e2", t(100, 1)),
            ("1.5e-1", t(3, 20)),
            ("-0", t(0, 1)),
            ("-0.0", t(0, 1)),
            ("123456789012.345", t(24691357802469, 200)),
            // String rationals, reduced.
            (r#""5/2""#, t(5, 2)),
            (r#""30000/1001""#, t(30000, 1001)),
            (r#""10/4""#, t(5, 2)),
            (r#""0/7""#, t(0, 1)),
            (r#""18014398509481982/2""#, t(max, 1)),
            // String decimals and integers, exact.
            (r#""2.5""#, t(5, 2)),
            (r#""2.50""#, t(5, 2)),
            (r#""0.04""#, t(1, 25)),
            (r#""3.000""#, t(3, 1)),
            (r#""4503599627370495.5""#, t(max, 2)),
            (
                r#""0.0000000000000002220446049250313080847263336181640625""#,
                t(1, 1 << 52),
            ),
            (r#""3""#, t(3, 1)),
            (r#""007""#, t(7, 1)),
            // The object form is unchanged: kept as written, validated where it is used.
            (r#"{"num":5,"den":2}"#, t(5, 2)),
            (r#"{"den":2,"num":5}"#, t(5, 2)),
            (r#"{"num":2,"den":4}"#, t(2, 4)),
            (r#"{"num":1,"den":0}"#, t(1, 0)),
        ] {
            assert_eq!(read(json), Ok(expected), "{json}");
        }
    }

    #[test]
    fn rejects_inexact_negative_malformed_and_out_of_range_literals() {
        for json in [
            // Negative.
            "-1",
            "-2.5",
            r#""-1""#,
            r#""-1/2""#,
            // Zero denominators.
            r#""1/0""#,
            r#""0/0""#,
            // Not a plain exact literal.
            r#""abc""#,
            r#""""#,
            r#""NaN""#,
            r#""nan""#,
            r#""inf""#,
            r#""-Infinity""#,
            r#""1e3""#,
            r#""0x10""#,
            r#""+2""#,
            r#"" 2""#,
            r#""2 ""#,
            r#""2.""#,
            r#"".5""#,
            r#""1/2/3""#,
            r#""1.5/2""#,
            r#""1/""#,
            "\"\u{663}\"",
            // Beyond 2^53-1 once reduced, or not exact within it.
            "9007199254740992",
            "18446744073709551616",
            "1e300",
            "5e-324",
            "0.30000000000000004",
            "2.220446049250313e-16",
            r#""9007199254740992""#,
            r#""1/9007199254740992""#,
            r#""9007199254740990.5""#,
            r#""0.0000000000000001""#,
            r#""0.00000000000000011102230246251565404236316680908203125""#,
            r#""99999999999999999999999999999999999999999/3""#,
        ] {
            let message = read(json).unwrap_err();
            assert!(message.starts_with("invalid value: "), "{json}: {message}");
            assert!(message.contains(EXPECTED), "{json}: {message}");
        }
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.5] {
            let parsed: std::result::Result<Time, de::value::Error> =
                Time::deserialize(value.into_deserializer());
            assert!(parsed.is_err(), "{value}");
        }
        for json in ["null", "true", "[5,2]"] {
            assert!(read(json).unwrap_err().contains(EXPECTED), "{json}");
        }
        for (json, message) in [
            (
                r#"{"num":1,"den":2,"extra":0}"#,
                "unknown field `extra`, expected `num` or `den`",
            ),
            (r#"{"num":1,"num":2,"den":1}"#, "duplicate field `num`"),
            (r#"{"num":1}"#, "missing field `den`"),
            (r#"{"num":"1","den":2}"#, "invalid type: string"),
            (r#"{"num":-1,"den":2}"#, "invalid value: integer `-1`"),
            (r#"{"num":1.5,"den":2}"#, "invalid type: floating point"),
        ] {
            let error = serde_json::from_str::<Time>(json).unwrap_err().to_string();
            assert!(error.contains(message), "{json}: {error}");
        }
    }

    #[test]
    fn serializes_only_the_object_form() {
        for json in ["2.5", r#""5/2""#, r#""2.5""#, r#"{"num":5,"den":2}"#] {
            let time = read(json).unwrap();
            assert_eq!(
                serde_json::to_string(&time).unwrap(),
                r#"{"num":5,"den":2}"#
            );
        }
        let unreduced = read(r#"{"num":2,"den":4}"#).unwrap();
        assert_eq!(
            serde_json::to_string(&unreduced).unwrap(),
            r#"{"num":2,"den":4}"#
        );
    }
}
