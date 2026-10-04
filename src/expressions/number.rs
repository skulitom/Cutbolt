//! Exact bounded signed property values; timeline clocks remain rational too.
use crate::{Result, error};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Number {
    pub num: i64,
    pub den: u64,
}

fn precision() -> crate::Error {
    error(
        "EXPRESSION_PRECISION",
        "Expression result exceeds the bounded exact rational profile",
    )
}

impl Number {
    pub(crate) fn integer(value: i32) -> Self {
        Self {
            num: value as i64,
            den: 1,
        }
    }
    pub(crate) fn make(num: i128, den: u128) -> Result<Self> {
        if den == 0 {
            return Err(error(
                "EXPRESSION_DOMAIN",
                "A rational denominator or divisor is zero",
            ));
        }
        let (mut a, mut b) = (num.unsigned_abs(), den);
        while b != 0 {
            (a, b) = (b, a % b);
        }
        let divisor = i128::try_from(a).map_err(|_| precision())?;
        let num = num / divisor;
        let den = den / a;
        if num.unsigned_abs() > 9_007_199_254_740_991 || den > 1_000_000_000_000 {
            return Err(precision());
        }
        Ok(Self {
            num: num as i64,
            den: den as u64,
        })
    }
    pub(crate) fn normalized(self) -> Result<Self> {
        Self::make(self.num as i128, self.den as u128)
    }
    pub(crate) fn add(self, rhs: Self) -> Result<Self> {
        Self::make(
            self.num as i128 * rhs.den as i128 + rhs.num as i128 * self.den as i128,
            self.den as u128 * rhs.den as u128,
        )
    }
    pub(crate) fn sub(self, rhs: Self) -> Result<Self> {
        Self::make(
            self.num as i128 * rhs.den as i128 - rhs.num as i128 * self.den as i128,
            self.den as u128 * rhs.den as u128,
        )
    }
    pub(crate) fn mul(self, rhs: Self) -> Result<Self> {
        Self::make(
            self.num as i128 * rhs.num as i128,
            self.den as u128 * rhs.den as u128,
        )
    }
    pub(crate) fn div(self, rhs: Self) -> Result<Self> {
        if rhs.num == 0 {
            return Err(error("EXPRESSION_DOMAIN", "Division by zero"));
        }
        Self::make(
            self.num as i128 * rhs.den as i128 * rhs.num.signum() as i128,
            self.den as u128 * rhs.num.unsigned_abs() as u128,
        )
    }
    pub(crate) fn less(self, rhs: Self) -> bool {
        (self.num as i128) * (rhs.den as i128) < (rhs.num as i128) * (self.den as i128)
    }
    pub(crate) fn floor(self) -> Result<Self> {
        Self::make((self.num as i128).div_euclid(self.den as i128), 1)
    }
    pub(crate) fn modulo(self, rhs: Self) -> Result<Self> {
        if rhs.num <= 0 {
            return Err(error(
                "EXPRESSION_DOMAIN",
                "Modulo requires a positive divisor",
            ));
        }
        self.sub(rhs.mul(self.div(rhs)?.floor()?)?)
    }
    pub(crate) fn rounded(self, minimum: i32, maximum: i32) -> Result<i32> {
        // Bounds apply before rounding, so rounding cannot conceal an excursion.
        if self.less(Self::integer(minimum)) || Self::integer(maximum).less(self) {
            return Err(error(
                "EXPRESSION_RANGE",
                "Bound expression is outside the declared property range",
            ));
        }
        let magnitude =
            (self.num.unsigned_abs() as u128 * 2 + self.den as u128) / (self.den as u128 * 2);
        Ok((magnitude as i64 * self.num.signum()) as i32)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Scalar,
    Vector2,
    Boolean,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Value {
    Scalar(Number),
    Vector2([Number; 2]),
    Boolean(bool),
}

impl Value {
    pub(crate) fn kind(&self) -> Kind {
        match self {
            Self::Scalar(_) => Kind::Scalar,
            Self::Vector2(_) => Kind::Vector2,
            Self::Boolean(_) => Kind::Boolean,
        }
    }
    pub(crate) fn normalized(&self) -> Result<Self> {
        Ok(match self {
            Self::Scalar(n) => Self::Scalar(n.normalized()?),
            Self::Vector2([x, y]) => Self::Vector2([x.normalized()?, y.normalized()?]),
            Self::Boolean(b) => Self::Boolean(*b),
        })
    }
    pub(crate) fn scalar(&self) -> Result<Number> {
        match self {
            Self::Scalar(n) => Ok(*n),
            _ => Err(error("EXPRESSION_TYPE", "Expected a scalar")),
        }
    }
    pub(crate) fn boolean(&self) -> Result<bool> {
        match self {
            Self::Boolean(b) => Ok(*b),
            _ => Err(error("EXPRESSION_TYPE", "Expected a boolean")),
        }
    }
    pub(crate) fn vector(&self) -> Result<[Number; 2]> {
        match self {
            Self::Vector2(v) => Ok(*v),
            _ => Err(error("EXPRESSION_TYPE", "Expected a two-component vector")),
        }
    }
    pub(crate) fn pair(
        &self,
        rhs: &Self,
        operation: fn(Number, Number) -> Result<Number>,
    ) -> Result<Self> {
        match (self, rhs) {
            (Self::Scalar(a), Self::Scalar(b)) => Ok(Self::Scalar(operation(*a, *b)?)),
            (Self::Vector2(a), Self::Vector2(b)) => Ok(Self::Vector2([
                operation(a[0], b[0])?,
                operation(a[1], b[1])?,
            ])),
            _ => Err(error(
                "EXPRESSION_TYPE",
                "Arithmetic requires matching numeric types",
            )),
        }
    }
    pub(crate) fn scaled(
        &self,
        rhs: Number,
        operation: fn(Number, Number) -> Result<Number>,
    ) -> Result<Self> {
        match self {
            Self::Scalar(a) => Ok(Self::Scalar(operation(*a, rhs)?)),
            Self::Vector2(a) => Ok(Self::Vector2([
                operation(a[0], rhs)?,
                operation(a[1], rhs)?,
            ])),
            _ => Err(error(
                "EXPRESSION_TYPE",
                "Scaling requires a numeric left operand",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_negative_modulo_rounding_and_precision_guards() {
        let n = Number::make(-7, 2).unwrap();
        assert_eq!(n.floor().unwrap(), Number::integer(-4));
        assert_eq!(n.rounded(-10, 10).unwrap(), -4);
        assert_eq!(
            n.modulo(Number::integer(3)).unwrap(),
            Number::make(5, 2).unwrap()
        );
        assert_eq!(Number::make(100, 200).unwrap(), Number::make(1, 2).unwrap());
        assert!(Number::make(1, 0).is_err());
        assert!(Number::make(1, 1_000_000_000_001).is_err());
        assert!(Number::integer(1).div(Number::integer(0)).is_err());
        assert!(Number::make(511, 2).unwrap().rounded(0, 255).is_err());
    }
}
