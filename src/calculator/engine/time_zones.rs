//! Converts a time between places: `10:30 Mexico to India` is `22:00 India (UTC+5:30)`.
//!
//! The time is taken as today's in the place it's in, because daylight saving time changes the
//! difference: 10:00 in Mexico City is 11:00 in Miami in winter and 12:00 in summer.
//!
//! A place is one of [`PLACES`] (any letter case, `_` or a space between words), "local", UTC or
//! GMT with an offset or without (UTC-5, GMT+5:30), or an IANA time zone such as America/Lima.
//! Abbreviations like CST or IST are left out on purpose: each means more than one time zone.

use chrono::offset::LocalResult;
use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime, Offset, TimeZone, Utc};
use chrono_tz::{OffsetComponents, Tz};

use super::parser::TimeConversion;
use super::{CalcError, Result};

pub const LOCAL: &str = "local";

/// UTC offsets in use go from -12 to +14 hours.
const MAX_OFFSET_HOURS: u32 = 14;

/// Gives the calculator the time now and the computer's time zone (tests use a fixed one).
pub trait Clock {
    fn now(&self) -> DateTime<Utc>;
    fn local_zone(&self) -> Zone;

    /// Today's date where the computer is.
    fn today(&self) -> NaiveDate {
        let now = self.now().naive_utc();
        self.local_zone().utc_to_local(now).date()
    }
}

/// The computer's clock and time zone.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }

    fn local_zone(&self) -> Zone {
        iana_time_zone::get_timezone()
            .ok()
            .and_then(|id| id.parse::<Tz>().ok())
            .map_or_else(|| Zone::Fixed(*chrono::Local::now().offset()), Zone::Named)
    }
}

/// A time zone: one with rules, such as daylight saving time, or a fixed offset from UTC.
#[derive(Debug, Clone, Copy)]
pub enum Zone {
    Named(Tz),
    Fixed(FixedOffset),
}

impl Zone {
    /// The zone's offset from UTC at that moment, in seconds.
    pub fn offset_seconds(&self, utc: NaiveDateTime) -> i32 {
        match self {
            Self::Named(tz) => tz.offset_from_utc_datetime(&utc).fix().local_minus_utc(),
            Self::Fixed(offset) => offset.local_minus_utc(),
        }
    }

    pub fn utc_to_local(&self, utc: NaiveDateTime) -> NaiveDateTime {
        utc + chrono::TimeDelta::seconds(i64::from(self.offset_seconds(utc)))
    }

    /// The moment a time on the zone's clocks is; `None` for a time the clocks skip when they
    /// move forward. A time that happens twice, when they move back, is taken in standard time.
    fn local_to_utc(&self, local: NaiveDateTime) -> Option<NaiveDateTime> {
        match self {
            Self::Fixed(offset) => Some(local - chrono::TimeDelta::seconds(i64::from(offset.local_minus_utc()))),
            Self::Named(tz) => match tz.from_local_datetime(&local) {
                LocalResult::Single(time) => Some(time.naive_utc()),
                LocalResult::Ambiguous(first, second) => {
                    let standard = if first.offset().dst_offset().is_zero() { first } else { second };
                    Some(standard.naive_utc())
                }
                LocalResult::None => None,
            },
        }
    }
}

/// A place whose time can be converted.
pub struct Place {
    /// The name shown after a result.
    pub name: &'static str,
    /// Its IANA time zone, which knows its daylight saving time rules.
    pub zone_id: &'static str,
    /// Other names lines can use, such as "cdmx".
    pub other_names: &'static [&'static str],
}

impl Place {
    pub fn all_names(&self) -> impl Iterator<Item = &'static str> {
        std::iter::once(self.name).chain(self.other_names.iter().copied())
    }

    pub fn zone(&self) -> Tz {
        self.zone_id.parse().expect("PLACES only has known time zones")
    }
}

const fn place(name: &'static str, zone_id: &'static str, other_names: &'static [&'static str]) -> Place {
    Place { name, zone_id, other_names }
}

pub static PLACES: &[Place] = &[
    // Mexico has had no daylight saving time since 2022, except along the US border.
    place("Mexico City", "America/Mexico_City", &["mexico", "cdmx", "ciudad de mexico"]),
    place("Guadalajara", "America/Mexico_City", &[]),
    place("Monterrey", "America/Monterrey", &[]),
    place("Tijuana", "America/Tijuana", &[]),
    place("Cancun", "America/Cancun", &[]),
    // United States and Canada
    place("Miami", "America/New_York", &[]),
    place("New York", "America/New_York", &["nyc"]),
    place("Chicago", "America/Chicago", &[]),
    place("Denver", "America/Denver", &[]),
    place("Los Angeles", "America/Los_Angeles", &[]),
    place("Toronto", "America/Toronto", &[]),
    // South America
    place("Bogotá", "America/Bogota", &["bogota", "colombia"]),
    place("Cali", "America/Bogota", &[]),
    place("Medellín", "America/Bogota", &["medellin"]),
    place("Lima", "America/Lima", &["peru"]),
    place("Santiago", "America/Santiago", &["chile"]),
    place("Buenos Aires", "America/Argentina/Buenos_Aires", &["argentina"]),
    place("São Paulo", "America/Sao_Paulo", &["sao paulo", "saopaulo", "brazil", "brasil"]),
    place("Rio de Janeiro", "America/Sao_Paulo", &["rio"]),
    // Europe
    place("London", "Europe/London", &[]),
    place("Madrid", "Europe/Madrid", &["spain", "espana"]),
    place("Paris", "Europe/Paris", &["france"]),
    place("Berlin", "Europe/Berlin", &["germany"]),
    // Asia and Oceania
    place("India", "Asia/Kolkata", &["delhi", "new delhi", "mumbai", "kolkata", "bangalore", "bengaluru"]),
    place("China", "Asia/Shanghai", &["beijing", "shanghai", "shenzhen"]),
    place("Hong Kong", "Asia/Hong_Kong", &[]),
    place("Tokyo", "Asia/Tokyo", &["japan"]),
    place("Singapore", "Asia/Singapore", &[]),
    place("Dubai", "Asia/Dubai", &[]),
    place("Sydney", "Australia/Sydney", &[]),
];

/// Finds a place or time zone by any of its names; gives the name to show and the zone.
/// `local` is the zone "local" means; `position` is where the name is, to report an error.
pub fn find(name: &str, position: usize, local: Zone) -> Result<(String, Zone)> {
    let words = name.replace('_', " ");

    if words.eq_ignore_ascii_case(LOCAL) {
        return Ok((LOCAL.to_string(), local));
    }

    if let Some(place) =
        PLACES.iter().find(|place| place.all_names().any(|n| n.eq_ignore_ascii_case(&words)))
    {
        return Ok((place.name.to_string(), Zone::Named(place.zone())));
    }

    if let Some(offset) = parse_utc_offset(name) {
        return from_utc_offset(offset, position);
    }

    // America/Lima
    if let Ok(tz) = Tz::from_str_insensitive(name) {
        return Ok((name.to_string(), Zone::Named(tz)));
    }

    Err(CalcError::new(format!("Unknown time zone '{name}'"), position))
}

/// UTC or GMT, then optionally a sign, 1 or 2 digits of hours, and ':' and 2 digits of minutes.
/// Gives `None` for anything else, and `Some(None)` for UTC alone.
fn parse_utc_offset(name: &str) -> Option<Option<(bool, u32, u32)>> {
    let prefix = name.get(..3)?;
    if !prefix.eq_ignore_ascii_case("utc") && !prefix.eq_ignore_ascii_case("gmt") {
        return None;
    }
    let rest = &name[3..];
    if rest.is_empty() {
        return Some(None);
    }

    let negative = match rest.as_bytes()[0] {
        b'+' => false,
        b'-' => true,
        _ => return None,
    };
    let (hours, minutes) = match rest[1..].split_once(':') {
        Some((hours, minutes)) => (hours, Some(minutes)),
        None => (&rest[1..], None),
    };
    let all_digits = |text: &str, lengths: std::ops::RangeInclusive<usize>| {
        lengths.contains(&text.len()) && text.bytes().all(|b| b.is_ascii_digit())
    };
    if !all_digits(hours, 1..=2) || minutes.is_some_and(|m| !all_digits(m, 2..=2)) {
        return None;
    }
    let minutes = minutes.map_or(0, |m| m.parse().unwrap_or(0));
    Some(Some((negative, hours.parse().unwrap_or(0), minutes)))
}

fn from_utc_offset(offset: Option<(bool, u32, u32)>, position: usize) -> Result<(String, Zone)> {
    let Some((negative, hours, minutes)) = offset else {
        return Ok(("UTC".to_string(), Zone::Fixed(FixedOffset::east_opt(0).expect("0 is valid"))));
    };
    if minutes > 59 || hours > MAX_OFFSET_HOURS || (hours == MAX_OFFSET_HOURS && minutes > 0) {
        return Err(CalcError::new(
            format!("A UTC offset must be from UTC-{MAX_OFFSET_HOURS} to UTC+{MAX_OFFSET_HOURS}"),
            position,
        ));
    }
    let size = (hours * 3600 + minutes * 60) as i32;
    let seconds = if negative { -size } else { size };
    let zone = FixedOffset::east_opt(seconds).expect("within ±14 hours");
    Ok((format_offset(seconds), Zone::Fixed(zone)))
}

/// Converts the time and writes the result: `22:00 India (UTC+5:30)`, with `, next day` or
/// `, previous day` when the day changes.
pub fn convert(node: &TimeConversion, clock: &dyn super::Clock) -> Result<String> {
    let local = clock.local_zone();
    let (from_name, from_zone) = match &node.from {
        None => (LOCAL.to_string(), local),
        Some(from) => find(from, node.from_position, local)?,
    };
    let (to_name, to_zone) = find(&node.to, node.to_position, local)?;

    let from_now = from_zone.utc_to_local(clock.now().naive_utc());
    let from_time = match node.time {
        Some(time) => from_now.date().and_time(time),
        None => from_now,
    };

    let utc = from_zone.local_to_utc(from_time).ok_or_else(|| {
        CalcError::new(
            format!(
                "{} doesn't exist in {from_name} today: the clocks move forward",
                format_time(from_time, node.twelve_hour)
            ),
            node.time_position,
        )
    })?;

    let to_time = to_zone.utc_to_local(utc);
    let days = (to_time.date() - from_time.date()).num_days();

    // "UTC-5 (UTC-5)" would say it twice.
    let offset = format_offset(to_zone.offset_seconds(utc));
    let place = if to_name == offset { to_name } else { format!("{to_name} ({offset})") };

    Ok(format!("{} {place}{}", format_time(to_time, node.twelve_hour), format_days(days)))
}

/// A UTC offset the short way: UTC, UTC-5, UTC+5:30.
pub fn format_offset(seconds: i32) -> String {
    if seconds == 0 {
        return "UTC".to_string();
    }
    let sign = if seconds < 0 { '-' } else { '+' };
    let size = seconds.unsigned_abs();
    let (hours, minutes) = (size / 3600, size % 3600 / 60);
    if minutes == 0 {
        format!("UTC{sign}{hours}")
    } else {
        format!("UTC{sign}{hours}:{minutes:02}")
    }
}

fn format_time(time: NaiveDateTime, twelve_hour: bool) -> String {
    use chrono::Timelike;
    let (hour, minute) = (time.hour(), time.minute());
    if !twelve_hour {
        return format!("{hour}:{minute:02}");
    }
    let half = if hour < 12 { "am" } else { "pm" };
    let hour = match hour % 12 {
        0 => 12,
        hour => hour,
    };
    format!("{hour}:{minute:02} {half}")
}

fn format_days(days: i64) -> String {
    match days {
        0 => String::new(),
        1 => ", next day".to_string(),
        -1 => ", previous day".to_string(),
        days => format!(", {days:+} days"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calculator::engine::testing::{FixedClock, calculator_at};

    const MEXICO_CITY: &str = "America/Mexico_City";

    /// 10:00 in Mexico City; Miami is on standard time (UTC-5).
    const WINTER: (i32, u32, u32, u32) = (2026, 1, 15, 16);
    /// 10:00 in Mexico City, which has no daylight saving time; Miami is on it (UTC-4).
    const SUMMER: (i32, u32, u32, u32) = (2026, 7, 15, 16);

    fn convert_at(line: &str, (year, month, day, hour): (i32, u32, u32, u32)) -> Result<String> {
        calculator_at(FixedClock::at(year, month, day, hour, 0, MEXICO_CITY))
            .calculate(line, &Default::default(), true)
            .map(|calculation| calculation.text)
    }

    fn convert(line: &str) -> String {
        convert_at(line, WINTER).unwrap_or_else(|error| panic!("{line}: {error}"))
    }

    #[test]
    fn converts_a_time_between_places() {
        for (line, expected) in [
            ("10:00 Mexico to Miami", "11:00 Miami (UTC-5)"),
            ("10:00 Mexico to Cali", "11:00 Cali (UTC-5)"),
            ("10:00 mexico to bogota", "11:00 Bogotá (UTC-5)"),
            ("10:00 Mexico to Sao Paulo", "13:00 São Paulo (UTC-3)"),
            ("10:00 Mexico to India", "21:30 India (UTC+5:30)"),
            ("10:00 Mexico to China", "0:00 China (UTC+8), next day"),
            ("11:00 Miami to Mexico", "10:00 Mexico City (UTC-6)"),
            ("21:30 india to cdmx", "10:00 Mexico City (UTC-6)"),
            ("8:00 China to Bogota", "19:00 Bogotá (UTC-5), previous day"),
        ] {
            assert_eq!(convert(line), expected, "{line}");
        }
    }

    #[test]
    fn follows_daylight_saving_time() {
        assert_eq!(convert_at("10:00 Mexico to Miami", WINTER).unwrap(), "11:00 Miami (UTC-5)");
        assert_eq!(convert_at("10:00 Mexico to Miami", SUMMER).unwrap(), "12:00 Miami (UTC-4)");
    }

    #[test]
    fn with_am_or_pm_answers_the_same_way() {
        for (line, expected) in [
            ("3 pm Mexico to Miami", "4:00 pm Miami (UTC-5)"),
            ("3:30 PM mexico to utc", "9:30 pm UTC"),
            ("12 am Mexico to UTC", "6:00 am UTC"),
            ("12 pm Mexico to UTC", "6:00 pm UTC"),
            ("11:30 pm Mexico to UTC", "5:30 am UTC, next day"),
        ] {
            assert_eq!(convert(line), expected, "{line}");
        }
    }

    #[test]
    fn without_a_place_uses_the_local_time_zone() {
        for (line, expected) in [
            ("10:00 to India", "21:30 India (UTC+5:30)"),
            ("10:00 local to UTC", "16:00 UTC"),
            ("now to India", "21:30 India (UTC+5:30)"),
            ("now Miami to China", "0:00 China (UTC+8), next day"),
            ("NOW to local", "10:00 local (UTC-6)"),
        ] {
            assert_eq!(convert(line), expected, "{line}");
        }
    }

    #[test]
    fn takes_time_zone_ids_and_utc_offsets() {
        for (line, expected) in [
            ("10:00 sao_paulo to India", "18:30 India (UTC+5:30)"),
            ("10:00 America/Lima to Mexico", "9:00 Mexico City (UTC-6)"),
            ("10:00 Mexico to Asia/Tokyo", "1:00 Asia/Tokyo (UTC+9), next day"),
            ("10:00 Mexico to utc-3", "13:00 UTC-3"),
            ("10:00 utc+5:30 to GMT", "4:30 UTC"),
            ("10:00 UTC to gmt-3:30", "6:30 UTC-3:30"),
        ] {
            assert_eq!(convert(line), expected, "{line}");
        }
    }

    #[test]
    fn a_time_the_clocks_skip_is_refused() {
        // On 8 March 2026, Miami's clocks went from 2:00 to 3:00.
        let error = convert_at("2:30 Miami to UTC", (2026, 3, 8, 12)).unwrap_err();
        assert_eq!(error.message, "2:30 doesn't exist in Miami today: the clocks move forward");
        assert_eq!(error.position, 0);
    }

    #[test]
    fn a_time_that_happens_twice_is_taken_in_standard_time() {
        // On 1 November 2026, Miami's clocks went from 2:00 back to 1:00.
        assert_eq!(convert_at("1:30 Miami to UTC", (2026, 11, 1, 12)).unwrap(), "6:30 UTC");
    }

    #[test]
    fn errors_say_where_and_why() {
        for (line, message, position) in [
            ("10:00", "Expected a time zone or 'to' after the time", 5),
            ("10:00 Mexico", "Expected 'to' after 'Mexico'", 12),
            ("10:00 Mexico 5 to India", "Expected 'to' after 'Mexico'", 13),
            ("10:00 to", "Expected a time zone after 'to'", 8),
            ("10:00 to India 5", "Unexpected '5'", 15),
            ("10: to UTC", "Expected minutes after ':'", 3),
            ("24:00 to UTC", "The hour must be from 0 to 23", 0),
            ("13 pm to UTC", "With am or pm, the hour must be from 1 to 12", 0),
            ("0:30 am to UTC", "With am or pm, the hour must be from 1 to 12", 0),
            ("10:5 to UTC", "The minutes must be two digits, from 00 to 59", 3),
            ("10:60 to UTC", "The minutes must be two digits, from 00 to 59", 3),
            ("1.5 pm to UTC", "Write a time as 10:30 or 10:30 pm", 0),
            ("10:00 Narnia to UTC", "Unknown time zone 'Narnia'", 6),
            ("10:00 to new narnia", "Unknown time zone 'new narnia'", 9),
            ("10:00 to UTC+15", "A UTC offset must be from UTC-14 to UTC+14", 9),
            ("10:00 to UTC+5:75", "A UTC offset must be from UTC-14 to UTC+14", 9),
        ] {
            let error = convert_at(line, WINTER).unwrap_err();
            assert_eq!((error.message.as_str(), error.position), (message, position), "{line}");
        }
    }

    #[test]
    fn a_number_without_am_or_pm_is_still_a_number() {
        assert_eq!(convert("3 h to min"), "180 min");
        assert_eq!(convert("2 + 3"), "5");
    }

    #[test]
    fn places_are_all_known_and_no_name_is_used_twice() {
        let utc = Zone::Fixed(FixedOffset::east_opt(0).unwrap());
        let mut seen = std::collections::HashSet::new();
        for place in PLACES {
            assert_eq!(find(place.name, 0, utc).unwrap().0, place.name);
            for name in place.all_names() {
                assert!(seen.insert(name.to_ascii_lowercase()), "{name} is used twice");
            }
        }
    }
}
