//! The constants and functions lines can use. Names are in any letter case.

use rust_decimal::prelude::ToPrimitive;
use rust_decimal::{Decimal, RoundingStrategy};
use rust_decimal_macros::dec;

use super::CalcError;
use super::math::{self, MathError};

pub const CONSTANTS: [(&str, Decimal); 2] = [
    ("pi", dec!(3.1415926535897932384626433833)),
    ("e", dec!(2.7182818284590452353602874714)),
];

/// The function whose result is a text, which the evaluator calls itself.
pub const IPCONFIG: &str = "ipconfig";

/// A function's argument, as calculated: a number or a text.
pub struct Argument {
    pub value: ArgumentValue,
    /// Where it is in the line, to report a wrong type.
    pub position: usize,
}

pub enum ArgumentValue {
    Number(Decimal),
    Text(String),
}

impl Argument {
    pub fn number(&self) -> Result<Decimal, Failure> {
        match &self.value {
            ArgumentValue::Number(number) => Ok(*number),
            ArgumentValue::Text(_) => Err(CalcError::new("Expected a number", self.position).into()),
        }
    }

    pub fn text(&self) -> Result<&str, Failure> {
        match &self.value {
            ArgumentValue::Text(text) => Ok(text),
            ArgumentValue::Number(_) => Err(CalcError::new("Expected text", self.position).into()),
        }
    }
}

/// Why a function failed: an argument of the wrong type (reported where the argument is), or
/// invalid input (reported where the function is).
pub enum Failure {
    Argument(CalcError),
    Math(MathError),
}

impl From<CalcError> for Failure {
    fn from(error: CalcError) -> Self {
        Self::Argument(error)
    }
}

impl From<MathError> for Failure {
    fn from(error: MathError) -> Self {
        Self::Math(error)
    }
}

pub struct Function {
    pub name: &'static str,
    pub min_arguments: usize,
    /// `usize::MAX` for any number.
    pub max_arguments: usize,
    pub invoke: fn(&[Argument]) -> Result<Decimal, Failure>,
}

impl Function {
    pub fn accepts(&self, count: usize) -> bool {
        (self.min_arguments..=self.max_arguments).contains(&count)
    }

    /// "1 argument", "1 to 2 arguments", "at least 1 argument".
    pub fn describe_argument_count(&self) -> String {
        let plural = |count: usize| {
            if count == 1 { "1 argument".to_string() } else { format!("{count} arguments") }
        };
        if self.max_arguments == usize::MAX {
            format!("at least {}", plural(self.min_arguments))
        } else if self.min_arguments == self.max_arguments {
            plural(self.min_arguments)
        } else {
            format!("{} to {}", self.min_arguments, plural(self.max_arguments))
        }
    }
}

const fn function(
    name: &'static str,
    min_arguments: usize,
    max_arguments: usize,
    invoke: fn(&[Argument]) -> Result<Decimal, Failure>,
) -> Function {
    Function { name, min_arguments, max_arguments, invoke }
}

pub const FUNCTIONS: &[Function] = &[
    function("abs", 1, 1, |a| Ok(a[0].number()?.abs())),
    function("sqrt", 1, 1, |a| Ok(math::sqrt(a[0].number()?)?)),
    // round(x) or round(x, decimals). Halves round away from zero: round(2.5) is 3.
    function("round", 1, 2, |a| {
        let value = a[0].number()?;
        let decimals = match a.get(1) {
            Some(decimals) => to_decimals(decimals.number()?)?,
            None => 0,
        };
        Ok(value.round_dp_with_strategy(decimals, RoundingStrategy::MidpointAwayFromZero))
    }),
    function("floor", 1, 1, |a| Ok(a[0].number()?.floor())),
    function("ceil", 1, 1, |a| Ok(a[0].number()?.ceil())),
    function("min", 1, usize::MAX, |a| {
        a.iter().try_fold(Decimal::MAX, |smallest, x| Ok(smallest.min(x.number()?)))
    }),
    function("max", 1, usize::MAX, |a| {
        a.iter().try_fold(Decimal::MIN, |largest, x| Ok(largest.max(x.number()?)))
    }),
    // Trigonometry in radians.
    function("sin", 1, 1, |a| Ok(math::trigonometric(f64::sin, a[0].number()?)?)),
    function("cos", 1, 1, |a| Ok(math::trigonometric(f64::cos, a[0].number()?)?)),
    function("tan", 1, 1, |a| Ok(math::trigonometric(f64::tan, a[0].number()?)?)),
    function("ln", 1, 1, |a| Ok(math::ln(a[0].number()?)?)),
    function("log", 1, 1, |a| Ok(math::log10(a[0].number()?)?)),
    function("exp", 1, 1, |a| Ok(math::from_f64(math::to_f64(a[0].number()?).exp())?)),
    // Text
    function("len", 1, 1, |a| Ok(Decimal::from(a[0].text()?.chars().count()))),
    // find(part, text) or find(part, text, start), as Excel's FIND.
    function("find", 2, 3, |a| {
        let (part, text) = (a[0].text()?, a[1].text()?);
        let start = match a.get(2) {
            Some(start) => start.number()?,
            None => Decimal::ONE,
        };
        Ok(find(part, text, start)?)
    }),
];

pub fn constant(name: &str) -> Option<Decimal> {
    CONSTANTS.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, value)| *value)
}

pub fn function_named(name: &str) -> Option<&'static Function> {
    FUNCTIONS.iter().find(|f| f.name.eq_ignore_ascii_case(name))
}

/// A function of either kind: one giving a number, or ipconfig.
pub fn is_function(name: &str) -> bool {
    function_named(name).is_some() || name.eq_ignore_ascii_case(IPCONFIG)
}

/// Where `part` first is in `text` (from 1), starting at `start`. Case-sensitive. An empty part
/// is found at `start`.
fn find(part: &str, text: &str, start: Decimal) -> Result<Decimal, MathError> {
    let (part, text): (Vec<char>, Vec<char>) = (part.chars().collect(), text.chars().collect());
    let last_start = text.len() + 1;

    let start = whole_number(start)
        .filter(|start| (1..=last_start).contains(start))
        .ok_or_else(|| {
            MathError::Other(format!("Start must be a whole number from 1 to {last_start}"))
        })?;

    (start - 1..=text.len())
        .find(|&index| text[index..].starts_with(&part))
        .map(|index| Decimal::from(index + 1))
        .ok_or_else(|| MathError::Other("Text not found".into()))
}

fn to_decimals(value: Decimal) -> Result<u32, MathError> {
    whole_number(value)
        .filter(|decimals| *decimals <= 28)
        .map(|decimals| decimals as u32)
        .ok_or_else(|| MathError::Other("Decimal places must be a whole number from 0 to 28".into()))
}

/// The value as a `usize`, if it's a whole number that fits in one.
fn whole_number(value: Decimal) -> Option<usize> {
    if value != value.trunc() {
        return None;
    }
    value.to_usize()
}
