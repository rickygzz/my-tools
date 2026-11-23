//! `zones.cfg`: the zones as text, which the user can write and the zone editor saves.
//!
//! ```text
//! # The default layout, for every monitor without its own
//! Layout = Columns 3          (or Columns 25 50 25, Rows 2, Grid 3 x 2)
//! Gap = 8
//!
//! # A monitor's own layout: the lines after a Monitor line (left, top, width, height)
//! Monitor = 1920, 0, 2560, 1440
//! Zone = 0, 0, 60, 100        (left, top, width, height, in percent of the work area)
//! Zone = 60, 0, 40, 100
//! ```
//!
//! Every line that can't be used is a problem, with its line number, and is skipped. Without a
//! layout that can be used, three columns are. A monitor's layout takes what it lacks (its
//! zones or its gap) from the default one.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use super::layout::{DEFAULT_GAP, Layout, Layouts, Rect, Zone, columns, equal, grid, rows};

const LAYOUT: &str = "Layout";
const ZONE: &str = "Zone";
const GAP: &str = "Gap";
const MONITOR: &str = "Monitor";

/// The most zones a layout has.
pub const MAX_ZONES: usize = 99;
pub const MAX_GAP: i32 = 100;

/// How far from 100 the widths or heights can add up to, for thirds such as 33.33.
const PERCENT_TOLERANCE: f64 = 0.5;

const LAYOUT_EXAMPLE: &str = "e.g. Layout = Columns 3, Columns 25 50 25, Rows 2 or Grid 3 x 2";
const ZONE_EXAMPLE: &str = "e.g. Zone = 0, 0, 50, 100";
const MONITOR_EXAMPLE: &str = "e.g. Monitor = 1920, 0, 2560, 1440";

/// What `zones.cfg` asks for: the layouts, and a problem for each line that was skipped.
#[derive(Debug, Default)]
pub struct ConfigResult {
    pub layouts: Layouts,
    /// Each problem's line, from 1, and what's wrong with it.
    pub problems: Vec<(usize, String)>,
}

impl ConfigResult {
    fn problem(&mut self, line: usize, message: impl Into<String>) {
        self.problems.push((line, message.into()));
    }

    /// The problems for the user, one per line: "Line 2: Gap: ...".
    pub fn problems_text(&self) -> String {
        let lines: Vec<String> = self.problems.iter().map(|(line, message)| format!("Line {line}: {message}")).collect();
        lines.join("\n")
    }
}

/// The lines of one layout: the default one, or a monitor's own.
#[derive(Default)]
struct Section {
    /// The monitor it's for; `None` for the default layout, or one that isn't used.
    bounds: Option<Rect>,
    /// The Monitor line it starts at; 0 for the default layout.
    line: usize,
    /// From a Layout line, and that line.
    layout: Option<(Vec<Zone>, usize)>,
    zones: Vec<Zone>,
    gap: Option<i32>,
}

impl Section {
    /// Its zones, from Zone lines or a Layout line, and its gap; what it lacks is `fallback`'s.
    fn build(self, result: &mut ConfigResult, fallback: &Layout) -> Layout {
        if let Some((_, line)) = &self.layout
            && !self.zones.is_empty()
        {
            result.problem(*line, format!("{LAYOUT}: there are {ZONE} lines too, and they're used instead."));
        }
        let zones = if !self.zones.is_empty() {
            self.zones
        } else {
            self.layout.map_or_else(|| fallback.zones.clone(), |(zones, _)| zones)
        };
        Layout { zones, gap: self.gap.unwrap_or(fallback.gap) }
    }
}

/// Reads the text of `zones.cfg`.
pub fn read(text: &str) -> ConfigResult {
    let mut result = ConfigResult::default();
    // The lines before the first Monitor line are the default layout's; each Monitor line
    // starts a monitor's own. Lines of a Monitor line that can't be used go to `unused`.
    let mut default = Section::default();
    let mut monitors: Vec<Section> = Vec::new();
    let mut unused = Section::default();
    let mut current = Current::Default;

    for (index, line) in text.lines().enumerate() {
        let number = index + 1;
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((name, value)) = line.split_once('=').map(|(name, value)| (name.trim(), value)) else {
            result.problem(number, format!("'{line}' is not 'Name = Value'."));
            continue;
        };
        if name.is_empty() || name.contains(char::is_whitespace) {
            result.problem(number, format!("'{line}' is not 'Name = Value'."));
            continue;
        }
        // A # after the value starts a comment.
        let value = value.split('#').next().unwrap_or_default().trim();

        if name.eq_ignore_ascii_case(MONITOR) {
            current = start_monitor(&mut result, &mut monitors, value, number);
            unused = Section::default();
            continue;
        }
        let section = match current {
            Current::Default => &mut default,
            Current::Monitor(index) => &mut monitors[index],
            Current::Unused => &mut unused,
        };
        if name.eq_ignore_ascii_case(LAYOUT) {
            read_layout_line(&mut result, section, value, number);
        } else if name.eq_ignore_ascii_case(ZONE) {
            read_zone_line(&mut result, section, value, number);
        } else if name.eq_ignore_ascii_case(GAP) {
            read_gap_line(&mut result, section, value, number);
        } else {
            result.problem(number, format!("{name}: each line is {LAYOUT} = ..., {ZONE} = ..., {GAP} = ... or {MONITOR} = ...."));
        }
    }

    let default = default.build(&mut result, &Layout::default());
    let monitors = monitors
        .into_iter()
        .filter_map(|section| {
            let bounds = section.bounds?;
            Some((bounds, section.build(&mut result, &default)))
        })
        .collect();
    result.layouts = Layouts { default, monitors };
    result.problems.sort_by_key(|(line, _)| *line);
    result
}

/// Which section the lines go to now.
#[derive(Clone, Copy)]
enum Current {
    Default,
    Monitor(usize),
    /// After a Monitor line that can't be used: read for their problems, but not used.
    Unused,
}

fn start_monitor(result: &mut ConfigResult, monitors: &mut Vec<Section>, value: &str, line: usize) -> Current {
    let Some(bounds) = read_monitor(value) else {
        result.problem(
            line,
            format!(
                "{MONITOR}: four whole numbers in pixels: left, top, width, height, as the system places the \
                 monitor; {MONITOR_EXAMPLE}. Its lines are not used."
            ),
        );
        return Current::Unused;
    };
    if let Some(other) = monitors.iter().find(|monitor| monitor.bounds == Some(bounds)) {
        result.problem(line, format!("{MONITOR}: there's one already for it, line {}; these lines are not used.", other.line));
        return Current::Unused;
    }
    monitors.push(Section { bounds: Some(bounds), line, ..Section::default() });
    Current::Monitor(monitors.len() - 1)
}

fn read_layout_line(result: &mut ConfigResult, section: &mut Section, value: &str, line: usize) {
    if let Some((_, first)) = &section.layout {
        result.problem(line, format!("{LAYOUT}: there's one already, line {first}, which is used."));
        return;
    }
    match read_layout(value) {
        Ok(zones) => section.layout = Some((zones, line)),
        Err(problem) => result.problem(line, format!("{LAYOUT}: {problem}")),
    }
}

fn read_zone_line(result: &mut ConfigResult, section: &mut Section, value: &str, line: usize) {
    if section.zones.len() == MAX_ZONES {
        result.problem(line, format!("{ZONE}: there can be {MAX_ZONES} zones at most."));
        return;
    }
    match read_zone(value) {
        Ok(zone) => section.zones.push(zone),
        Err(problem) => result.problem(line, format!("{ZONE}: {problem}")),
    }
}

fn read_gap_line(result: &mut ConfigResult, section: &mut Section, value: &str, line: usize) {
    if section.gap.is_some() {
        result.problem(line, format!("{GAP}: there's one already, which is used."));
        return;
    }
    match whole_number(value).filter(|&pixels| pixels <= MAX_GAP as u32) {
        Some(pixels) => section.gap = Some(pixels as i32),
        None => result.problem(line, format!("{GAP}: a whole number of pixels from 0 to {MAX_GAP}, not '{value}'.")),
    }
}

/// Numbers split by commas or spaces.
fn parts(value: &str) -> Vec<&str> {
    value.split([',', ' ', '\t']).filter(|part| !part.is_empty()).collect()
}

/// "left, top, width, height", whole pixels; left and top can be below 0, for a monitor left
/// of or above the main one.
fn read_monitor(value: &str) -> Option<Rect> {
    let numbers: Vec<i32> = parts(value).into_iter().map(signed_number).collect::<Option<_>>()?;
    match numbers[..] {
        [left, top, width, height] if width > 0 && height > 0 => Some(Rect::new(left, top, width, height)),
        _ => None,
    }
}

/// "Columns 3", "Columns 25 50 25", "Rows 2", "Grid 3 x 2".
fn read_layout(value: &str) -> Result<Vec<Zone>, String> {
    let spaced = value.replace(['x', 'X'], " x ");
    let words: Vec<&str> = spaced.split_whitespace().collect();
    let (kind, numbers) = words.split_first().map_or(("", &[][..]), |(kind, numbers)| (*kind, numbers));

    let is_columns = kind.eq_ignore_ascii_case("Columns");
    if is_columns || kind.eq_ignore_ascii_case("Rows") {
        let sizes = read_sizes(numbers, if is_columns { "columns" } else { "rows" })?;
        return Ok(if is_columns { columns(&sizes) } else { rows(&sizes) });
    }

    if kind.eq_ignore_ascii_case("Grid") {
        let counts: Vec<&str> = numbers.iter().copied().filter(|word| !word.eq_ignore_ascii_case("x")).collect();
        if let [columns, rows] = counts[..]
            && let (Some(columns), Some(rows)) = (count(columns), count(rows))
            && columns * rows <= MAX_ZONES
        {
            return Ok(grid(&equal(columns), &equal(rows)));
        }
        return Err(format!("Grid takes columns x rows, e.g. Grid 3 x 2, with {MAX_ZONES} zones at most."));
    }

    Err(if value.is_empty() {
        format!("which layout? {LAYOUT_EXAMPLE}.")
    } else {
        format!("'{value}' is not Columns, Rows or Grid; {LAYOUT_EXAMPLE}.")
    })
}

/// One number is a count; more are the sizes in percent, adding up to 100.
fn read_sizes(numbers: &[&str], what: &str) -> Result<Vec<f64>, String> {
    if let [one] = numbers
        && let Some(count) = count(one)
    {
        return Ok(equal(count));
    }

    let percents: Option<Vec<f64>> = numbers.iter().map(|number| percent(number).filter(|&percent| percent > 0.0)).collect();
    let Some(percents) = percents.filter(|percents| (2..=MAX_ZONES).contains(&percents.len())) else {
        let capital = capitalized(what);
        return Err(format!(
            "how many {what} from 1 to {MAX_ZONES}, or the size of each in percent, e.g. {capital} 3 or {capital} 25 50 25."
        ));
    };

    let total: f64 = percents.iter().sum();
    if (total - 100.0).abs() > PERCENT_TOLERANCE {
        return Err(format!("the {what} add up to {} percent, not 100.", format_percent(total)));
    }
    Ok(percents)
}

/// "left, top, width, height" in percent.
fn read_zone(value: &str) -> Result<Zone, String> {
    let numbers: Option<Vec<f64>> = parts(value).into_iter().map(percent).collect();
    let Some([left, top, width, height]) = numbers.and_then(|numbers| <[f64; 4]>::try_from(numbers).ok()) else {
        return Err(format!("four numbers in percent of the screen: left, top, width, height; {ZONE_EXAMPLE}."));
    };
    let zone = Zone::new(left, top, width, height);
    if width <= 0.0 || height <= 0.0 {
        return Err("its width and height must be more than 0.".into());
    }
    if zone.right() > 100.0 + PERCENT_TOLERANCE {
        return Err(format!("it goes past the right edge: left + width is {}, more than 100.", format_percent(zone.right())));
    }
    if zone.bottom() > 100.0 + PERCENT_TOLERANCE {
        return Err(format!("it goes past the bottom: top + height is {}, more than 100.", format_percent(zone.bottom())));
    }
    Ok(zone)
}

/// Digits only: no sign, no spaces.
fn whole_number(text: &str) -> Option<u32> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// Digits with an optional sign.
fn signed_number(text: &str) -> Option<i32> {
    let digits = text.strip_prefix(['-', '+']).unwrap_or(text);
    whole_number(digits)?;
    text.parse().ok()
}

/// A count of zones, columns or rows: 1 to [`MAX_ZONES`].
fn count(text: &str) -> Option<usize> {
    whole_number(text).map(|count| count as usize).filter(|count| (1..=MAX_ZONES).contains(count))
}

/// A percent from 0 to 100: digits and a decimal point, no sign or exponent.
fn percent(text: &str) -> Option<f64> {
    let valid = text.bytes().all(|byte| byte.is_ascii_digit() || byte == b'.') && text.bytes().any(|byte| byte.is_ascii_digit());
    text.parse().ok().filter(|&percent| valid && percent <= 100.0)
}

/// Rounded to hundredths, without a needless ".0": 110, 33.33.
fn format_percent(number: f64) -> String {
    format!("{}", (number * 100.0).round() / 100.0)
}

fn capitalized(word: &str) -> String {
    let mut chars = word.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// The content of a new `zones.cfg`: how to use it, and three columns.
pub fn default_text() -> String {
    format!("{HELP}\n{LAYOUT} = Columns 3\n{GAP} = {DEFAULT_GAP}\n")
}

/// The content of `zones.cfg` for the layouts, as the editor saves them: how to use the file,
/// then the default layout, then each monitor's own after its Monitor line. A Zone line per
/// zone, in percent rounded to hundredths: each zone's right and bottom are rounded, not its
/// size, so zones side by side still meet.
#[cfg_attr(not(test), expect(dead_code, reason = "the zone editor saves with it (step 4)"))]
pub fn text_of(layouts: &Layouts) -> String {
    let mut text = format!("{HELP}# Saved by the window zones editor.\n\n");
    write_layout(&mut text, &layouts.default);
    for (bounds, layout) in &layouts.monitors {
        let Rect { left, top, width, height } = bounds;
        let _ = writeln!(text, "\n{MONITOR} = {left}, {top}, {width}, {height}");
        write_layout(&mut text, layout);
    }
    text
}

#[cfg_attr(not(test), expect(dead_code, reason = "the zone editor saves with it (step 4)"))]
fn write_layout(text: &mut String, layout: &Layout) {
    let round = |number: f64| (number * 100.0).round() / 100.0;
    for zone in &layout.zones {
        let (left, top) = (round(zone.left), round(zone.top));
        let width = round(round(zone.right()) - left);
        let height = round(round(zone.bottom()) - top);
        let _ = writeln!(text, "{ZONE} = {left}, {top}, {width}, {height}");
    }
    let _ = writeln!(text, "{GAP} = {}", layout.gap);
}

const HELP: &str = "\
# my-tools window zones: hold Shift while you drag a window, and drop it on a zone.
# Zones are numbered from 1. Changes apply at the next drag.
# The default layout, for every monitor without its own:
#   Layout = Columns 3           3 columns as wide as each other
#   Layout = Columns 25 50 25    columns this wide, in percent of the screen
#   Layout = Rows 2              or Rows 30 70
#   Layout = Grid 3 x 2          3 columns, 2 rows
# Or your own zones instead, one line each, in percent: left, top, width, height
#   Zone = 0, 0, 60, 100
#   Zone = 60, 0, 40, 50
#   Zone = 60, 50, 40, 50
# Gap: pixels between zones and around them, 0 for none.
# A monitor's own layout: the same lines after a Monitor line, with where the system has the
# monitor, in pixels: left, top, width, height. Without a Gap, it takes the default one's.
#   Monitor = 1920, 0, 2560, 1440
#   Layout = Rows 2
";

/// Where `zones.cfg` is: in the user's config folder (`%APPDATA%\my-tools` on Windows,
/// `~/Library/Application Support/my-tools` on macOS, `~/.config/my-tools` on Linux).
pub fn file_path() -> Option<PathBuf> {
    dirs::config_dir().map(|folder| folder.join("my-tools").join("zones.cfg"))
}

/// Reads the file, creating it with three columns when it isn't there. When it can't be read
/// or written, the default layout is used and the reason is a problem (at line 0).
pub fn load(path: &Path) -> ConfigResult {
    let text = (|| {
        if !path.exists() {
            if let Some(folder) = path.parent() {
                std::fs::create_dir_all(folder)?;
            }
            std::fs::write(path, default_text())?;
        }
        std::fs::read_to_string(path)
    })();
    match text {
        Ok(text) => read(&text),
        Err(error) => {
            let mut result = ConfigResult::default();
            result.problem(0, format!("Could not use {}, so the default zones are used: {error}", path.display()));
            result
        }
    }
}

/// Writes the layouts, for the next drag. It writes a file next to it first, then renames it
/// over the old one, so a crash halfway doesn't leave half a file. `Err` says why it couldn't.
#[cfg_attr(not(test), expect(dead_code, reason = "the zone editor saves with it (step 4)"))]
pub fn save(path: &Path, layouts: &Layouts) -> Result<(), String> {
    let write = || {
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder)?;
        }
        let new = path.with_extension("cfg.new");
        std::fs::write(&new, text_of(layouts))?;
        std::fs::rename(&new, path)
    };
    write().map_err(|error| format!("Could not save {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The zones as "left top width height|...", to compare at a glance.
    fn describe(zones: &[Zone]) -> String {
        let zones: Vec<String> = zones
            .iter()
            .map(|zone| [zone.left, zone.top, zone.width, zone.height].map(format_percent).join(" "))
            .collect();
        zones.join("|")
    }

    #[test]
    fn the_new_file_is_three_columns_with_the_default_gap() {
        let result = read(&default_text());
        assert!(result.problems.is_empty(), "{:?}", result.problems);
        assert_eq!(result.layouts.default, Layout::default());
    }

    #[test]
    fn nothing_but_comments_is_the_default() {
        let result = read("# nothing\n\n");
        assert!(result.problems.is_empty());
        assert_eq!(result.layouts.default, Layout::default());
    }

    #[test]
    fn the_lines_give_the_zones() {
        for (text, expected) in [
            ("Layout = Columns 2", "0 0 50 100|50 0 50 100"),
            ("layout = columns 25 50 25 # wide center", "0 0 25 100|25 0 50 100|75 0 25 100"),
            ("Layout = Rows 30 70", "0 0 100 30|0 30 100 70"),
            ("Layout = Grid 2 x 2", "0 0 50 50|50 0 50 50|0 50 50 50|50 50 50 50"),
            ("Layout = Grid 2x1", "0 0 50 100|50 0 50 100"),
            ("Zone = 0, 0, 60, 100\nZone = 60, 0, 40, 50\nZone = 60 50 40 50", "0 0 60 100|60 0 40 50|60 50 40 50"),
        ] {
            let result = read(text);
            assert!(result.problems.is_empty(), "{text}: {:?}", result.problems);
            assert_eq!(describe(&result.layouts.default.zones), expected, "{text}");
        }
    }

    #[test]
    fn thirds_in_percent_add_up_to_close_enough_to_100() {
        let result = read("Layout = Columns 33.33 33.33 33.33");
        assert!(result.problems.is_empty());
        assert_eq!(result.layouts.default.zones.len(), 3);
    }

    #[test]
    fn a_gap_is_used() {
        assert_eq!(read("Layout = Columns 2\nGap = 0").layouts.default.gap, 0);
    }

    #[test]
    fn a_line_that_cant_be_used_is_a_problem_with_its_line_and_the_default_is_used() {
        const EXAMPLE: &str = "e.g. Layout = Columns 3, Columns 25 50 25, Rows 2 or Grid 3 x 2.";
        const GRID: &str = "Layout: Grid takes columns x rows, e.g. Grid 3 x 2, with 99 zones at most.";
        let rows_help = "Layout: how many rows from 1 to 99, or the size of each in percent, e.g. Rows 3 or Rows 25 50 25.";
        for (line, message) in [
            ("Layout = Diagonal 3", format!("Layout: 'Diagonal 3' is not Columns, Rows or Grid; {EXAMPLE}")),
            ("Layout =", format!("Layout: which layout? {EXAMPLE}")),
            ("Layout = Columns 30 60", "Layout: the columns add up to 90 percent, not 100.".into()),
            ("Layout = Rows 0", rows_help.into()),
            ("Layout = Grid 3", GRID.into()),
            ("Layout = Grid 10 x 10", GRID.into()),
            ("Zone = 0, 0, 50", "Zone: four numbers in percent of the screen: left, top, width, height; e.g. Zone = 0, 0, 50, 100.".into()),
            ("Zone = 60, 0, 50, 100", "Zone: it goes past the right edge: left + width is 110, more than 100.".into()),
            ("Zone = 0, 60, 50, 50", "Zone: it goes past the bottom: top + height is 110, more than 100.".into()),
            ("Zone = 0, 0, 0, 100", "Zone: its width and height must be more than 0.".into()),
            ("Gap = -1", "Gap: a whole number of pixels from 0 to 100, not '-1'.".into()),
            ("Gap = 101", "Gap: a whole number of pixels from 0 to 100, not '101'.".into()),
            ("Columns = 3", "Columns: each line is Layout = ..., Zone = ..., Gap = ... or Monitor = ....".into()),
            ("wide", "'wide' is not 'Name = Value'.".into()),
        ] {
            let result = read(&format!("# zones\n{line}"));
            assert_eq!(result.problems, [(2, message.clone())], "{line}");
            assert_eq!(result.problems_text(), format!("Line 2: {message}"));
            assert_eq!(result.layouts.default, Layout::default(), "{line}");
        }
    }

    #[test]
    fn of_two_layouts_the_first_is_used() {
        let result = read("Layout = Columns 2\nLayout = Rows 2");
        assert_eq!(result.problems_text(), "Line 2: Layout: there's one already, line 1, which is used.");
        assert_eq!(describe(&result.layouts.default.zones), "0 0 50 100|50 0 50 100");
    }

    #[test]
    fn of_a_layout_and_zones_the_zones_are_used() {
        let result = read("Layout = Columns 2\nZone = 0, 0, 100, 100");
        assert_eq!(result.problems_text(), "Line 1: Layout: there are Zone lines too, and they're used instead.");
        assert_eq!(describe(&result.layouts.default.zones), "0 0 100 100");
    }

    #[test]
    fn after_a_monitor_line_is_that_monitors_own_layout_with_the_default_gap_when_it_has_none() {
        let result = read(
            "Layout = Columns 2\nGap = 12\n\nMonitor = 1920, 0, 2560, 1440   # the wide one\nLayout = Rows 2\n\
             Monitor = -1080, -200, 1080, 1920\nZone = 0, 0, 100, 100\nGap = 0",
        );
        assert!(result.problems.is_empty(), "{:?}", result.problems);
        assert_eq!(describe(&result.layouts.default.zones), "0 0 50 100|50 0 50 100");
        let [(wide, wide_layout), (tall, tall_layout)] = &result.layouts.monitors[..] else {
            panic!("expected two monitors: {:?}", result.layouts.monitors);
        };
        assert_eq!(*wide, Rect::new(1920, 0, 2560, 1440));
        assert_eq!(describe(&wide_layout.zones), "0 0 100 50|0 50 100 50");
        assert_eq!(wide_layout.gap, 12);
        assert_eq!(*tall, Rect::new(-1080, -200, 1080, 1920));
        assert_eq!(tall_layout.gap, 0);
    }

    #[test]
    fn a_monitor_with_only_a_gap_has_the_default_zones() {
        let result = read("Layout = Rows 2\nMonitor = 0, 0, 1920, 1080\nGap = 0");
        assert_eq!(result.layouts.monitors.len(), 1);
        assert_eq!(result.layouts.monitors[0].1.zones, result.layouts.default.zones);
    }

    #[test]
    fn a_monitor_line_that_cant_be_used_is_a_problem_and_its_lines_are_not_used() {
        for monitor in ["Monitor = 1920, 0, 2560", "Monitor = 1920, 0, 0, 1440", "Monitor = wide"] {
            let result = read(&format!("Layout = Columns 2\n{monitor}\nLayout = Rows 2"));
            assert_eq!(result.problems.len(), 1, "{monitor}: {:?}", result.problems);
            assert!(result.problems_text().starts_with("Line 2: Monitor: four whole numbers in pixels"));
            assert!(result.layouts.monitors.is_empty());
            assert_eq!(describe(&result.layouts.default.zones), "0 0 50 100|50 0 50 100");
        }
    }

    #[test]
    fn of_the_same_monitor_twice_the_first_is_used() {
        let result = read("Monitor = 0, 0, 1920, 1080\nLayout = Rows 2\nMonitor = 0, 0, 1920, 1080\nLayout = Columns 4");
        assert_eq!(result.problems_text(), "Line 3: Monitor: there's one already for it, line 1; these lines are not used.");
        assert_eq!(result.layouts.monitors.len(), 1);
        assert_eq!(describe(&result.layouts.monitors[0].1.zones), "0 0 100 50|0 50 100 50");
    }

    #[test]
    fn what_the_editor_saves_reads_back_the_same() {
        let layouts = Layouts {
            default: Layout { zones: columns(&equal(3)), gap: 4 },
            monitors: vec![(Rect::new(-1080, -420, 1080, 1920), Layout { zones: rows(&[30.0, 70.0]), gap: 0 })],
        };
        let result = read(&text_of(&layouts));
        assert!(result.problems.is_empty(), "{:?}", result.problems);
        // The middle one is wider by a hundredth: each right edge is rounded, so they still meet.
        assert_eq!(describe(&result.layouts.default.zones), "0 0 33.33 100|33.33 0 33.34 100|66.67 0 33.33 100");
        assert_eq!(result.layouts.default.gap, 4);
        assert_eq!(result.layouts.monitors[0].0, layouts.monitors[0].0);
        assert_eq!(result.layouts.monitors[0].1, layouts.monitors[0].1);
    }

    #[test]
    fn a_missing_file_is_made_with_three_columns_and_saving_replaces_it() {
        let folder = std::env::temp_dir().join(format!("my-tools-zones-test-{}", std::process::id()));
        let path = folder.join("zones.cfg");
        let _ = std::fs::remove_dir_all(&folder);

        let result = load(&path);
        assert!(result.problems.is_empty());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), default_text());

        let layouts = Layouts { default: Layout { zones: rows(&equal(2)), gap: 0 }, monitors: vec![] };
        save(&path, &layouts).unwrap();
        assert_eq!(load(&path).layouts, layouts);

        let _ = std::fs::remove_dir_all(&folder);
    }
}
