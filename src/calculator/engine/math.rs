//! Maths for `Decimal` that it doesn't provide itself, and the arithmetic errors a calculation
//! can run into.
//!
//! `Decimal` is exact for decimal fractions (0.1 + 0.2 is 0.3) with up to 28 decimals. Functions
//! such as `ln` or `sin` go through `f64` instead, which keeps about 15 digits.

use rust_decimal::prelude::ToPrimitive;
use rust_decimal::{Decimal, RoundingStrategy};

use super::CalcError;

/// An arithmetic error, reported at the position of the operator or function that hit it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MathError {
    DivideByZero,
    /// The result doesn't fit in a `Decimal` (about ±7.9 × 10²⁸).
    Overflow,
    Other(String),
}

impl MathError {
    pub fn at(self, position: usize) -> CalcError {
        let message = match self {
            Self::DivideByZero => "Division by zero".to_string(),
            Self::Overflow => "Number is too large".to_string(),
            Self::Other(message) => message,
        };
        CalcError::new(message, position)
    }
}

pub type MathResult = Result<Decimal, MathError>;

/// Turns `None` from a `checked_*` operation into an overflow.
pub fn checked(result: Option<Decimal>) -> MathResult {
    result.ok_or(MathError::Overflow)
}

pub fn divide(x: Decimal, y: Decimal) -> MathResult {
    if y.is_zero() {
        return Err(MathError::DivideByZero);
    }
    checked(x.checked_div(y))
}

/// The remainder, with the sign of `x`: -7 % 3 is -1.
pub fn remainder(x: Decimal, y: Decimal) -> MathResult {
    if y.is_zero() {
        return Err(MathError::DivideByZero);
    }
    checked(x.checked_rem(y))
}

pub fn to_f64(value: Decimal) -> f64 {
    value.to_f64().unwrap_or(f64::NAN)
}

/// Turns an `f64` result into a `Decimal`, keeping 15 significant digits: the digits after
/// those are only the noise of binary floating point.
pub fn from_f64(value: f64) -> MathResult {
    if value.is_nan() {
        return Err(MathError::Other("Result is not a real number".into()));
    }
    if value.is_infinite() || value.abs() > to_f64(Decimal::MAX) {
        return Err(MathError::Overflow);
    }
    if value == 0.0 {
        return Ok(Decimal::ZERO);
    }
    // 15 significant digits, as 1.41421356237310e0.
    if let Ok(decimal) = Decimal::from_scientific(&format!("{value:.14e}")) {
        return Ok(decimal);
    }
    // Too many decimals for from_scientific (a tiny number): write it out in full instead, and
    // let parsing round it to the 28 decimals a Decimal holds.
    let exponent = value.abs().log10().floor() as i32;
    let decimals = (14 - exponent).max(0) as usize;
    format!("{value:.decimals$}").parse().map_err(|_| MathError::Overflow)
}

/// Rounds to this many significant digits, halves away from zero: 1234.5678 to 6 digits is
/// 1234.57. The digits before the decimal point are always kept: 123456 to 2 digits is 123456.
pub fn round_to_significant_digits(value: Decimal, digits: i32) -> Decimal {
    if value.is_zero() {
        return value;
    }
    let exponent = to_f64(value.abs()).log10().floor() as i32;
    let decimals = (digits - 1 - exponent).clamp(0, 28) as u32;
    value.round_dp_with_strategy(decimals, RoundingStrategy::MidpointAwayFromZero)
}

/// x^y. Whole exponents are exact (repeated squaring); others go through `f64`.
pub fn pow(x: Decimal, y: Decimal) -> MathResult {
    let whole = if y == y.trunc() { y.to_i64() } else { None };
    let Some(exponent) = whole else {
        return from_f64(to_f64(x).powf(to_f64(y)));
    };

    let mut base = x;
    if exponent < 0 {
        // 0^-1 is a division by zero. Using 1/x lets tiny results round to 0 instead of
        // overflowing.
        base = divide(Decimal::ONE, base)?;
    }

    let mut remaining = exponent.unsigned_abs();
    let mut result = Decimal::ONE;
    while remaining > 0 {
        if remaining & 1 == 1 {
            result = checked(result.checked_mul(base))?;
        }
        remaining >>= 1;
        if remaining > 0 {
            base = checked(base.checked_mul(base))?;
        }
    }
    Ok(result)
}

/// Square root with all 28 decimals: an `f64` estimate refined with Newton's method.
pub fn sqrt(x: Decimal) -> MathResult {
    if x.is_sign_negative() && !x.is_zero() {
        return Err(MathError::Other("Square root of a negative number".into()));
    }
    if x.is_zero() {
        return Ok(Decimal::ZERO);
    }

    let mut guess = from_f64(to_f64(x).sqrt())?;
    for _ in 0..10 {
        let next = divide(checked(guess.checked_add(divide(x, guess)?))?, Decimal::TWO)?;
        if next == guess {
            break;
        }
        guess = next;
    }
    Ok(guess)
}

/// Natural logarithm.
pub fn ln(x: Decimal) -> MathResult {
    ensure_positive(x)?;
    from_f64(to_f64(x).ln())
}

/// Base-10 logarithm.
pub fn log10(x: Decimal) -> MathResult {
    ensure_positive(x)?;
    from_f64(to_f64(x).log10())
}

/// sin, cos or tan of radians. `f64` is only accurate to about 1e-16, so sin(pi) would be
/// 1.2e-16 rather than 0: rounding to 15 decimals removes that noise.
pub fn trigonometric(function: fn(f64) -> f64, radians: Decimal) -> MathResult {
    Ok(from_f64(function(to_f64(radians)))?.round_dp(15))
}

fn ensure_positive(x: Decimal) -> Result<(), MathError> {
    if x <= Decimal::ZERO {
        return Err(MathError::Other("Logarithm of zero or a negative number".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calculator::engine::format_number;

    fn dec(text: &str) -> Decimal {
        text.parse().unwrap()
    }

    #[test]
    fn rounds_to_significant_digits() {
        for (value, digits, expected) in [
            ("1234.5678", 6, "1234.57"),
            ("0.00012345", 2, "0.00012"),
            ("-1234.5678", 6, "-1234.57"),
            ("123456", 2, "123456"),
            ("0.5", 1, "0.5"),
            ("0", 5, "0"),
            ("0.0000000000000000000000000001", 10, "0.0000000000000000000000000001"),
            ("99.99", 2, "100"),
        ] {
            let result = round_to_significant_digits(dec(value), digits);
            assert_eq!(format_number(result, false), expected, "{value} to {digits} digits");
        }
    }

    #[test]
    fn from_f64_keeps_15_significant_digits() {
        assert_eq!(from_f64(std::f64::consts::SQRT_2), Ok(dec("1.41421356237310")));
        assert_eq!(from_f64(0.49999999999999994), Ok(dec("0.5")));
        assert_eq!(from_f64(1.2246467991473532e-16), Ok(dec("0.0000000000000001224646799147")));
        assert_eq!(from_f64(1e-40), Ok(Decimal::ZERO));
        assert_eq!(from_f64(1e30), Err(MathError::Overflow));
        assert_eq!(from_f64(f64::NAN), Err(MathError::Other("Result is not a real number".into())));
    }
}
