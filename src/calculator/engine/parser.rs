//! Builds a syntax tree from the tokens (recursive descent).
//!
//! ```text
//! Line           ::= (Identifier '=')? Calculation        x = 5 keeps 5 in x
//! Calculation    ::= TimeConversion | Expression (Unit 'to' Unit)?
//! Unit           ::= Identifier ('/' Identifier | '^' Number)?
//! TimeConversion ::= (Time ('am' | 'pm')? | Number ('am' | 'pm') | 'now') Zone? 'to' Zone
//! Zone           ::= Identifier ('/' Identifier | Identifier)* (('+' | '-') (Number | Time))?
//!                    (the offset only after UTC or GMT; the words stop at 'to')
//! Expression     ::= Term (('+' | '-') Term)*
//! Term           ::= Unary (('*' | '/' | '%') Unary)*
//! Unary          ::= ('+' | '-') Unary | Power
//! Power          ::= Primary ('^' Unary)?
//! Primary        ::= Number DurationUnit?                 30 d, 2 wk; not before 'to'
//!                  | Date | Text
//!                  | Identifier ('(' (Expression (',' Expression)*)? ')')?
//!                  | '(' Expression ')'
//! ```
//!
//! `^` is right-associative and binds tighter than a leading sign: 2^3^2 = 2^9, -2^2 = -4,
//! 2^-1 = 0.5.

use std::fmt;

use chrono::{NaiveDate, NaiveTime};
use rust_decimal::Decimal;

use super::tokenizer::{Kind, Token, parse_number};
use super::{CalcError, LAST_ANSWER, Result, TODAY, functions};

const NOW: &str = "now";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Plus,
    Negate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Subtract,
    Multiply,
    Divide,
    Modulo,
    Power,
}

/// A node of the syntax tree. `position` is where it is in the line (from 0), to report errors.
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Number { value: Decimal, position: usize },
    Date { date: NaiveDate, position: usize },
    /// A length of time, to move a date by: 30 d, 2 wk, 3 months. Years are 12 months, weeks
    /// 7 days.
    Duration { months: Decimal, days: Decimal, position: usize },
    /// Text; only valid as a function argument: len("abc").
    Text { value: String, position: usize },
    /// A named value, such as `pi`, `ans` or `x`.
    Name { name: String, position: usize },
    /// `position` is the operator's.
    Unary { op: UnaryOp, operand: Box<Node>, position: usize },
    /// `position` is the operator's.
    Binary { op: BinaryOp, left: Box<Node>, right: Box<Node>, position: usize },
    /// `position` is the function name's.
    Call { name: String, arguments: Vec<Node>, position: usize },
    Conversion(Box<Conversion>),
    Time(Box<TimeConversion>),
    /// x = 5. `position` is the name's.
    Assignment { name: String, value: Box<Node>, position: usize },
}

/// A unit conversion, `10 km to mi`. Only valid as a whole line.
#[derive(Debug, Clone, PartialEq)]
pub struct Conversion {
    pub value: Node,
    pub from: String,
    pub from_position: usize,
    pub to: String,
    pub to_position: usize,
    /// Where the word 'to' is.
    pub position: usize,
}

/// A time converted between places, `10:30 Mexico to India`. Only valid as a whole line.
#[derive(Debug, Clone, PartialEq)]
pub struct TimeConversion {
    /// The time of day; `None` for now.
    pub time: Option<NaiveTime>,
    /// Written with am or pm, so the result is shown that way too.
    pub twelve_hour: bool,
    pub time_position: usize,
    /// The place the time is in; `None` for the computer's time zone.
    pub from: Option<String>,
    pub from_position: usize,
    pub to: String,
    pub to_position: usize,
    /// Where the word 'to' is.
    pub position: usize,
}

impl Node {
    pub fn position(&self) -> usize {
        match self {
            Self::Number { position, .. }
            | Self::Date { position, .. }
            | Self::Duration { position, .. }
            | Self::Text { position, .. }
            | Self::Name { position, .. }
            | Self::Unary { position, .. }
            | Self::Binary { position, .. }
            | Self::Call { position, .. }
            | Self::Assignment { position, .. } => *position,
            Self::Conversion(conversion) => conversion.position,
            Self::Time(time) => time.position,
        }
    }
}

impl UnaryOp {
    fn symbol(self) -> &'static str {
        match self {
            Self::Plus => "+",
            Self::Negate => "-",
        }
    }
}

impl BinaryOp {
    fn symbol(self) -> &'static str {
        match self {
            Self::Add => "+",
            Self::Subtract => "-",
            Self::Multiply => "*",
            Self::Divide => "/",
            Self::Modulo => "%",
            Self::Power => "^",
        }
    }
}

/// Prints the tree in prefix notation, for tests: `1+2*3` is `(+ 1 (* 2 3))`.
impl fmt::Display for Node {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Number { value, .. } => write!(f, "{value}"),
            Self::Date { date, .. } => write!(f, "{}", date.format("%Y-%m-%d")),
            Self::Duration { months, days, .. } => write!(f, "({months} months {days} days)"),
            Self::Text { value, .. } => write!(f, "\"{}\"", value.replace('"', "\"\"")),
            Self::Name { name, .. } => write!(f, "{name}"),
            Self::Unary { op, operand, .. } => write!(f, "({} {operand})", op.symbol()),
            Self::Binary { op, left, right, .. } => write!(f, "({} {left} {right})", op.symbol()),
            Self::Call { name, arguments, .. } => {
                write!(f, "({name}")?;
                for argument in arguments {
                    write!(f, " {argument}")?;
                }
                write!(f, ")")
            }
            Self::Conversion(c) => write!(f, "(to {} {} {})", c.value, c.from, c.to),
            Self::Time(t) => {
                let time = t.time.map_or(NOW.to_string(), |time| time.format("%H:%M").to_string());
                let twelve = if t.twelve_hour { " 12h" } else { "" };
                let from = t.from.as_deref().unwrap_or("local");
                write!(f, "(time {time}{twelve} {from} {})", t.to)
            }
            Self::Assignment { name, value, .. } => write!(f, "(= {name} {value})"),
        }
    }
}

/// Parses the tokens of a whole line, which end with [`Kind::End`].
pub fn parse(tokens: &[Token]) -> Result<Node> {
    assert!(tokens.last().is_some_and(|t| t.kind == Kind::End), "tokens must end with End");
    let mut parser = Parser { tokens, index: 0 };

    if parser.current().kind == Kind::End {
        return Err(CalcError::new("Empty expression", parser.current().position));
    }

    let node = if parser.current().kind == Kind::Identifier && parser.next().kind == Kind::Assign {
        parser.parse_assignment()?
    } else {
        parser.parse_calculation()?
    };

    let current = parser.current();
    if current.kind != Kind::End {
        return Err(unexpected(current));
    }
    Ok(node)
}

/// What `name` already is to the calculator, so it can't be given a value; `None` if it's free.
pub fn reserved_as(name: &str) -> Option<&'static str> {
    if functions::constant(name).is_some() {
        return Some("a constant");
    }
    if functions::is_function(name) {
        return Some("a function");
    }
    match name.to_ascii_lowercase().as_str() {
        LAST_ANSWER => Some("the last answer"),
        TODAY => Some("today's date"),
        NOW | "to" | "am" | "pm" => Some("a word the calculator uses"),
        _ => None,
    }
}

/// The months and days in one of a unit of time a date can be moved by; `None` for any other
/// word.
pub fn duration_unit(name: &str) -> Option<(Decimal, Decimal)> {
    match name.to_ascii_lowercase().as_str() {
        "d" | "day" | "days" => Some((Decimal::ZERO, Decimal::ONE)),
        "w" | "wk" | "week" | "weeks" => Some((Decimal::ZERO, Decimal::from(7))),
        "mo" | "month" | "months" => Some((Decimal::ONE, Decimal::ZERO)),
        "y" | "yr" | "year" | "years" => Some((Decimal::from(12), Decimal::ZERO)),
        _ => None,
    }
}

fn unexpected(token: &Token) -> CalcError {
    CalcError::new(format!("Unexpected '{}'", token.value), token.position)
}

fn is_word(token: &Token, word: &str) -> bool {
    token.kind == Kind::Identifier && token.value.eq_ignore_ascii_case(word)
}

fn is_to(token: &Token) -> bool {
    is_word(token, "to")
}

fn is_half_of_day(token: &Token) -> bool {
    is_word(token, "am") || is_word(token, "pm")
}

struct Parser<'a> {
    tokens: &'a [Token],
    index: usize,
}

impl<'a> Parser<'a> {
    fn current(&self) -> &'a Token {
        &self.tokens[self.index]
    }

    fn next(&self) -> &'a Token {
        &self.tokens[(self.index + 1).min(self.tokens.len() - 1)]
    }

    /// Moves to the next token, but never past End.
    fn advance(&mut self) {
        if self.index < self.tokens.len() - 1 {
            self.index += 1;
        }
    }

    fn expect(&mut self, kind: Kind, message: &str) -> Result<()> {
        if self.current().kind != kind {
            return Err(CalcError::new(message, self.current().position));
        }
        self.advance();
        Ok(())
    }

    fn parse_calculation(&mut self) -> Result<Node> {
        if self.starts_with_time() {
            return self.parse_time_conversion();
        }
        let node = self.parse_expression()?;
        if self.current().kind == Kind::Identifier {
            return self.parse_conversion(node);
        }
        Ok(node)
    }

    /// A name, '=' and what it keeps; not a name the calculator has, which it would hide.
    fn parse_assignment(&mut self) -> Result<Node> {
        let name = self.current();
        if let Some(reason) = reserved_as(&name.value) {
            return Err(CalcError::new(
                format!("'{}' is {reason}, so it can't be given a value", name.value),
                name.position,
            ));
        }
        self.advance();
        self.advance();

        if self.current().kind == Kind::End {
            return Err(CalcError::new("Expected a value after '='", self.current().position));
        }

        let value = self.parse_calculation()?;
        if let Node::Time(time) = &value {
            return Err(CalcError::new(
                "A time in another place can't be kept in a name",
                time.position,
            ));
        }
        Ok(Node::Assignment {
            name: name.value.clone(),
            value: Box::new(value),
            position: name.position,
        })
    }

    fn parse_conversion(&mut self, value: Node) -> Result<Node> {
        let from_token = self.current();
        if is_to(from_token) {
            return Err(CalcError::new("Expected a unit before 'to'", from_token.position));
        }
        let from = self.parse_unit();

        if !is_to(self.current()) {
            return Err(CalcError::new(
                format!("Expected 'to' after '{from}'"),
                from_token.position,
            ));
        }
        let to_word = self.current();
        self.advance();

        let to_token = self.current();
        if to_token.kind != Kind::Identifier {
            return Err(CalcError::new("Expected a unit after 'to'", to_token.position));
        }
        let to = self.parse_unit();

        Ok(Node::Conversion(Box::new(Conversion {
            value,
            from,
            from_position: from_token.position,
            to,
            to_position: to_token.position,
            position: to_word.position,
        })))
    }

    /// A unit's name, which can have an operator in it: km/h, m^2.
    fn parse_unit(&mut self) -> String {
        let mut name = self.current().value.clone();
        self.advance();

        let (current, next) = (self.current(), self.next());
        if (current.kind == Kind::Divide && next.kind == Kind::Identifier)
            || (current.kind == Kind::Power && next.kind == Kind::Number)
        {
            name.push_str(&current.value);
            name.push_str(&next.value);
            self.advance();
            self.advance();
        }
        name
    }

    /// A time conversion starts with a time (10:30, 3 pm) or 'now'. A number without am or pm
    /// is a number: 3 h to min.
    fn starts_with_time(&self) -> bool {
        let (current, next) = (self.current(), self.next());
        match current.kind {
            Kind::Time => true,
            Kind::Number => is_half_of_day(next),
            Kind::Identifier => {
                current.value.eq_ignore_ascii_case(NOW)
                    && matches!(next.kind, Kind::Identifier | Kind::End)
            }
            _ => false,
        }
    }

    fn parse_time_conversion(&mut self) -> Result<Node> {
        let time_token = self.current();
        let (time, twelve_hour) = if time_token.kind == Kind::Identifier {
            self.advance(); // now
            (None, false)
        } else {
            let (time, twelve_hour) = self.parse_time()?;
            (Some(time), twelve_hour)
        };

        let from_token = self.current();
        let mut from = None;
        if !is_to(from_token) {
            if from_token.kind != Kind::Identifier {
                return Err(CalcError::new(
                    "Expected a time zone or 'to' after the time",
                    from_token.position,
                ));
            }
            let zone = self.parse_time_zone();
            // The zone's words go on until 'to', so only something else can stop them.
            if !is_to(self.current()) {
                return Err(CalcError::new(
                    format!("Expected 'to' after '{zone}'"),
                    self.current().position,
                ));
            }
            from = Some(zone);
        }

        let to_word = self.current();
        self.advance();

        let to_token = self.current();
        if to_token.kind != Kind::Identifier {
            return Err(CalcError::new("Expected a time zone after 'to'", to_token.position));
        }
        let to = self.parse_time_zone();

        Ok(Node::Time(Box::new(TimeConversion {
            time,
            twelve_hour,
            time_position: time_token.position,
            from,
            from_position: from_token.position,
            to,
            to_position: to_token.position,
            position: to_word.position,
        })))
    }

    /// A time: 10:30 or 22:30 (24-hour), or 10 pm or 10:30 pm (12-hour).
    fn parse_time(&mut self) -> Result<(NaiveTime, bool)> {
        let token = self.current();
        self.advance();

        let (hours_text, minutes_text) = match token.value.split_once(':') {
            Some((hours, minutes)) => (hours, Some(minutes)),
            None => (token.value.as_str(), None),
        };
        if hours_text.contains('.') {
            return Err(CalcError::new("Write a time as 10:30 or 10:30 pm", token.position));
        }

        let twelve_hour = is_half_of_day(self.current());
        let pm = twelve_hour && self.current().value.eq_ignore_ascii_case("pm");
        if twelve_hour {
            self.advance();
        }

        let hours_range = if twelve_hour { 1..=12 } else { 0..=23 };
        let mut hours = match hours_text.parse::<u32>() {
            Ok(hours) if hours_range.contains(&hours) => hours,
            _ => {
                let message = if twelve_hour {
                    "With am or pm, the hour must be from 1 to 12"
                } else {
                    "The hour must be from 0 to 23"
                };
                return Err(CalcError::new(message, token.position));
            }
        };

        let mut minutes = 0;
        if let Some(text) = minutes_text {
            minutes = match text.parse::<u32>() {
                Ok(minutes) if text.len() == 2 && minutes <= 59 => minutes,
                _ => {
                    return Err(CalcError::new(
                        "The minutes must be two digits, from 00 to 59",
                        token.position + hours_text.len() + 1,
                    ));
                }
            };
        }

        if twelve_hour {
            // 12 am is midnight and 12 pm is noon.
            hours = hours % 12 + if pm { 12 } else { 0 };
        }

        let time = NaiveTime::from_hms_opt(hours, minutes, 0).expect("checked above");
        Ok((time, twelve_hour))
    }

    /// A place or time zone, which can have several words (Sao Paulo), slashes (America/Lima)
    /// and, after UTC or GMT, an offset (UTC-5, UTC+5:30).
    fn parse_time_zone(&mut self) -> String {
        let mut name = self.current().value.clone();
        self.advance();

        loop {
            let (current, next) = (self.current(), self.next());
            if current.kind == Kind::Divide && next.kind == Kind::Identifier {
                name.push('/');
                name.push_str(&next.value);
                self.advance();
                self.advance();
            } else if current.kind == Kind::Identifier && !is_to(current) {
                name.push(' ');
                name.push_str(&current.value);
                self.advance();
            } else {
                break;
            }
        }

        let is_utc = name.eq_ignore_ascii_case("utc") || name.eq_ignore_ascii_case("gmt");
        let (current, next) = (self.current(), self.next());
        if is_utc
            && matches!(current.kind, Kind::Plus | Kind::Minus)
            && matches!(next.kind, Kind::Number | Kind::Time)
        {
            name.push_str(&current.value);
            name.push_str(&next.value);
            self.advance();
            self.advance();
        }
        name
    }

    fn parse_expression(&mut self) -> Result<Node> {
        let mut left = self.parse_term()?;
        loop {
            let op = match self.current().kind {
                Kind::Plus => BinaryOp::Add,
                Kind::Minus => BinaryOp::Subtract,
                _ => return Ok(left),
            };
            let position = self.current().position;
            self.advance();
            let right = self.parse_term()?;
            left = Node::Binary { op, left: Box::new(left), right: Box::new(right), position };
        }
    }

    fn parse_term(&mut self) -> Result<Node> {
        let mut left = self.parse_unary()?;
        loop {
            let op = match self.current().kind {
                Kind::Multiply => BinaryOp::Multiply,
                Kind::Divide => BinaryOp::Divide,
                Kind::Modulo => BinaryOp::Modulo,
                _ => return Ok(left),
            };
            let position = self.current().position;
            self.advance();
            let right = self.parse_unary()?;
            left = Node::Binary { op, left: Box::new(left), right: Box::new(right), position };
        }
    }

    fn parse_unary(&mut self) -> Result<Node> {
        let op = match self.current().kind {
            Kind::Plus => UnaryOp::Plus,
            Kind::Minus => UnaryOp::Negate,
            _ => return self.parse_power(),
        };
        let position = self.current().position;
        self.advance();
        let operand = self.parse_unary()?;
        Ok(Node::Unary { op, operand: Box::new(operand), position })
    }

    fn parse_power(&mut self) -> Result<Node> {
        let left = self.parse_primary()?;
        if self.current().kind != Kind::Power {
            return Ok(left);
        }
        let position = self.current().position;
        self.advance();
        // Unary (not Primary) makes ^ right-associative and allows a signed exponent: 2^3^2, 2^-1.
        let right = self.parse_unary()?;
        Ok(Node::Binary { op: BinaryOp::Power, left: Box::new(left), right: Box::new(right), position })
    }

    fn parse_primary(&mut self) -> Result<Node> {
        let token = self.current();
        let position = token.position;

        match token.kind {
            Kind::Number => {
                self.advance();
                let value = parse_number(&token.value).expect("checked by the tokenizer");

                // A length of time, for a date: 30 d. But 1 d to h is a conversion.
                if self.current().kind == Kind::Identifier
                    && !is_to(self.next())
                    && let Some((months, days)) = duration_unit(&self.current().value)
                {
                    self.advance();
                    let too_large = || CalcError::new("Number is too large", position);
                    return Ok(Node::Duration {
                        months: value.checked_mul(months).ok_or_else(too_large)?,
                        days: value.checked_mul(days).ok_or_else(too_large)?,
                        position,
                    });
                }
                Ok(Node::Number { value, position })
            }
            Kind::Date => {
                self.advance();
                let date = parse_date(&token.value).ok_or_else(|| {
                    CalcError::new(format!("There's no such date as {}", token.value), position)
                })?;
                Ok(Node::Date { date, position })
            }
            Kind::Text => {
                // Allowed anywhere here; the evaluator only takes it as a function argument.
                self.advance();
                Ok(Node::Text { value: token.value.clone(), position })
            }
            Kind::Identifier => {
                self.advance();
                if self.current().kind == Kind::LeftParenthesis {
                    self.parse_call(token)
                } else {
                    Ok(Node::Name { name: token.value.clone(), position })
                }
            }
            Kind::LeftParenthesis => {
                self.advance();
                let node = self.parse_expression()?;
                self.expect(Kind::RightParenthesis, "Expected ')'")?;
                Ok(node)
            }
            Kind::End => Err(CalcError::new("Unexpected end of expression", position)),
            _ => Err(unexpected(token)),
        }
    }

    fn parse_call(&mut self, name: &Token) -> Result<Node> {
        self.advance(); // the '('

        let mut arguments = Vec::new();
        if self.current().kind != Kind::RightParenthesis {
            arguments.push(self.parse_expression()?);
            while self.current().kind == Kind::Comma {
                self.advance();
                arguments.push(self.parse_expression()?);
            }
        }

        let message = if arguments.is_empty() { "Expected ')'" } else { "Expected ',' or ')'" };
        self.expect(Kind::RightParenthesis, message)?;

        Ok(Node::Call { name: name.value.clone(), arguments, position: name.position })
    }
}

/// A date written year-month-day (2026-12-25 or 2026-1-5); `None` if there's no such day.
fn parse_date(text: &str) -> Option<NaiveDate> {
    let mut parts = text.splitn(3, '-').map(str::parse::<u32>);
    let (year, month, day) = (parts.next()?.ok()?, parts.next()?.ok()?, parts.next()?.ok()?);
    if year == 0 {
        return None;
    }
    NaiveDate::from_ymd_opt(year as i32, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calculator::engine::tokenizer::tokenize;

    fn tree(line: &str) -> Result<Node> {
        parse(&tokenize(line)?)
    }

    #[test]
    fn builds_the_tree() {
        for (line, expected) in [
            // Numbers and names
            ("1", "1"),
            ("1.5", "1.5"),
            (".5", "0.5"),
            ("1.", "1"),
            ("pi", "pi"),
            // Precedence and associativity
            ("1+2", "(+ 1 2)"),
            ("1+2*3", "(+ 1 (* 2 3))"),
            ("(1+2)*3", "(* (+ 1 2) 3)"),
            ("1-2-3", "(- (- 1 2) 3)"),
            ("8/4/2", "(/ (/ 8 4) 2)"),
            ("7%3*2", "(* (% 7 3) 2)"),
            ("2^3^2", "(^ 2 (^ 3 2))"),
            ("2*3^2", "(* 2 (^ 3 2))"),
            ("((1))", "1"),
            // Signs
            ("-2^2", "(- (^ 2 2))"),
            ("2^-1", "(^ 2 (- 1))"),
            ("--1", "(- (- 1))"),
            ("+1", "(+ 1)"),
            ("2*-3", "(* 2 (- 3))"),
            ("1--1", "(- 1 (- 1))"),
            // Function calls
            ("sqrt(2)", "(sqrt 2)"),
            ("max(1, 2+3)", "(max 1 (+ 2 3))"),
            ("rand()", "(rand)"),
            ("max(min(1, 2), -pi)", "(max (min 1 2) (- pi))"),
            // Text
            (r#"len("abc")"#, r#"(len "abc")"#),
            (r#"len("a""b") + 1"#, r#"(+ (len "a""b") 1)"#),
            // Unit conversions
            ("10 km to mi", "(to 10 km mi)"),
            ("10 km TO mi", "(to 10 km mi)"),
            ("(1+2)*3 km to mi", "(to (* (+ 1 2) 3) km mi)"),
            ("-1 C to F", "(to (- 1) C F)"),
            ("pi m to ft", "(to pi m ft)"),
            ("100 km/h to m/s", "(to 100 km/h m/s)"),
            ("2 m^2 to ft^2", "(to 2 m^2 ft^2)"),
            // Time conversions
            ("10:30 Mexico to India", "(time 10:30 Mexico India)"),
            ("3 pm sao paulo to new york", "(time 15:00 12h sao paulo new york)"),
            ("12:15 am cali to Bogota", "(time 00:15 12h cali Bogota)"),
            ("10:30 to China", "(time 10:30 local China)"),
            ("now to China", "(time now local China)"),
            ("9:00 America/Lima to UTC-5", "(time 09:00 America/Lima UTC-5)"),
            ("9:00 GMT+5:30 to utc", "(time 09:00 GMT+5:30 utc)"),
            // Names, dates and lengths of time
            ("x = 1 + 2", "(= x (+ 1 2))"),
            ("2026-1-5 + 30 d", "(+ 2026-01-05 (0 months 30 days))"),
        ] {
            assert_eq!(tree(line).unwrap().to_string(), expected, "{line}");
        }
    }

    #[test]
    fn syntax_errors_say_where() {
        for (line, position) in [
            ("", 0),
            ("   ", 3),
            ("1+", 2),
            ("*1", 0),
            ("(1", 2),
            ("1)", 1),
            ("()", 1),
            ("1 2", 2),
            ("2pi", 1),
            ("max(1,)", 6),
            ("max(1 2)", 6),
            ("max(1", 5),
            ("10 km", 3),
            ("10 km mi", 3),
            ("10 km/", 3),
            ("10 to mi", 3),
            ("10 km to", 8),
            ("10 km to 5", 9),
            ("10 km to mi 5", 12),
            ("(10 km to mi)", 4),
            ("10 km to mi + 1", 12),
        ] {
            assert_eq!(tree(line).unwrap_err().position, position, "{line}");
        }
    }

    #[test]
    fn a_binary_node_is_at_its_operator() {
        let node = tree("1 + 2").unwrap();
        assert!(matches!(node, Node::Binary { op: BinaryOp::Add, position: 2, .. }));
    }
}
