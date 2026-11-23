//! Calculates the value of a syntax tree: a number, a date, a length of time or a text.
//! A date moves by a length of time or a number of days; two dates are a length of time apart.

use chrono::{Datelike, Months, NaiveDate, TimeDelta};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

use super::functions::{self, Argument, ArgumentValue, Failure};
use super::math::{self, MathError, MathResult, checked};
use super::network::{self, NetworkReader};
use super::parser::{BinaryOp, Conversion, Node, UnaryOp};
use super::{CalcError, LAST_ANSWER, Names, Result, TODAY, Value, units};

/// Conversions are rounded to this many significant digits: enough for any practical use,
/// instead of the 28 decimals of 1 km to mi.
const CONVERSION_DIGITS: i32 = 10;

pub struct Evaluator<'a> {
    pub names: &'a Names,
    pub today: NaiveDate,
    pub network: &'a dyn NetworkReader,
}

impl Evaluator<'_> {
    pub fn evaluate(&self, node: &Node) -> Result<Value> {
        match node {
            Node::Number { value, .. } => Ok(Value::Number(*value)),
            Node::Date { date, .. } => Ok(Value::Date(*date)),
            Node::Duration { months, days, .. } => Ok(Value::Duration { months: *months, days: *days }),
            Node::Name { name, position } => self.name(name, *position),
            Node::Unary { op, operand, position } => self.unary(*op, operand, *position),
            Node::Binary { op, left, right, position } => self.binary(*op, left, right, *position),
            Node::Call { name, arguments, position } => self.call(name, arguments, *position),
            Node::Conversion(conversion) => self.conversion(conversion),
            Node::Assignment { value, .. } => self.evaluate(value),
            // The calculator converts times itself.
            Node::Time(time) => Err(CalcError::new("A time has no number value", time.position)),
            Node::Text { position, .. } => {
                Err(CalcError::new("Text can only be used as a function argument", *position))
            }
        }
    }

    fn number(&self, node: &Node) -> Result<Decimal> {
        number_of(self.evaluate(node)?, node.position())
    }

    fn name(&self, name: &str, position: usize) -> Result<Value> {
        if let Some(value) = functions::constant(name) {
            return Ok(Value::Number(value));
        }
        if name.eq_ignore_ascii_case(TODAY) {
            return Ok(Value::Date(self.today));
        }
        if let Some(value) = super::find_name(self.names, name) {
            return Ok(value.clone());
        }
        let message = if name.eq_ignore_ascii_case(LAST_ANSWER) {
            "There's no last answer yet".to_string()
        } else {
            format!("Unknown name '{name}'")
        };
        Err(CalcError::new(message, position))
    }

    fn unary(&self, op: UnaryOp, operand: &Node, position: usize) -> Result<Value> {
        let value = self.evaluate(operand)?;
        if op == UnaryOp::Plus {
            return Ok(value);
        }
        match value {
            Value::Number(number) => Ok(Value::Number(-number)),
            Value::Duration { months, days } => Ok(Value::Duration { months: -months, days: -days }),
            Value::Text(_) => Err(CalcError::new("A text can't be negative", position)),
            Value::Date(_) => Err(CalcError::new("A date can't be negative", position)),
        }
    }

    fn binary(&self, op: BinaryOp, left: &Node, right: &Node, position: usize) -> Result<Value> {
        let left = self.evaluate(left)?;
        let right = self.evaluate(right)?;

        if let (Value::Number(x), Value::Number(y)) = (&left, &right) {
            let (x, y) = (*x, *y);
            let result = match op {
                BinaryOp::Add => checked(x.checked_add(y)),
                BinaryOp::Subtract => checked(x.checked_sub(y)),
                BinaryOp::Multiply => checked(x.checked_mul(y)),
                BinaryOp::Divide => math::divide(x, y),
                BinaryOp::Modulo => math::remainder(x, y),
                BinaryOp::Power => math::pow(x, y),
            };
            return result.map(Value::Number).map_err(|error| error.at(position));
        }

        time_arithmetic(op, left, right, position)
    }

    fn conversion(&self, node: &Conversion) -> Result<Value> {
        let from = units::find(&node.from, node.from_position)?;
        let to = units::find(&node.to, node.to_position)?;
        if from.group != to.group {
            return Err(CalcError::new(
                format!("Can't convert {} to {}", from.symbol, to.symbol),
                node.position,
            ));
        }

        let value = self.number(&node.value)?;
        let converted = from
            .to_base(value)
            .and_then(|base| to.to_unit(base))
            .map(|result| math::round_to_significant_digits(result, CONVERSION_DIGITS))
            .map_err(|error| error.at(node.position))?;
        Ok(Value::Number(converted))
    }

    fn call(&self, name: &str, arguments: &[Node], position: usize) -> Result<Value> {
        if name.eq_ignore_ascii_case(functions::IPCONFIG) {
            if !arguments.is_empty() {
                return Err(CalcError::new(
                    format!("Function '{name}' expects no arguments"),
                    position,
                ));
            }
            return Ok(Value::Text(network::config_text(&self.network.read())));
        }

        let function = functions::function_named(name)
            .ok_or_else(|| CalcError::new(format!("Unknown function '{name}'"), position))?;

        if !function.accepts(arguments.len()) {
            return Err(CalcError::new(
                format!("Function '{}' expects {}", function.name, function.describe_argument_count()),
                position,
            ));
        }

        let arguments = arguments
            .iter()
            .map(|argument| self.argument(argument))
            .collect::<Result<Vec<_>>>()?;

        match (function.invoke)(&arguments) {
            Ok(result) => Ok(Value::Number(result)),
            Err(Failure::Argument(error)) => Err(error),
            Err(Failure::Math(error)) => Err(error.at(position)),
        }
    }

    fn argument(&self, node: &Node) -> Result<Argument> {
        let value = match node {
            Node::Text { value, .. } => ArgumentValue::Text(value.clone()),
            _ => ArgumentValue::Number(self.number(node)?),
        };
        Ok(Argument { value, position: node.position() })
    }
}

fn number_of(value: Value, position: usize) -> Result<Decimal> {
    match value {
        Value::Number(number) => Ok(number),
        Value::Date(_) => Err(CalcError::new("Expected a number, not a date", position)),
        Value::Text(_) => Err(CalcError::new("Expected a number, not a text", position)),
        Value::Duration { .. } => {
            Err(CalcError::new("Expected a number, not a length of time", position))
        }
    }
}

/// Dates and lengths of time: a date and a length of time (or a number of days) make a date;
/// two dates a length of time; two lengths of time another; and a length of time can be
/// multiplied or divided by a number.
fn time_arithmetic(op: BinaryOp, left: Value, right: Value, position: usize) -> Result<Value> {
    use BinaryOp::*;
    use Value::*;

    let at = |error: MathError| error.at(position);
    match (op, &left, &right) {
        (Add | Subtract, Date(date), Number(_) | Duration { .. }) => {
            moved(*date, &right, op == Subtract, position)
        }
        (Add, Number(_) | Duration { .. }, Date(date)) => moved(*date, &left, false, position),
        (Subtract, Date(a), Date(b)) => Ok(Duration {
            months: Decimal::ZERO,
            days: Decimal::from((*a - *b).num_days()),
        }),
        (Add | Subtract, Duration { months: m1, days: d1 }, Duration { months: m2, days: d2 }) => {
            let combine = |x: Decimal, y: Decimal| {
                checked(if op == Subtract { x.checked_sub(y) } else { x.checked_add(y) })
            };
            Ok(Duration { months: combine(*m1, *m2).map_err(at)?, days: combine(*d1, *d2).map_err(at)? })
        }
        (Multiply, Duration { months, days }, Number(by))
        | (Multiply, Number(by), Duration { months, days }) => {
            scaled(*months, *days, |x| checked(x.checked_mul(*by))).map_err(at)
        }
        (Divide, Duration { months, days }, Number(by)) => {
            scaled(*months, *days, |x| math::divide(x, *by)).map_err(at)
        }
        _ => Err(CalcError::new(
            format!("Can't {} {} and {}", verb(op), article(&left), article(&right)),
            position,
        )),
    }
}

/// The date moved by a length of time, or a number of days: its months first, then its days.
fn moved(date: NaiveDate, by: &Value, backwards: bool, position: usize) -> Result<Value> {
    let (months, days) = match by {
        Value::Duration { months, days } => (*months, *days),
        Value::Number(days) => (Decimal::ZERO, *days),
        _ => unreachable!("only called with a number or a length of time"),
    };
    if months != months.trunc() || days != days.trunc() {
        return Err(CalcError::new("A date moves by whole days and whole months", position));
    }

    let out_of_range = || CalcError::new("The date is out of range", position);
    let sign = if backwards { -1 } else { 1 };
    let months = months.to_i64().ok_or_else(out_of_range)? * sign;
    let days = days.to_i64().ok_or_else(out_of_range)? * sign;

    let month_count = Months::new(u32::try_from(months.unsigned_abs()).map_err(|_| out_of_range())?);
    let moved = if months < 0 {
        date.checked_sub_months(month_count)
    } else {
        date.checked_add_months(month_count)
    };
    let moved = moved
        .and_then(|date| date.checked_add_signed(TimeDelta::try_days(days)?))
        .filter(|date| (1..=9999).contains(&date.year()))
        .ok_or_else(out_of_range)?;
    Ok(Value::Date(moved))
}

fn scaled(months: Decimal, days: Decimal, by: impl Fn(Decimal) -> MathResult) -> std::result::Result<Value, MathError> {
    Ok(Value::Duration { months: by(months)?, days: by(days)? })
}

fn verb(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Add => "add",
        BinaryOp::Subtract => "subtract",
        BinaryOp::Multiply => "multiply",
        BinaryOp::Divide => "divide",
        BinaryOp::Modulo => "take the remainder of",
        BinaryOp::Power => "raise",
    }
}

fn article(value: &Value) -> &'static str {
    match value {
        Value::Number(_) => "a number",
        Value::Date(_) => "a date",
        Value::Duration { .. } => "a length of time",
        Value::Text(_) => "a text",
    }
}
