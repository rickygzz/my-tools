//! The calculator's engine: turns a line such as `10 km to mi` into `6.213711922 mi`.
//!
//! A line goes through three steps:
//!
//! 1. [`tokenizer`] splits it into tokens: `10`, `km`, `to`, `mi`.
//! 2. [`parser`] builds a syntax tree from them: `(to 10 km mi)`.
//! 3. [`evaluator`] calculates the tree's value, with [`functions`], [`units`], [`math`] and
//!    [`network`]; [`time_zones`] converts times between places.
//!
//! Numbers are `Decimal`s (exact for decimal fractions, 28 decimals), as in C#'s `decimal`.
//! The engine keeps nothing between lines: the window keeps the names and the last answer.

mod evaluator;
mod functions;
mod math;
mod network;
mod parser;
mod time_zones;
mod tokenizer;
mod units;

use std::collections::HashMap;
use std::fmt;

use chrono::{Datelike, NaiveDate, Weekday};
use rust_decimal::Decimal;

#[cfg(test)]
pub use network::NetworkAddress;
pub use network::{NetworkReader, SystemNetwork};
pub use time_zones::{Clock, SystemClock, Zone};

/// The name of the last answer, which the window keeps among the names: `ans * 2`.
pub const LAST_ANSWER: &str = "ans";
/// The name of today's date: `today + 30 d`.
pub const TODAY: &str = "today";

/// Why a line can't be calculated, and where (counting characters from 0).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalcError {
    pub message: String,
    pub position: usize,
}

impl CalcError {
    pub fn new(message: impl Into<String>, position: usize) -> Self {
        Self { message: message.into(), position }
    }
}

/// "Division by zero at position 3", counting from 1 for people.
impl fmt::Display for CalcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at position {}", self.message, self.position + 1)
    }
}

impl std::error::Error for CalcError {}

pub type Result<T> = std::result::Result<T, CalcError>;

/// What a calculation gives, and what a name keeps.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Number(Decimal),
    Date(NaiveDate),
    /// A length of time. Months are kept apart from days, since a month has no fixed number of
    /// days: 2026-01-31 + 1 month is 2026-02-28.
    Duration { months: Decimal, days: Decimal },
    /// A text a function gives, such as ipconfig()'s; it can be kept, but not calculated with.
    Text(String),
}

impl Value {
    /// As the calculator shows it: a number as [`format_number`] writes it, a date with its day
    /// of the week (`2026-12-25 (Friday)`), a length of time in months and days
    /// (`1 month, 3 days`).
    pub fn to_text(&self, group_thousands: bool) -> String {
        match self {
            Self::Number(number) => format_number(*number, group_thousands),
            Self::Date(date) => format!("{} ({})", date.format("%Y-%m-%d"), weekday_name(date.weekday())),
            Self::Text(text) => text.clone(),
            Self::Duration { months, days } => {
                let counted = |count: Decimal, unit: &str| {
                    let plural = if count.abs() == Decimal::ONE { "" } else { "s" };
                    format!("{} {unit}{plural}", format_number(count, group_thousands))
                };
                let mut parts = Vec::new();
                if !months.is_zero() {
                    parts.push(counted(*months, "month"));
                }
                if !days.is_zero() || parts.is_empty() {
                    parts.push(counted(*days, "day"));
                }
                parts.join(", ")
            }
        }
    }
}

fn weekday_name(day: Weekday) -> &'static str {
    match day {
        Weekday::Mon => "Monday",
        Weekday::Tue => "Tuesday",
        Weekday::Wed => "Wednesday",
        Weekday::Thu => "Thursday",
        Weekday::Fri => "Friday",
        Weekday::Sat => "Saturday",
        Weekday::Sun => "Sunday",
    }
}

/// The values given names (`x = 5`), the last answer among them. Looked up in any letter case.
pub type Names = HashMap<String, Value>;

/// The value of a name, in any letter case.
pub fn find_name<'a>(names: &'a Names, name: &str) -> Option<&'a Value> {
    names.iter().find(|(key, _)| key.eq_ignore_ascii_case(name)).map(|(_, value)| value)
}

/// What [`Calculator::calculate`] gives.
#[derive(Debug, Clone, PartialEq)]
pub struct Calculation {
    /// The result as shown: `22`, `6.213711922 mi`, `2026-12-25 (Friday)`,
    /// `22:00 India (UTC+5:30)`.
    pub text: String,
    /// The value, to keep as the last answer or in a name; `None` for a time in another place.
    pub value: Option<Value>,
    /// The name it was given, as in `x = 5`.
    pub assigned_name: Option<String>,
}

pub struct Calculator {
    clock: Box<dyn Clock>,
    network: Box<dyn NetworkReader>,
}

impl Default for Calculator {
    fn default() -> Self {
        Self::new(Box::new(SystemClock), Box::new(SystemNetwork))
    }
}

impl Calculator {
    /// `clock` gives today's date, the time now and the local time zone; `network` gives
    /// ipconfig() the computer's addresses.
    pub fn new(clock: Box<dyn Clock>, network: Box<dyn NetworkReader>) -> Self {
        Self { clock, network }
    }

    /// Calculates a line: an expression, or a name given a value (`x = 5`). It can use the
    /// names given earlier and the last answer as `ans`.
    ///
    /// `group_thousands` separates thousands with ',' (off, the result can be typed back in,
    /// since ',' also separates function arguments).
    pub fn calculate(&self, line: &str, names: &Names, group_thousands: bool) -> Result<Calculation> {
        let tree = parser::parse(&tokenizer::tokenize(line)?)?;

        if let parser::Node::Time(time) = &tree {
            let text = time_zones::convert(time, self.clock.as_ref())?;
            return Ok(Calculation { text, value: None, assigned_name: None });
        }

        let evaluator = evaluator::Evaluator {
            names,
            today: self.clock.today(),
            network: self.network.as_ref(),
        };
        let value = evaluator.evaluate(&tree)?;

        let (assigned_name, calculation) = match &tree {
            parser::Node::Assignment { name, value, .. } => (Some(name.clone()), value.as_ref()),
            other => (None, other),
        };

        let mut text = value.to_text(group_thousands);
        if let parser::Node::Conversion(conversion) = calculation {
            let unit = units::find(&conversion.to, conversion.to_position)?;
            text = format!("{text} {}", unit.symbol);
        }

        Ok(Calculation { text, value: Some(value), assigned_name })
    }
}

/// Writes a number for people: '.' for decimals (as lines use), no trailing zeros, never an
/// exponent; with `group_thousands`, ',' between thousands (1,234,567.5).
pub fn format_number(value: Decimal, group_thousands: bool) -> String {
    // Avoids "-0" from negating zero.
    if value.is_zero() {
        return "0".to_string();
    }
    let text = value.normalize().to_string();
    if !group_thousands {
        return text;
    }

    let (sign, digits) = match text.strip_prefix('-') {
        Some(digits) => ("-", digits),
        None => ("", text.as_str()),
    };
    let (whole, fraction) = match digits.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (digits, None),
    };

    let mut grouped = String::new();
    for (index, digit) in whole.chars().enumerate() {
        if index > 0 && (whole.len() - index) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    match fraction {
        Some(fraction) => format!("{sign}{grouped}.{fraction}"),
        None => format!("{sign}{grouped}"),
    }
}

/// The things lines can use, for the window's suggestions.
pub mod catalog {
    #[cfg(test)]
    pub use super::functions::FUNCTIONS;
    pub use super::functions::IPCONFIG;
    pub use super::time_zones::{PLACES, format_offset};
    pub use super::units::{UNITS, Unit};

    /// The unit with this name, if there's exactly one.
    pub fn find_unit(name: &str) -> Option<&'static Unit> {
        super::units::find(name, 0).ok()
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use chrono::{DateTime, TimeZone, Utc};

    use super::*;

    /// A clock stopped at one moment, in a chosen local time zone.
    pub struct FixedClock {
        now: DateTime<Utc>,
        zone: Zone,
    }

    impl FixedClock {
        /// At that UTC time, in the IANA zone `local`.
        pub fn at(year: i32, month: u32, day: u32, hour: u32, minute: u32, local: &str) -> Self {
            Self {
                now: Utc.with_ymd_and_hms(year, month, day, hour, minute, 0).unwrap(),
                zone: Zone::Named(local.parse().unwrap()),
            }
        }
    }

    impl Clock for FixedClock {
        fn now(&self) -> DateTime<Utc> {
            self.now
        }

        fn local_zone(&self) -> Zone {
            self.zone
        }
    }

    pub struct FakeNetwork(pub Vec<NetworkAddress>);

    impl NetworkReader for FakeNetwork {
        fn read(&self) -> Vec<NetworkAddress> {
            self.0.clone()
        }
    }

    pub fn calculator_at(clock: FixedClock) -> Calculator {
        Calculator::new(Box::new(clock), Box::new(FakeNetwork(Vec::new())))
    }

    pub fn calculator_with_network(network: FakeNetwork) -> Calculator {
        Calculator::new(Box::new(friday()), Box::new(network))
    }

    /// Friday 13 November 2026, at noon in UTC.
    pub fn friday() -> FixedClock {
        FixedClock::at(2026, 11, 13, 12, 0, "UTC")
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{calculator_at, friday};
    use super::*;

    fn calculate(line: &str) -> Result<Calculation> {
        calculate_with(line, &Names::new())
    }

    fn calculate_with(line: &str, names: &Names) -> Result<Calculation> {
        calculator_at(friday()).calculate(line, names, false)
    }

    /// The line's result as shown, without thousands separators.
    fn text(line: &str) -> String {
        calculate(line).unwrap_or_else(|error| panic!("{line}: {error}")).text
    }

    fn number(line: &str) -> Decimal {
        match calculate(line).unwrap_or_else(|error| panic!("{line}: {error}")).value {
            Some(Value::Number(number)) => number,
            other => panic!("{line} gave {other:?}"),
        }
    }

    fn error(line: &str) -> CalcError {
        calculate(line).expect_err(line)
    }

    fn dec(text: &str) -> Decimal {
        text.parse().unwrap()
    }

    #[test]
    fn calculates() {
        for (line, expected) in [
            // Arithmetic and precedence
            ("1+2", "3"),
            ("2+3*4", "14"),
            ("(1+2)*3", "9"),
            ("10-4-3", "3"),
            ("10/4", "2.5"),
            ("7%3", "1"),
            ("-7%3", "-1"),
            ("1.5*2", "3"),
            // Decimal is exact for decimal fractions
            ("0.1+0.2", "0.3"),
            // Signs and power
            ("-2^2", "-4"),
            ("(-2)^2", "4"),
            ("2^-1", "0.5"),
            ("2^3^2", "512"),
            ("2^10", "1024"),
            ("0^0", "1"),
            ("1.1^2", "1.21"),
            ("--3", "3"),
            // Constants, in any case
            ("pi", "3.1415926535897932384626433833"),
            ("PI", "3.1415926535897932384626433833"),
            ("e", "2.7182818284590452353602874714"),
            // Functions
            ("abs(-3)", "3"),
            ("sqrt(16)", "4"),
            ("sqrt(0)", "0"),
            ("round(2.5)", "3"),
            ("round(-2.5)", "-3"),
            ("round(2.345, 2)", "2.35"),
            ("floor(-1.5)", "-2"),
            ("ceil(1.2)", "2"),
            ("min(3, 1, 2)", "1"),
            ("max(3, 1, 2)", "3"),
            ("max(5)", "5"),
            ("log(1000)", "3"),
            ("exp(0)", "1"),
            ("sin(0)", "0"),
            ("cos(0)", "1"),
            ("SQRT(4) + Max(1, 2)", "4"),
            // Text
            (r#"len("hello")"#, "5"),
            (r#"len("")"#, "0"),
            (r#"len("a b")"#, "3"),
            (r#"len("a""b")"#, "3"),
            (r#"LEN("hello") * 2 + 1"#, "11"),
            (r#"find("b", "abc")"#, "2"),
            (r#"find("a", "banana")"#, "2"),
            (r#"find("a", "banana", 3)"#, "4"),
            (r#"find("a", "banana", 2)"#, "2"),
            (r#"find("na", "banana") + 1"#, "4"),
            (r#"find("", "abc")"#, "1"),
            (r#"find("", "abc", 4)"#, "4"),
            (r#"find("""", "say ""hi""")"#, "5"),
        ] {
            assert_eq!(number(line), dec(expected), "{line}");
        }
    }

    #[test]
    fn sqrt_has_all_28_decimals() {
        assert_eq!(number("sqrt(2)"), dec("1.4142135623730950488016887242"));
    }

    #[test]
    fn fractional_powers_have_15_digits() {
        assert_eq!(number("2^0.5").round_dp(14), dec("1.41421356237310"));
    }

    #[test]
    fn calculation_errors_say_where_and_why() {
        for (line, position, message) in [
            ("1/0", 1, "Division by zero"),
            ("1 + 5 % 0", 6, "Division by zero"),
            ("0^-1", 1, "Division by zero"),
            ("10^30", 2, "Number is too large"),
            ("79228162514264337593543950335 + 1", 30, "Number is too large"),
            ("(-8)^0.5", 4, "Result is not a real number"),
            ("sqrt(-1)", 0, "Square root of a negative number"),
            ("ln(0)", 0, "Logarithm of zero or a negative number"),
            ("log(-1)", 0, "Logarithm of zero or a negative number"),
            ("round(1.5, 0.5)", 0, "Decimal places must be a whole number from 0 to 28"),
            ("round(1.5, 29)", 0, "Decimal places must be a whole number from 0 to 28"),
            ("2 * x", 4, "Unknown name 'x'"),
            ("sqrt", 0, "Unknown name 'sqrt'"),
            ("1 + foo(1)", 4, "Unknown function 'foo'"),
            ("pi()", 0, "Unknown function 'pi'"),
            ("sqrt(1, 2)", 0, "Function 'sqrt' expects 1 argument"),
            ("round(1, 2, 3)", 0, "Function 'round' expects 1 to 2 arguments"),
            ("min()", 0, "Function 'min' expects at least 1 argument"),
            (r#""abc""#, 0, "Text can only be used as a function argument"),
            (r#"1 + "a""#, 4, "Text can only be used as a function argument"),
            (r#"-"a""#, 1, "Text can only be used as a function argument"),
            ("len(5)", 4, "Expected text"),
            ("len(pi)", 4, "Expected text"),
            (r#"sqrt("4")"#, 5, "Expected a number"),
            (r#"max(1, "2")"#, 7, "Expected a number"),
            (r#"len("a", "b")"#, 0, "Function 'len' expects 1 argument"),
            ("len()", 0, "Function 'len' expects 1 argument"),
            (r#"find("x", "abc")"#, 0, "Text not found"),
            (r#"find("B", "abc")"#, 0, "Text not found"),
            (r#"find("a", "banana", 7)"#, 0, "Text not found"),
            (r#"find("a", "abc", 0)"#, 0, "Start must be a whole number from 1 to 4"),
            (r#"find("a", "abc", 5)"#, 0, "Start must be a whole number from 1 to 4"),
            (r#"find("a", "abc", 1.5)"#, 0, "Start must be a whole number from 1 to 4"),
            (r#"find(1, "abc")"#, 5, "Expected text"),
            (r#"find("a", "abc", "1")"#, 17, "Expected a number"),
            (r#"find("a")"#, 0, "Function 'find' expects 2 to 3 arguments"),
            ("", 0, "Empty expression"),
            ("1 +", 3, "Unexpected end of expression"),
            ("1 $ 2", 2, "Invalid character '$'"),
        ] {
            let error = error(line);
            assert_eq!((error.message.as_str(), error.position), (message, position), "{line}");
        }
    }

    #[test]
    fn errors_count_positions_from_1_for_people() {
        assert_eq!(error("1/0").to_string(), "Division by zero at position 2");
    }

    #[test]
    fn numbers_are_shown_plainly() {
        for (line, expected) in [
            ("1.5*2", "3"),
            ("0.1+0.2", "0.3"),
            ("1.10", "1.1"),
            ("-0", "0"),
            ("-0.5", "-0.5"),
            ("1/3", "0.3333333333333333333333333333"),
            ("10^20", "100000000000000000000"),
            ("10^-20", "0.00000000000000000001"),
            ("79228162514264337593543950335", "79228162514264337593543950335"),
            ("sin(pi)", "0"),
            ("cos(pi/2)", "0"),
            ("cos(pi)", "-1"),
            ("sin(pi/6)", "0.5"),
        ] {
            assert_eq!(text(line), expected, "{line}");
        }
    }

    #[test]
    fn thousands_can_be_grouped() {
        for (value, expected) in [
            ("1", "1"),
            ("999", "999"),
            ("1000", "1,000"),
            ("1234567.5", "1,234,567.5"),
            ("-1234567.5", "-1,234,567.5"),
            ("0.0001", "0.0001"),
            ("-0", "0"),
            ("0.3333333333333333333333333333", "0.3333333333333333333333333333"),
            ("79228162514264337593543950335", "79,228,162,514,264,337,593,543,950,335"),
        ] {
            assert_eq!(format_number(dec(value), true), expected, "{value}");
        }
    }

    #[test]
    fn converts_units() {
        for (line, expected) in [
            // Length
            ("1 km to m", "1000 m"),
            ("1 mi to km", "1.609344 km"),
            ("1 km to mi", "0.6213711922 mi"),
            ("1 ft to in", "12 in"),
            ("1 yd to ft", "3 ft"),
            ("1 nmi to m", "1852 m"),
            ("1 mm to nm", "1000000 nm"),
            ("1 nm to km", "0.000000000001 km"),
            // Mass
            ("1 kg to g", "1000 g"),
            ("1 lb to oz", "16 oz"),
            ("1 st to lb", "14 lb"),
            ("1 ton to lb", "2000 lb"),
            ("1 t to kg", "1000 kg"),
            ("1 kg to lb", "2.204622622 lb"),
            // Temperature
            ("100 C to F", "212 F"),
            ("0 C to F", "32 F"),
            ("-40 C to F", "-40 F"),
            ("98.6 F to C", "37 C"),
            ("32 F to C", "0 C"),
            ("0 C to K", "273.15 K"),
            ("0 K to C", "-273.15 C"),
            ("212 F to K", "373.15 K"),
            // Area
            ("1 km2 to m2", "1000000 m2"),
            ("1 ha to m2", "10000 m2"),
            ("1 ft2 to in2", "144 in2"),
            ("1 mi2 to ac", "640 ac"),
            ("1 m^2 to cm^2", "10000 cm2"),
            // Volume
            ("1 L to mL", "1000 mL"),
            ("1 m3 to L", "1000 L"),
            ("1 cc to mL", "1 mL"),
            ("1 gal to qt", "4 qt"),
            ("1 qt to pt", "2 pt"),
            ("1 pt to cup", "2 cup"),
            ("1 cup to floz", "8 floz"),
            ("1 floz to tbsp", "2 tbsp"),
            ("1 tbsp to tsp", "3 tsp"),
            ("1 gal to in3", "231 in3"),
            ("1 ft3 to in3", "1728 in3"),
            ("1 ukgal to L", "4.54609 L"),
            // Time
            ("1 h to min", "60 min"),
            ("1 d to h", "24 h"),
            ("1 wk to d", "7 d"),
            ("1 yr to d", "365.25 d"),
            ("1 s to ms", "1000 ms"),
            ("3600 s to h", "1 h"),
            // Speed
            ("36 km/h to m/s", "10 m/s"),
            ("100 km/h to mph", "62.13711922 mph"),
            ("60 mph to km/h", "96.56064 km/h"),
            ("1 knot to km/h", "1.852 km/h"),
            ("1 mph to ft/s", "1.466666667 ft/s"),
            // Data
            ("1 B to b", "8 b"),
            ("1 MB to kB", "1000 kB"),
            ("1 GB to MB", "1000 MB"),
            ("1 TB to GB", "1000 GB"),
            ("1 PB to TB", "1000 TB"),
            ("1 KiB to B", "1024 B"),
            ("1 GiB to MiB", "1024 MiB"),
            ("1 TiB to GiB", "1024 GiB"),
            ("1 PiB to TiB", "1024 TiB"),
            ("1 GiB to GB", "1.073741824 GB"),
            ("100 Mb to MB", "12.5 MB"),
            ("8 Gb to GB", "1 GB"),
            ("8 Tb to TB", "1 TB"),
            ("8 kb to kB", "1 kB"),
            // The value can be any expression
            ("(1+1)*5 km to m", "10000 m"),
            ("sqrt(16) ft to in", "48 in"),
            ("1 km to km", "1 km"),
            ("0 km to mi", "0 mi"),
            ("-1 km to m", "-1000 m"),
            // Long names, shown as the symbol
            ("1 meters to feet", "3.280839895 ft"),
            ("1 kilometre to miles", "0.6213711922 mi"),
            ("1 hour to seconds", "3600 s"),
            ("1 kilo to pounds", "2.204622622 lb"),
            ("100 celsius to fahrenheit", "212 F"),
            ("1 gallon to liters", "3.785411784 L"),
            ("1 hectare to acres", "2.471053815 ac"),
            ("1 kmh to kph", "1 km/h"),
            ("1 byte to bits", "8 b"),
            ("1 kilobyte to bytes", "1000 B"),
            // Any letter case, when it fits one unit
            ("1 KM to M", "1000 m"),
            ("1 Km TO Mm", "1000000 mm"),
            ("100 c to f", "212 F"),
            ("1 l to ml", "1000 mL"),
            ("1 HOUR to MIN", "60 min"),
            ("1 KB to B", "1000 B"),
            // Without a conversion
            ("1+1", "2"),
            ("1234567.5", "1234567.5"),
        ] {
            assert_eq!(text(line), expected, "{line}");
        }
    }

    #[test]
    fn conversions_group_thousands_before_the_unit() {
        let calculation = calculator_at(friday()).calculate("1000 km to m", &Names::new(), true);
        assert_eq!(calculation.unwrap().text, "1,000,000 m");
    }

    #[test]
    fn conversion_errors_say_where_and_why() {
        for (line, message, position) in [
            ("1 mb to kB", "Unit 'mb' could be Mb or MB", 2),
            ("1 kB to gb", "Unit 'gb' could be Gb or GB", 8),
            ("1 tb to kB", "Unit 'tb' could be Tb or TB", 2),
            ("1 xyz to m", "Unknown unit 'xyz'", 2),
            ("1 m to xyz", "Unknown unit 'xyz'", 7),
            ("1 km to kg", "Can't convert km to kg", 5),
            ("1 C to km", "Can't convert C to km", 4),
            ("1/0 km to m", "Division by zero", 1),
            ("79228162514264337593543950335 km to nm", "Number is too large", 33),
        ] {
            let error = error(line);
            assert_eq!((error.message.as_str(), error.position), (message, position), "{line}");
        }
    }

    #[test]
    fn dates_move_by_lengths_of_time_and_are_a_length_of_time_apart() {
        for (line, expected) in [
            ("today", "2026-11-13 (Friday)"),
            ("today + 30 d", "2026-12-13 (Sunday)"),
            ("today + 30", "2026-12-13 (Sunday)"),
            ("30 days + today", "2026-12-13 (Sunday)"),
            ("today - 2 wk", "2026-10-30 (Friday)"),
            ("today + 1 yr", "2027-11-13 (Saturday)"),
            ("2026-01-31 + 1 month", "2026-02-28 (Saturday)"),
            ("2026-1-5", "2026-01-05 (Monday)"),
            ("2026-12-25 - today", "42 days"),
            ("today - 2026-12-25", "-42 days"),
            ("2026-12-25 - 2026-12-25", "0 days"),
            ("2 wk + 3 d", "17 days"),
            ("1 yr + 2 mo", "14 months"),
            ("2 * 1 wk", "14 days"),
            ("1 month", "1 month"),
            ("-1 d", "-1 day"),
            ("1 mo + 3 d", "1 month, 3 days"),
            ("(2026-12-25 - today) / 2", "21 days"),
            // Before 'to', a unit of time is a conversion's; spaced out, a date is a sum.
            ("1 yr to d", "365.25 d"),
            ("1 wk to h", "168 h"),
            ("2026 - 12 - 25", "1989"),
        ] {
            assert_eq!(text(line), expected, "{line}");
        }
    }

    #[test]
    fn dates_that_cant_be_calculated_say_why() {
        for (line, message, position) in [
            ("today * 2", "Can't multiply a date and a number", 6),
            ("today + today", "Can't add a date and a date", 6),
            ("today + 1.5 d", "A date moves by whole days and whole months", 6),
            ("2026-02-30", "There's no such date as 2026-02-30", 0),
            ("sqrt(today)", "Expected a number, not a date", 5),
            ("-today", "A date can't be negative", 0),
            ("9999-12-31 + 1", "The date is out of range", 11),
        ] {
            let error = error(line);
            assert_eq!((error.message.as_str(), error.position), (message, position), "{line}");
        }
    }

    #[test]
    fn an_assignment_gives_the_value_and_the_name() {
        let calculation = calculate("Rate = 5 km to m").unwrap();
        assert_eq!(calculation.text, "5000 m");
        assert_eq!(calculation.value, Some(Value::Number(Decimal::from(5000))));
        assert_eq!(calculation.assigned_name.as_deref(), Some("Rate"));
    }

    #[test]
    fn names_given_earlier_are_used_in_any_letter_case() {
        let names = Names::from([
            ("x".to_string(), Value::Number(Decimal::from(5))),
            ("ans".to_string(), Value::Number(Decimal::from(3))),
        ]);
        for (line, expected) in [("x * 2", "10"), ("X * 2", "10"), ("ans + x", "8"), ("y = ans * x", "15")] {
            assert_eq!(calculate_with(line, &names).unwrap().text, expected, "{line}");
        }
    }

    #[test]
    fn a_name_can_keep_a_date() {
        let christmas = calculate("christmas = 2026-12-25").unwrap().value.unwrap();
        let names = Names::from([("christmas".to_string(), christmas)]);
        assert_eq!(calculate_with("christmas - today", &names).unwrap().text, "42 days");
    }

    #[test]
    fn names_that_cant_be_used_say_why() {
        for (line, message) in [
            ("pi = 3", "'pi' is a constant, so it can't be given a value"),
            ("sqrt = 2", "'sqrt' is a function, so it can't be given a value"),
            ("ANS = 2", "'ANS' is the last answer, so it can't be given a value"),
            ("today = 2", "'today' is today's date, so it can't be given a value"),
            ("to = 2", "'to' is a word the calculator uses, so it can't be given a value"),
            ("x =", "Expected a value after '='"),
            ("x = 10:30 Mexico to India", "A time in another place can't be kept in a name"),
            ("ans * 2", "There's no last answer yet"),
            ("x * 2", "Unknown name 'x'"),
            ("5 = 3", "Unexpected '='"),
        ] {
            assert_eq!(error(line).message, message, "{line}");
        }
    }

    #[test]
    fn a_time_in_another_place_has_no_value_to_keep() {
        let calculation = calculate("10:00 to UTC").unwrap();
        assert_eq!(calculation.value, None);
        assert_eq!(calculation.assigned_name, None);
    }
}
