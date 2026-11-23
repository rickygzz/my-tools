//! The units conversions can use (10 km to mi).
//!
//! A name matches exactly first, because case can matter: MB is megabytes and Mb megabits.
//! Otherwise any letter case works, as long as it only fits one unit: KM is km, but mb could be
//! either of those two.
//!
//! Base units: meter, kilogram, kelvin, square meter, liter, second, km/h and byte, chosen so the
//! factors are exact wherever possible.

use rust_decimal::Decimal;
use rust_decimal_macros::dec;

use super::CalcError;
use super::math::{MathResult, checked, divide};

/// What a unit measures. Only units of the same group convert to each other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Length,
    Mass,
    Temperature,
    Area,
    Volume,
    Time,
    Speed,
    Data,
}

impl Group {
    pub fn name(self) -> &'static str {
        match self {
            Self::Length => "length",
            Self::Mass => "mass",
            Self::Temperature => "temperature",
            Self::Area => "area",
            Self::Volume => "volume",
            Self::Time => "time",
            Self::Speed => "speed",
            Self::Data => "data",
        }
    }
}

/// A unit, defined by how a value in it becomes a value in its group's base unit:
/// `(value + offset) * factor / divisor`.
#[derive(Debug, PartialEq)]
pub struct Unit {
    /// The name shown after a result.
    pub symbol: &'static str,
    pub group: Group,
    /// Other names lines can use, such as "meters".
    pub names: &'static [&'static str],
    /// How many base units one of this unit is.
    factor: Decimal,
    /// Only temperatures have one, because their zeros differ.
    offset: Decimal,
    /// For a factor a Decimal can't hold exactly, like Fahrenheit's 5/9, which would turn 32 F
    /// into a tiny number instead of 0 C. Multiplying before dividing keeps these exact.
    divisor: Decimal,
}

impl Unit {
    /// A value in this unit, in the base unit: 1 km is 1000 m.
    pub fn to_base(&self, value: Decimal) -> MathResult {
        let shifted = checked(value.checked_add(self.offset))?;
        divide(checked(shifted.checked_mul(self.factor))?, self.divisor)
    }

    /// A value in the base unit, in this unit: 1000 m is 1 km.
    pub fn to_unit(&self, value: Decimal) -> MathResult {
        let scaled = divide(checked(value.checked_mul(self.divisor))?, self.factor)?;
        checked(scaled.checked_sub(self.offset))
    }

    /// The symbol and the other names.
    pub fn all_names(&self) -> impl Iterator<Item = &'static str> {
        std::iter::once(self.symbol).chain(self.names.iter().copied())
    }
}

const fn unit(
    symbol: &'static str,
    group: Group,
    factor: Decimal,
    names: &'static [&'static str],
) -> Unit {
    Unit { symbol, group, names, factor, offset: Decimal::ZERO, divisor: Decimal::ONE }
}

use Group::*;

pub static UNITS: &[Unit] = &[
    // Length: the inch is exactly 25.4 mm, the mile 1760 yards, the nautical mile 1852 m.
    unit("nm", Length, dec!(0.000000001), &["nanometer", "nanometers", "nanometre", "nanometres"]),
    unit("um", Length, dec!(0.000001), &["micrometer", "micrometers", "micrometre", "micrometres", "micron", "microns"]),
    unit("mm", Length, dec!(0.001), &["millimeter", "millimeters", "millimetre", "millimetres"]),
    unit("cm", Length, dec!(0.01), &["centimeter", "centimeters", "centimetre", "centimetres"]),
    unit("m", Length, dec!(1), &["meter", "meters", "metre", "metres"]),
    unit("km", Length, dec!(1000), &["kilometer", "kilometers", "kilometre", "kilometres"]),
    unit("in", Length, dec!(0.0254), &["inch", "inches"]),
    unit("ft", Length, dec!(0.3048), &["foot", "feet"]),
    unit("yd", Length, dec!(0.9144), &["yard", "yards"]),
    unit("mi", Length, dec!(1609.344), &["mile", "miles"]),
    unit("nmi", Length, dec!(1852), &[]),
    // Mass: the pound is exactly 0.45359237 kg; ton is the US short ton (2000 lb), t the metric.
    unit("mg", Mass, dec!(0.000001), &["milligram", "milligrams"]),
    unit("g", Mass, dec!(0.001), &["gram", "grams"]),
    unit("kg", Mass, dec!(1), &["kilogram", "kilograms", "kilo", "kilos"]),
    unit("t", Mass, dec!(1000), &["tonne", "tonnes"]),
    unit("oz", Mass, dec!(0.028349523125), &["ounce", "ounces"]),
    unit("lb", Mass, dec!(0.45359237), &["lbs", "pound", "pounds"]),
    unit("st", Mass, dec!(6.35029318), &["stone", "stones"]),
    unit("ton", Mass, dec!(907.18474), &["tons"]),
    // Temperature
    unit("K", Temperature, dec!(1), &["kelvin"]),
    Unit { offset: dec!(273.15), ..unit("C", Temperature, dec!(1), &["celsius"]) },
    Unit {
        offset: dec!(459.67),
        divisor: dec!(9),
        ..unit("F", Temperature, dec!(5), &["fahrenheit"])
    },
    // Area
    unit("mm2", Area, dec!(0.000001), &["mm^2"]),
    unit("cm2", Area, dec!(0.0001), &["cm^2"]),
    unit("m2", Area, dec!(1), &["m^2"]),
    unit("km2", Area, dec!(1000000), &["km^2"]),
    unit("ha", Area, dec!(10000), &["hectare", "hectares"]),
    unit("in2", Area, dec!(0.00064516), &["in^2"]),
    unit("ft2", Area, dec!(0.09290304), &["ft^2"]),
    unit("yd2", Area, dec!(0.83612736), &["yd^2"]),
    unit("ac", Area, dec!(4046.8564224), &["acre", "acres"]),
    unit("mi2", Area, dec!(2589988.110336), &["mi^2"]),
    // Volume: gal and its parts are US (231 cubic inches); ukgal is the imperial gallon.
    unit("mL", Volume, dec!(0.001), &["milliliter", "milliliters", "millilitre", "millilitres"]),
    unit("cm3", Volume, dec!(0.001), &["cm^3", "cc"]),
    unit("L", Volume, dec!(1), &["liter", "liters", "litre", "litres"]),
    unit("m3", Volume, dec!(1000), &["m^3"]),
    unit("in3", Volume, dec!(0.016387064), &["in^3"]),
    unit("ft3", Volume, dec!(28.316846592), &["ft^3"]),
    unit("tsp", Volume, dec!(0.00492892159375), &["teaspoon", "teaspoons"]),
    unit("tbsp", Volume, dec!(0.01478676478125), &["tablespoon", "tablespoons"]),
    unit("floz", Volume, dec!(0.0295735295625), &[]),
    unit("cup", Volume, dec!(0.2365882365), &["cups"]),
    unit("pt", Volume, dec!(0.473176473), &["pint", "pints"]),
    unit("qt", Volume, dec!(0.946352946), &["quart", "quarts"]),
    unit("gal", Volume, dec!(3.785411784), &["gallon", "gallons"]),
    unit("ukgal", Volume, dec!(4.54609), &[]),
    // Time: yr is the average year, 365.25 days.
    unit("ms", Time, dec!(0.001), &["millisecond", "milliseconds"]),
    unit("s", Time, dec!(1), &["sec", "secs", "second", "seconds"]),
    unit("min", Time, dec!(60), &["mins", "minute", "minutes"]),
    unit("h", Time, dec!(3600), &["hr", "hrs", "hour", "hours"]),
    unit("d", Time, dec!(86400), &["day", "days"]),
    unit("wk", Time, dec!(604800), &["week", "weeks"]),
    unit("yr", Time, dec!(31557600), &["year", "years"]),
    // Speed
    unit("km/h", Speed, dec!(1), &["kmh", "kph"]),
    unit("m/s", Speed, dec!(3.6), &[]),
    unit("mph", Speed, dec!(1.609344), &["mi/h"]),
    unit("ft/s", Speed, dec!(1.09728), &[]),
    unit("knot", Speed, dec!(1.852), &["knots", "kn"]),
    // Data: kB, MB... are powers of 1000 and KiB, MiB... of 1024; lowercase b is bits.
    unit("b", Data, dec!(0.125), &["bit", "bits"]),
    unit("kb", Data, dec!(125), &["Kb", "kbit"]),
    unit("Mb", Data, dec!(125000), &["Mbit"]),
    unit("Gb", Data, dec!(125000000), &["Gbit"]),
    unit("Tb", Data, dec!(125000000000), &["Tbit"]),
    unit("B", Data, dec!(1), &["byte", "bytes"]),
    unit("kB", Data, dec!(1000), &["KB", "kilobyte", "kilobytes"]),
    unit("MB", Data, dec!(1000000), &["megabyte", "megabytes"]),
    unit("GB", Data, dec!(1000000000), &["gigabyte", "gigabytes"]),
    unit("TB", Data, dec!(1000000000000), &["terabyte", "terabytes"]),
    unit("PB", Data, dec!(1000000000000000), &["petabyte", "petabytes"]),
    unit("KiB", Data, dec!(1024), &[]),
    unit("MiB", Data, dec!(1048576), &[]),
    unit("GiB", Data, dec!(1073741824), &[]),
    unit("TiB", Data, dec!(1099511627776), &[]),
    unit("PiB", Data, dec!(1125899906842624), &[]),
];

/// Finds a unit by any of its names. `position` is where the name is, to report an error.
pub fn find(name: &str, position: usize) -> Result<&'static Unit, CalcError> {
    if let Some(unit) = UNITS.iter().find(|unit| unit.all_names().any(|n| n == name)) {
        return Ok(unit);
    }

    let matches: Vec<&Unit> = UNITS
        .iter()
        .filter(|unit| unit.all_names().any(|n| n.eq_ignore_ascii_case(name)))
        .collect();

    match matches.as_slice() {
        [] => Err(CalcError::new(format!("Unknown unit '{name}'"), position)),
        [unit] => Ok(unit),
        several => {
            let symbols: Vec<&str> = several.iter().map(|unit| unit.symbol).collect();
            Err(CalcError::new(
                format!("Unit '{name}' could be {}", symbols.join(" or ")),
                position,
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calculator::engine::math::round_to_significant_digits;
    use std::collections::HashSet;

    #[test]
    fn every_name_is_used_once_and_every_symbol_finds_its_unit() {
        let mut seen = HashSet::new();
        for unit in UNITS {
            for name in unit.all_names() {
                assert!(seen.insert(name), "{name} is used twice");
            }
            assert!(std::ptr::eq(find(unit.symbol, 0).unwrap(), unit));
        }
    }

    #[test]
    fn converting_there_and_back_gives_the_same_value() {
        let value = dec!(12.5);
        for from in UNITS {
            for to in UNITS.iter().filter(|to| to.group == from.group) {
                let there = to.to_unit(from.to_base(value).unwrap()).unwrap();
                let back = from.to_unit(to.to_base(there).unwrap()).unwrap();
                assert_eq!(round_to_significant_digits(back, 10), value, "{} {}", from.symbol, to.symbol);
            }
        }
    }
}
