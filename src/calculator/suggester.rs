//! What the calculator offers to write while a line is typed, from what's before the caret
//! (only when the caret is at the end):
//!
//! - after a time (`10:30 `, `3 pm `, `now `), the places to convert it from, then after `to`
//!   the places to convert it to;
//! - after a number, the units, then after `to` the units of the same kind; with a date in the
//!   line, the lengths of time to move it by;
//! - where a value starts (at the start, or after an operator, `(`, `,` or `=`), the functions,
//!   constants and names given values whose name starts with the letters typed.
//!
//! Choosing a first place or unit writes `to` after it, so the second ones follow. Only a single
//! number shows every unit as soon as a space follows it; after anything else the units wait for
//! a letter, so they don't pop up at every space of a sum.

use std::sync::LazyLock;

use chrono::{DateTime, Utc};
use regex::{Captures, Match, Regex};

use super::engine::catalog::{IPCONFIG, PLACES, UNITS, Unit, find_unit, format_offset};
use super::engine::{LAST_ANSWER, Names, TODAY, Value, Zone, find_name};

/// Something to write in place of what's being typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    /// What the list shows: "Mexico City (UTC-6)", "km (kilometer, length)".
    pub text: String,
    /// What goes in the line: "Mexico City to ", "mi".
    pub insert: String,
}

/// What to offer, and the part of the line a choice replaces (in characters).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestions {
    pub start: usize,
    /// 0 when nothing of it is typed yet.
    pub length: usize,
    /// Never empty.
    pub items: Vec<Suggestion>,
}

/// How each function is written and what it gives. Every function of the engine needs a line
/// here; a test checks.
pub const FUNCTION_HELP: [(&str, &str, &str); 16] = [
    ("abs", "abs(x)", "absolute value"),
    ("sqrt", "sqrt(x)", "square root"),
    ("round", "round(x, decimals)", "rounds; decimals are optional"),
    ("floor", "floor(x)", "rounds down"),
    ("ceil", "ceil(x)", "rounds up"),
    ("min", "min(a, ...)", "smallest"),
    ("max", "max(a, ...)", "largest"),
    ("sin", "sin(x)", "sine, in radians"),
    ("cos", "cos(x)", "cosine, in radians"),
    ("tan", "tan(x)", "tangent, in radians"),
    ("ln", "ln(x)", "natural log"),
    ("log", "log(x)", "base-10 log"),
    ("exp", "exp(x)", "e to the power x"),
    ("len", "len(\"text\")", "length of a text"),
    ("find", "find(\"part\", \"text\", start)", "position of the part; start is optional"),
    (IPCONFIG, "ipconfig()", "IP address and gateway"),
];

/// The names that stand for a value.
const VALUES: [(&str, &str); 4] = [
    ("pi", "3.14159..."),
    ("e", "2.71828..."),
    (LAST_ANSWER, "the last answer"),
    (TODAY, "today's date"),
];

/// What a date moves by.
const DURATIONS: [(&str, &str); 4] = [("d", "days"), ("wk", "weeks"), ("mo", "months"), ("yr", "years")];

const TO: &str = " to ";

fn regex(pattern: &str) -> Regex {
    Regex::new(pattern).expect("the pattern is valid")
}

/// A time the calculator converts, then a space: 10:30, 10:30 pm, 3 pm, now.
static TIME: LazyLock<Regex> = LazyLock::new(|| {
    regex(r"(?i)^(?P<time>\s*(?:now|(?P<h1>\d{1,2}):\d{2}(?:\s*(?P<ap1>[ap]m))?|(?P<h2>\d{1,2})\s*(?P<ap2>[ap]m)))\s")
});
/// After the time: a place or none, then to, then the place being typed.
static DESTINATION: LazyLock<Regex> =
    LazyLock::new(|| regex(r"(?i)^\s+(?:\S.*?\s+)?to\s+(?P<query>.*)$"));
static SOURCE: LazyLock<Regex> = LazyLock::new(|| regex(r"^\s+(?P<query>.*)$"));
/// A number or ')', a unit, to, and the unit being typed.
static TO_UNIT: LazyLock<Regex> = LazyLock::new(|| {
    regex(r"(?:\d|\))\s*(?P<from>[A-Za-z][\w/^]*)\s+to\s+(?P<query>(?:[A-Za-z][\w/^]*)?)$")
});
/// A number or ')', and the unit being typed after it.
static FROM_UNIT: LazyLock<Regex> =
    LazyLock::new(|| regex(r"(?:\d|\))\s*(?P<query>(?:[A-Za-z][\w/^]*)?)$"));
/// Letters where a value starts.
static FUNCTION_NAME: LazyLock<Regex> =
    LazyLock::new(|| regex(r"(?:^|[-+*/^%(,=])\s*(?P<query>[A-Za-z]\w*)$"));
static SINGLE_NUMBER: LazyLock<Regex> =
    LazyLock::new(|| regex(r"^\s*(?:\d+(?:\.\d*)?|\.\d+)\s+$"));
/// today, or a date as year-month-day.
static DATE: LazyLock<Regex> = LazyLock::new(|| regex(r"(?i)\btoday\b|\d{4}-\d{1,2}-\d{1,2}"));
/// A number, then a space or the letters of what it is: "30 ", "30 d", "30d".
static DURATION: LazyLock<Regex> =
    LazyLock::new(|| regex(r"\d(?:\s+(?P<q1>[A-Za-z]*)|(?P<q2>[A-Za-z]+))$"));

/// What to offer for `line` with the caret at `caret` (in characters); `None` for nothing.
/// `utc_now` gives each place's UTC offset, which changes with daylight saving time; `names`
/// are the names given values and the last answer, offered with their values.
pub fn suggest(line: &str, caret: usize, utc_now: DateTime<Utc>, names: &Names) -> Option<Suggestions> {
    if caret != line.chars().count() {
        return None;
    }

    // After a time, places or nothing: a word there is never a unit.
    match TIME.captures(line) {
        Some(time) => for_time(line, &time, utc_now),
        None => for_units(line).or_else(|| for_functions(line, names)),
    }
}

/// The suggestions replacing `typed`, which starts at byte `start` of `line`; `None` when there
/// are none.
fn result(line: &str, start: usize, typed: &str, items: Vec<Suggestion>) -> Option<Suggestions> {
    if items.is_empty() {
        return None;
    }
    Some(Suggestions { start: line[..start].chars().count(), length: typed.chars().count(), items })
}

fn result_at(line: &str, typed: Match<'_>, items: Vec<Suggestion>) -> Option<Suggestions> {
    result(line, typed.start(), typed.as_str(), items)
}

fn starts_with(name: &str, typed: &str) -> bool {
    name.len() >= typed.len()
        && name.is_char_boundary(typed.len())
        && name[..typed.len()].eq_ignore_ascii_case(typed)
}

fn for_functions(line: &str, names: &Names) -> Option<Suggestions> {
    let query = FUNCTION_NAME.captures(line)?.name("query")?;
    let typed = query.as_str();

    let functions = FUNCTION_HELP
        .iter()
        .filter(|(name, ..)| starts_with(name, typed))
        .map(|&(name, usage, meaning)| {
            // A function writes its name and '(', or both parentheses when it takes nothing.
            let insert = if usage.ends_with("()") { usage.to_string() } else { format!("{name}(") };
            (name.to_string(), Suggestion { text: format!("{usage}: {meaning}"), insert })
        });

    // A constant, or the last answer with its value once there's one.
    let values = VALUES.iter().filter(|(name, _)| starts_with(name, typed)).map(|&(name, meaning)| {
        let text = match find_name(names, name) {
            Some(value) => format!("{name}: {meaning}, {}", shown(value)),
            None => format!("{name}: {meaning}"),
        };
        (name.to_string(), Suggestion { text, insert: name.to_string() })
    });

    let given = names
        .iter()
        .filter(|(name, _)| !name.eq_ignore_ascii_case(LAST_ANSWER) && starts_with(name, typed))
        .map(|(name, value)| {
            let text = format!("{name}: {}", shown(value));
            (name.clone(), Suggestion { text, insert: name.clone() })
        });

    let mut items: Vec<(String, Suggestion)> = functions.chain(values).chain(given).collect();
    items.sort_by_key(|(name, _)| name.to_ascii_uppercase());
    result_at(line, query, items.into_iter().map(|(_, item)| item).collect())
}

/// A value as the calculator shows it, cut short when it's long, as a text can be.
fn shown(value: &Value) -> String {
    const LONGEST: usize = 40;
    let text = value.to_text(true);
    if text.chars().count() <= LONGEST {
        return text;
    }
    let cut: String = text.chars().take(LONGEST - 3).collect();
    format!("{cut}...")
}

fn for_time(line: &str, time: &Captures<'_>, utc_now: DateTime<Utc>) -> Option<Suggestions> {
    let time_end = time.name("time")?.end();
    let rest = &line[time_end..];

    if let Some(destination) = DESTINATION.captures(rest) {
        let query = destination.name("query")?;
        let items = places(query.as_str(), "", utc_now);
        return result(line, time_end + query.start(), query.as_str(), items);
    }

    let source = SOURCE.captures(rest)?;
    let typed = source.name("query")?;
    let mut items = Vec::new();

    // "10:30 p" may be the start of pm, more likely than of Paris.
    let has_half = time.name("ap1").or(time.name("ap2")).is_some();
    let hour = time.name("h1").or(time.name("h2"));
    if !has_half
        && let Some(hour) = hour
        && !typed.as_str().is_empty()
        && hour.as_str().parse::<u32>().is_ok_and(|hour| (1..=12).contains(&hour))
    {
        items.extend(["am", "pm"].iter().filter(|word| starts_with(word, typed.as_str())).map(|word| {
            Suggestion { text: word.to_string(), insert: format!("{word} ") }
        }));
    }

    items.extend(places(typed.as_str(), TO, utc_now));
    result(line, time_end + typed.start(), typed.as_str(), items)
}

fn for_units(line: &str) -> Option<Suggestions> {
    // With a date, a number is days, weeks, months or years to move it by.
    if DATE.is_match(line)
        && let Some(duration) = DURATION.captures(line)
    {
        let typed = duration.name("q1").or(duration.name("q2"))?;
        let items = DURATIONS
            .iter()
            .filter(|(symbol, name)| starts_with(symbol, typed.as_str()) || starts_with(name, typed.as_str()))
            .map(|(symbol, name)| Suggestion { text: format!("{symbol} ({name})"), insert: symbol.to_string() })
            .collect();
        return result_at(line, typed, items);
    }

    if let Some(to) = TO_UNIT.captures(line) {
        let from = find_unit(to.name("from")?.as_str());
        let query = to.name("query")?;
        return result_at(line, query, units(query.as_str(), from, ""));
    }

    let typed = FROM_UNIT.captures(line)?.name("query")?;
    // Nothing typed yet: every unit only after a single number.
    if typed.as_str().is_empty() && !SINGLE_NUMBER.is_match(line) {
        return None;
    }
    result_at(line, typed, units(typed.as_str(), None, TO))
}

/// The units whose symbol starts with what's typed, then those with another name that does; of
/// the kind of `from`, if there's one, but not `from` itself.
fn units(typed: &str, from: Option<&Unit>, after: &str) -> Vec<Suggestion> {
    let candidates: Vec<&Unit> = UNITS
        .iter()
        .filter(|unit| from.is_none_or(|from| unit.group == from.group && !std::ptr::eq(*unit, from)))
        .collect();

    let by_symbol = candidates
        .iter()
        .filter(|unit| starts_with(unit.symbol, typed))
        .map(|unit| unit_item(unit, unit.names.first().copied(), after));
    let by_name = candidates.iter().filter(|unit| !starts_with(unit.symbol, typed)).filter_map(|unit| {
        let name = unit.names.iter().find(|name| starts_with(name, typed))?;
        Some(unit_item(unit, Some(name), after))
    });
    by_symbol.chain(by_name).collect()
}

fn unit_item(unit: &Unit, name: Option<&str>, after: &str) -> Suggestion {
    let kind = unit.group.name();
    let text = match name {
        Some(name) => format!("{} ({name}, {kind})", unit.symbol),
        None => format!("{} ({kind})", unit.symbol),
    };
    Suggestion { text, insert: format!("{}{after}", unit.symbol) }
}

/// The places whose name, or another name, starts with what's typed: any case, `_` for a
/// space, without accents.
fn places(typed: &str, after: &str, utc_now: DateTime<Utc>) -> Vec<Suggestion> {
    let start = simplified(typed);
    let mut items = Vec::new();

    for place in PLACES {
        let other = if simplified(place.name).starts_with(&start) {
            None
        } else {
            match place.other_names.iter().find(|name| simplified(name).starts_with(&start)) {
                Some(name) => Some(*name),
                None => continue,
            }
        };
        let offset = format_offset(Zone::Named(place.zone()).offset_seconds(utc_now.naive_utc()));
        let text = match other {
            Some(other) => format!("{} ({other}, {offset})", place.name),
            None => format!("{} ({offset})", place.name),
        };
        items.push(Suggestion { text, insert: format!("{}{after}", place.name) });
    }

    if "local".starts_with(&start) {
        items.push(Suggestion {
            text: "local (this computer's time zone)".into(),
            insert: format!("local{after}"),
        });
    }
    if "utc".starts_with(&start) {
        items.push(Suggestion { text: "UTC (or UTC-5, UTC+5:30)".into(), insert: format!("UTC{after}") });
    }
    items
}

/// As the calculator reads a place: any case, `_` for a space, without accents (São is sao).
fn simplified(name: &str) -> String {
    name.replace('_', " ")
        .to_lowercase()
        .chars()
        .map(|c| match c {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => 'a',
            'è' | 'é' | 'ê' | 'ë' => 'e',
            'ì' | 'í' | 'î' | 'ï' => 'i',
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' => 'o',
            'ù' | 'ú' | 'û' | 'ü' => 'u',
            'ñ' => 'n',
            'ç' => 'c',
            'ý' | 'ÿ' => 'y',
            c => c,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use rust_decimal::Decimal;

    use super::*;
    use crate::calculator::engine::catalog::FUNCTIONS;

    /// In winter, so Mexico City is UTC-6 and São Paulo UTC-3 whatever day the tests run.
    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 15, 12, 0, 0).unwrap()
    }

    fn suggest_at_end(line: &str) -> Option<Suggestions> {
        suggest(line, line.chars().count(), now(), &Names::new())
    }

    fn texts(line: &str) -> Vec<String> {
        suggest_at_end(line).map_or_else(Vec::new, |s| s.items.into_iter().map(|item| item.text).collect())
    }

    #[test]
    fn after_a_time_every_place() {
        for line in ["10:30 ", "3 pm ", "9:00 am ", "now ", "10:00 to "] {
            let texts = texts(line);
            assert_eq!(texts[0], "Mexico City (UTC-6)", "{line}");
            assert!(texts.contains(&"local (this computer's time zone)".to_string()), "{line}");
            assert!(texts.contains(&"UTC (or UTC-5, UTC+5:30)".to_string()), "{line}");
        }
    }

    #[test]
    fn a_place_is_found_by_any_name_in_any_case_without_accents() {
        for (line, expected) in [
            ("10:30 sao", "São Paulo (UTC-3)"),
            ("10:30 SÃO", "São Paulo (UTC-3)"),
            ("10:30 cdmx", "Mexico City (cdmx, UTC-6)"),
            ("10:30 new_y", "New York (UTC-5)"),
            ("10:30 Mexico City to tok", "Tokyo (UTC+9)"),
        ] {
            assert_eq!(texts(line), [expected], "{line}");
        }
    }

    #[test]
    fn a_place_replaces_whats_typed_and_the_first_one_writes_to() {
        let from = suggest_at_end("10:30 mex").unwrap();
        let to = suggest_at_end("10:30 Mexico City to ind").unwrap();
        assert_eq!((from.start, from.length, from.items[0].insert.as_str()), (6, 3, "Mexico City to "));
        assert_eq!((to.start, to.length, to.items[0].insert.as_str()), (21, 3, "India"));
    }

    #[test]
    fn after_a_12_hour_time_p_is_first_pm() {
        let texts = texts("10:30 p");
        assert_eq!(texts[0], "pm");
        assert!(texts.contains(&"Paris (UTC+1)".to_string()));
        // A 24-hour time can't have it.
        assert!(!super::tests::texts("15:30 p").contains(&"pm".to_string()));
    }

    #[test]
    fn after_a_time_nothing_but_places() {
        for line in ["10:30 kg", "10:30 India ", "10:30"] {
            assert_eq!(suggest_at_end(line), None, "{line}");
        }
    }

    #[test]
    fn after_a_single_number_and_a_space_every_unit() {
        for line in ["10 ", " 2.5 "] {
            let suggestions = suggest_at_end(line).unwrap();
            assert_eq!(suggestions.items[0].text, "nm (nanometer, length)");
            assert_eq!(suggestions.items[0].insert, "nm to ");
            assert!(suggestions.items.iter().any(|item| item.text == "KiB (data)"));
        }
    }

    #[test]
    fn units_after_other_than_a_single_number_wait_for_a_letter() {
        for line in ["10", "1 + 2 ", "max(1, 5) ", "2+2"] {
            assert_eq!(suggest_at_end(line), None, "{line}");
        }
    }

    #[test]
    fn a_unit_is_found_by_its_symbol_first_then_by_its_other_names() {
        assert_eq!(
            texts("(1 + 2) kilo"),
            ["km (kilometer, length)", "kg (kilogram, mass)", "kB (kilobyte, data)"]
        );
        let by_letter = texts("1 + 2 K");
        assert_eq!(by_letter[0], "km (kilometer, length)");
        assert!(by_letter.contains(&"K (kelvin, temperature)".to_string()));
    }

    #[test]
    fn after_a_number_with_a_date_the_lengths_of_time_to_move_it_by() {
        for (line, expected) in [
            ("today + 30 ", &["d (days)", "wk (weeks)", "mo (months)", "yr (years)"][..]),
            ("today + 30 m", &["mo (months)"]),
            ("2026-12-25 - 3w", &["wk (weeks)"]),
            ("TODAY + 1 Y", &["yr (years)"]),
        ] {
            let suggestions = suggest_at_end(line).unwrap();
            let texts: Vec<&str> = suggestions.items.iter().map(|item| item.text.as_str()).collect();
            assert_eq!(texts, expected, "{line}");
            // The unit alone, not a conversion.
            assert!(suggestions.items.iter().all(|item| !item.insert.contains(TO)));
        }
    }

    #[test]
    fn after_to_units_of_the_same_kind_but_not_the_same_unit() {
        let suggestions = suggest_at_end("5 kg to ").unwrap();
        assert_eq!(suggestions.items[0].insert, "mg");
        assert!(suggestions.items.iter().all(|item| item.insert != "kg"));
        assert!(suggestions.items.iter().all(|item| item.text.ends_with("mass)")));
        assert_eq!(texts("5 zz to po"), ["lb (pound, mass)"]);
    }

    #[test]
    fn where_a_value_starts_the_functions_and_constants_whose_name_starts_with_whats_typed() {
        for (line, expected) in [
            ("s", &["sin(x): sine, in radians", "sqrt(x): square root"][..]),
            ("SQ", &["sqrt(x): square root"]),
            ("2 * (ro", &["round(x, decimals): rounds; decimals are optional"]),
            ("max(1, mi", &["min(a, ...): smallest"]),
            ("x = fl", &["floor(x): rounds down"]),
            ("p", &["pi: 3.14159..."]),
            ("tod", &["today: today's date"]),
            ("e", &["e: 2.71828...", "exp(x): e to the power x"]),
        ] {
            assert_eq!(texts(line), expected, "{line}");
        }
    }

    #[test]
    fn a_function_writes_its_parenthesis() {
        for (line, start, length, insert) in
            [("sq", 0, 2, "sqrt("), ("1 + ipc", 4, 3, "ipconfig()"), ("an", 0, 2, "ans")]
        {
            let suggestions = suggest_at_end(line).unwrap();
            assert_eq!(
                (suggestions.start, suggestions.length, suggestions.items[0].insert.as_str()),
                (start, length, insert),
                "{line}"
            );
        }
    }

    #[test]
    fn without_letters_where_a_value_starts_no_functions() {
        for line in ["", "1 + ", "sqrt(", "len(\"s", "zz"] {
            assert_eq!(suggest_at_end(line), None, "{line}");
        }
    }

    #[test]
    fn after_a_number_units_come_before_functions() {
        // 2 s is two seconds, never a function.
        assert!(texts("2 s").iter().all(|text| !text.contains("(x)")));
    }

    #[test]
    fn the_names_given_values_are_offered_with_their_values_cut_short_when_long() {
        let names = Names::from([
            ("Total".to_string(), Value::Number(Decimal::new(12345, 1))),
            ("tip".to_string(), Value::Text("a".repeat(50))),
            (LAST_ANSWER.to_string(), Value::Number(Decimal::from(7))),
        ]);
        let texts: Vec<String> =
            suggest("2 + t", 5, now(), &names).unwrap().items.into_iter().map(|item| item.text).collect();
        let answer = suggest("an", 2, now(), &names).unwrap().items;

        assert_eq!(
            texts,
            [
                "tan(x): tangent, in radians".to_string(),
                format!("tip: {}...", "a".repeat(37)),
                "today: today's date".to_string(),
                "Total: 1,234.5".to_string(),
            ]
        );
        assert_eq!(answer.len(), 1);
        assert_eq!(answer[0].text, "ans: the last answer, 7");
    }

    #[test]
    fn every_function_of_the_engine_is_offered() {
        let mut engine: Vec<&str> = FUNCTIONS.iter().map(|f| f.name).chain([IPCONFIG]).collect();
        let mut helped: Vec<&str> = FUNCTION_HELP.iter().map(|(name, ..)| *name).collect();
        engine.sort_unstable();
        helped.sort_unstable();
        assert_eq!(engine, helped);
    }

    #[test]
    fn with_the_caret_before_the_end_nothing() {
        assert_eq!(suggest("10 km", 2, now(), &Names::new()), None);
    }
}
