//! What the calculator window does, without the window: calculates the line typed, keeps the
//! history, the names given values and the lines entered, and works out the suggestions.
//!
//! The window (`view.rs`) draws from this state each frame and calls these methods when keys
//! are pressed, so all the behaviour can be tested without a screen.

use chrono::{DateTime, Utc};

use super::engine::{Calculator, LAST_ANSWER, Names};
use super::suggester::{self, Suggestions};

/// The oldest operations are dropped after this many.
pub const MAX_HISTORY: usize = 200;
/// How many lines Up and Down go back through.
pub const MAX_RECALLED: usize = 200;
/// What the history shows before the first operation.
pub const NO_HISTORY: &str = "No operations yet";

/// An operation that worked, as the history shows it: `operation = result`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    pub operation: String,
    pub result: String,
}

/// What the line under the text box shows.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Output {
    #[default]
    Nothing,
    Result(String),
    /// Why the line couldn't be calculated.
    Error(String),
}

/// Part of the line to select, in characters: the character at fault after an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub start: usize,
    pub length: usize,
}

pub struct Presenter {
    calculator: Calculator,
    /// The time now, for the UTC offsets the suggestions show.
    utc_now: fn() -> DateTime<Utc>,

    /// The line being typed; the text box edits it directly.
    pub expression: String,
    output: Output,
    history: Vec<HistoryEntry>,
    history_shown: bool,

    /// The names given values, and the last answer, until the app exits: x = 5, ans * 2.
    names: Names,

    /// The lines entered, newest last, those that failed too so they can be fixed.
    entered: Vec<String>,
    /// Which of them the text box shows, if Up brought one back.
    recalled: Option<usize>,
    /// What was being typed before Up, which Down past the newest brings back.
    draft: String,

    suggestions: Option<Suggestions>,
    /// The suggestion chosen with Up and Down, if any.
    pub chosen: Option<usize>,
}

impl Presenter {
    pub fn new(calculator: Calculator, utc_now: fn() -> DateTime<Utc>) -> Self {
        Self {
            calculator,
            utc_now,
            expression: String::new(),
            output: Output::Nothing,
            history: Vec::new(),
            history_shown: false,
            names: Names::new(),
            entered: Vec::new(),
            recalled: None,
            draft: String::new(),
            suggestions: None,
            chosen: None,
        }
    }

    pub fn output(&self) -> &Output {
        &self.output
    }

    pub fn history(&self) -> &[HistoryEntry] {
        &self.history
    }

    pub fn history_shown(&self) -> bool {
        self.history_shown
    }

    pub fn suggestions(&self) -> Option<&Suggestions> {
        self.suggestions.as_ref()
    }

    /// The window was just shown.
    pub fn shown(&mut self) {
        self.hide_suggestions();
    }

    /// Calculates the line. An empty line is ignored. After an error, gives the part of the line
    /// to select: the character at fault, or the end when it's past the end.
    pub fn evaluate(&mut self) -> Option<Selection> {
        self.hide_suggestions();

        let line = self.expression.trim().to_string();
        if line.is_empty() {
            return None;
        }
        self.remember(&line);

        match self.calculator.calculate(&self.expression, &self.names, true) {
            Ok(calculation) => {
                // A time in another place has no value, so the last answer stays.
                if let Some(value) = calculation.value {
                    if let Some(name) = &calculation.assigned_name {
                        self.set_name(name, value.clone());
                    }
                    self.set_name(LAST_ANSWER, value);
                }

                if self.history.len() == MAX_HISTORY {
                    self.history.remove(0);
                }
                self.history.push(HistoryEntry { operation: line, result: calculation.text.clone() });
                self.output = Output::Result(calculation.text);
                None
            }
            Err(error) => {
                let length = self.expression.chars().count();
                self.output = Output::Error(error.to_string());
                Some(Selection {
                    start: error.position.min(length),
                    length: usize::from(error.position < length),
                })
            }
        }
    }

    /// Keeps a value under a name, replacing one given in another letter case.
    fn set_name(&mut self, name: &str, value: super::engine::Value) {
        self.names.retain(|key, _| !key.eq_ignore_ascii_case(name));
        self.names.insert(name.to_string(), value);
    }

    /// The result showing, for Ctrl+C; `None` while an error shows, or before the first result.
    pub fn result_to_copy(&self) -> Option<&str> {
        match &self.output {
            Output::Result(result) => Some(result),
            _ => None,
        }
    }

    pub fn toggle_history(&mut self) {
        self.history_shown = !self.history_shown;
    }

    /// A line entered, unless it's the one entered last, so Up doesn't show it twice.
    fn remember(&mut self, line: &str) {
        if self.entered.last().is_none_or(|last| last != line) {
            self.entered.push(line.to_string());
            if self.entered.len() > MAX_RECALLED {
                self.entered.remove(0);
            }
        }
        self.recalled = None;
    }

    /// Up (`back`) brings back the line entered before the one shown, newest first; Down the one
    /// after it, and past the newest, what was being typed. Gives where to put the caret when
    /// the line changed.
    pub fn recall(&mut self, back: bool) -> Option<usize> {
        if self.entered.is_empty() {
            return None;
        }

        let line = if back {
            let index = match self.recalled {
                Some(0) => return None,
                Some(index) => index - 1,
                None => {
                    self.draft = self.expression.clone();
                    self.entered.len() - 1
                }
            };
            self.recalled = Some(index);
            self.entered[index].clone()
        } else {
            let index = self.recalled? + 1;
            if index < self.entered.len() {
                self.recalled = Some(index);
                self.entered[index].clone()
            } else {
                self.recalled = None;
                self.draft.clone()
            }
        };

        // A line brought back is whole, so nothing is offered for it.
        self.hide_suggestions();
        self.expression = line;
        Some(self.expression.chars().count())
    }

    /// The line was typed in (not set by `recall` or `choose`): it starts over from here, and
    /// what can come next is worked out again. `caret` is where the caret is, in characters.
    pub fn expression_changed(&mut self, caret: usize) {
        self.recalled = None;
        self.update_suggestions(caret);
    }

    fn update_suggestions(&mut self, caret: usize) {
        self.suggestions = suggester::suggest(&self.expression, caret, (self.utc_now)(), &self.names);
        self.chosen = None;
    }

    pub fn hide_suggestions(&mut self) {
        self.suggestions = None;
        self.chosen = None;
    }

    /// Moves the choice in the list down (or up, `back`), stopping at the ends; up past the
    /// first chooses none.
    pub fn move_choice(&mut self, back: bool) {
        let Some(suggestions) = &self.suggestions else { return };
        let last = suggestions.items.len() - 1;
        self.chosen = match (self.chosen, back) {
            (None, false) => Some(0),
            (Some(index), false) => Some((index + 1).min(last)),
            (Some(0) | None, true) => None,
            (Some(index), true) => Some(index - 1),
        };
    }

    /// Writes suggestion `index` over what was typed of it, and offers what can follow it.
    /// Gives where to put the caret.
    pub fn choose(&mut self, index: usize) -> Option<usize> {
        let suggestions = self.suggestions.take()?;
        let Some(item) = suggestions.items.get(index) else {
            self.suggestions = Some(suggestions);
            return None;
        };

        let chars: Vec<char> = self.expression.chars().collect();
        let start = suggestions.start.min(chars.len());
        let end = (suggestions.start + suggestions.length).min(chars.len());
        let before: String = chars[..start].iter().collect();
        let after: String = chars[end..].iter().collect();

        self.expression = format!("{before}{}{after}", item.insert);
        let caret = start + item.insert.chars().count();
        self.recalled = None;
        self.update_suggestions(caret);
        Some(caret)
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;
    use crate::calculator::engine::testing::{FixedClock, calculator_at};

    /// In winter, so Mexico City is UTC-6.
    fn winter() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 15, 12, 0, 0).unwrap()
    }

    fn presenter() -> Presenter {
        Presenter::new(calculator_at(FixedClock::at(2026, 1, 15, 16, 0, "America/Mexico_City")), winter)
    }

    /// Types a whole line, as the text box would report it.
    fn type_line(presenter: &mut Presenter, line: &str) {
        presenter.expression = line.to_string();
        presenter.expression_changed(line.chars().count());
    }

    fn enter(presenter: &mut Presenter, line: &str) -> Option<Selection> {
        type_line(presenter, line);
        presenter.evaluate()
    }

    fn texts(presenter: &Presenter) -> Vec<String> {
        presenter
            .suggestions()
            .map_or_else(Vec::new, |s| s.items.iter().map(|item| item.text.clone()).collect())
    }

    fn result(text: &str) -> Output {
        Output::Result(text.to_string())
    }

    #[test]
    fn shows_the_formatted_result() {
        for (line, expected) in
            [("2+2*3", "8"), ("10000", "10,000"), ("1/4", "0.25"), ("3 km to m", "3,000 m")]
        {
            let mut presenter = presenter();
            enter(&mut presenter, line);
            assert_eq!(presenter.output(), &result(expected), "{line}");
        }
    }

    #[test]
    fn an_empty_line_shows_nothing_and_isnt_kept() {
        for line in ["", "   "] {
            let mut presenter = presenter();
            assert_eq!(enter(&mut presenter, line), None);
            assert_eq!(presenter.output(), &Output::Nothing);
            assert!(presenter.history().is_empty());
        }
    }

    #[test]
    fn an_error_selects_the_character_at_fault_or_the_end() {
        let mut presenter = presenter();
        assert_eq!(enter(&mut presenter, "1 $ 2"), Some(Selection { start: 2, length: 1 }));
        assert_eq!(presenter.output(), &Output::Error("Invalid character '$' at position 3".into()));

        assert_eq!(enter(&mut presenter, "1 +"), Some(Selection { start: 3, length: 0 }));
        assert_eq!(presenter.result_to_copy(), None);
    }

    #[test]
    fn the_history_keeps_what_worked_oldest_first_up_to_the_newest_200() {
        let mut presenter = presenter();
        enter(&mut presenter, "1+1");
        enter(&mut presenter, "2+)");
        enter(&mut presenter, "  2*3 ");
        assert_eq!(
            presenter.history(),
            [
                HistoryEntry { operation: "1+1".into(), result: "2".into() },
                HistoryEntry { operation: "2*3".into(), result: "6".into() },
            ]
        );

        for i in 0..MAX_HISTORY {
            enter(&mut presenter, &i.to_string());
        }
        assert_eq!(presenter.history().len(), MAX_HISTORY);
        assert_eq!(presenter.history()[0].operation, "0");
    }

    #[test]
    fn toggling_the_history_shows_and_hides_it() {
        let mut presenter = presenter();
        presenter.toggle_history();
        assert!(presenter.history_shown());
        presenter.toggle_history();
        assert!(!presenter.history_shown());
    }

    #[test]
    fn a_time_then_places_write_the_conversion_which_calculates() {
        let mut presenter = presenter();
        type_line(&mut presenter, "10:30 ");
        assert_eq!(texts(&presenter)[0], "Mexico City (UTC-6)");

        // The first place writes "to", so the second follows.
        assert_eq!(presenter.choose(0), Some(21));
        assert_eq!(presenter.expression, "10:30 Mexico City to ");
        assert!(texts(&presenter).contains(&"India (UTC+5:30)".to_string()));

        type_line(&mut presenter, "10:30 Mexico City to ind");
        assert_eq!(texts(&presenter), ["India (UTC+5:30)"]);
        presenter.choose(0);
        assert_eq!(presenter.expression, "10:30 Mexico City to India");

        assert_eq!(presenter.evaluate(), None);
        assert_eq!(presenter.suggestions(), None);
    }

    #[test]
    fn a_number_then_units_write_the_conversion_with_units_of_the_same_kind() {
        let mut presenter = presenter();
        type_line(&mut presenter, "10 ");
        let km = texts(&presenter).iter().position(|t| t == "km (kilometer, length)").unwrap();
        assert!(texts(&presenter).contains(&"kg (kilogram, mass)".to_string()));

        presenter.choose(km);
        assert_eq!(presenter.expression, "10 km to ");
        assert!(texts(&presenter).iter().all(|text| text.ends_with("length)")));
        assert!(!texts(&presenter).contains(&"km (kilometer, length)".to_string()));
    }

    #[test]
    fn a_choice_when_none_are_shown_does_nothing() {
        let mut presenter = presenter();
        type_line(&mut presenter, "2+2");
        assert_eq!(presenter.choose(0), None);
        assert_eq!(presenter.expression, "2+2");
    }

    #[test]
    fn moving_the_choice_stops_at_the_ends() {
        let mut presenter = presenter();
        type_line(&mut presenter, "SQ"); // one suggestion: sqrt
        presenter.move_choice(false);
        presenter.move_choice(false);
        assert_eq!(presenter.chosen, Some(0));
        presenter.move_choice(true);
        assert_eq!(presenter.chosen, None);
    }

    #[test]
    fn the_names_given_values_and_the_last_answer_are_offered_with_their_values() {
        let mut presenter = presenter();
        enter(&mut presenter, "rate = 1500");
        enter(&mut presenter, "2 * 21");

        type_line(&mut presenter, "10 * r");
        assert_eq!(
            texts(&presenter),
            ["rate: 1,500", "round(x, decimals): rounds; decimals are optional"]
        );
        type_line(&mut presenter, "an");
        assert_eq!(texts(&presenter), ["ans: the last answer, 42"]);
    }

    #[test]
    fn up_goes_back_through_the_lines_entered_and_down_past_the_newest_brings_back_the_draft() {
        let mut presenter = presenter();
        // A line that failed comes back too, to be fixed; the same line twice is there once.
        enter(&mut presenter, "1+1");
        enter(&mut presenter, "2+");
        enter(&mut presenter, "2+");
        type_line(&mut presenter, "3");

        assert_eq!(presenter.recall(true), Some(2));
        assert_eq!(presenter.expression, "2+");
        presenter.recall(true);
        assert_eq!(presenter.expression, "1+1");
        assert_eq!(presenter.recall(true), None);
        assert_eq!(presenter.expression, "1+1");
        presenter.recall(false);
        assert_eq!(presenter.expression, "2+");
        presenter.recall(false);
        assert_eq!(presenter.expression, "3");
        assert_eq!(presenter.recall(false), None);
        assert_eq!(presenter.expression, "3");
    }

    #[test]
    fn a_line_brought_back_offers_no_suggestions_and_typing_starts_over() {
        let mut presenter = presenter();
        enter(&mut presenter, "10 km to mi");
        enter(&mut presenter, "2+2");

        presenter.recall(true);
        presenter.recall(true);
        assert_eq!(presenter.expression, "10 km to mi");
        assert_eq!(presenter.suggestions(), None);

        type_line(&mut presenter, "5");
        presenter.recall(true);
        assert_eq!(presenter.expression, "2+2");
    }

    #[test]
    fn recall_with_nothing_entered_does_nothing() {
        let mut presenter = presenter();
        type_line(&mut presenter, "7");
        assert_eq!(presenter.recall(true), None);
        assert_eq!(presenter.recall(false), None);
        assert_eq!(presenter.expression, "7");
    }

    #[test]
    fn the_last_answer_and_names_given_are_kept_for_the_next_lines() {
        let mut presenter = presenter();
        enter(&mut presenter, "2+3");
        enter(&mut presenter, "ans * 2");
        assert_eq!(presenter.output(), &result("10"));

        enter(&mut presenter, "x = 4");
        assert_eq!(presenter.output(), &result("4"));
        enter(&mut presenter, "X * ans");
        assert_eq!(presenter.output(), &result("16"));

        // A time in another place has no value, so the last answer stays.
        enter(&mut presenter, "10:00 to UTC");
        enter(&mut presenter, "ans");
        assert_eq!(presenter.output(), &result("16"));
    }

    #[test]
    fn a_line_that_fails_leaves_the_names_as_they_were() {
        let mut presenter = presenter();
        enter(&mut presenter, "x = 4");
        enter(&mut presenter, "x = 1/0");
        enter(&mut presenter, "x");
        assert_eq!(presenter.output(), &result("4"));
    }

    #[test]
    fn only_a_result_can_be_copied_not_an_error() {
        let mut presenter = presenter();
        assert_eq!(presenter.result_to_copy(), None);
        enter(&mut presenter, "6 * 7");
        assert_eq!(presenter.result_to_copy(), Some("42"));
        enter(&mut presenter, "6 *");
        assert_eq!(presenter.result_to_copy(), None);
    }
}
