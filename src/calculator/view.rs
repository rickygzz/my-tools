//! Draws the calculator with egui, and turns keys and clicks into calls on the [`Presenter`].
//!
//! egui is an *immediate mode* GUI: there are no widget objects to keep. Every frame, [`draw`]
//! lays the whole window out again from the presenter's state, and each widget call returns
//! what happened to it this frame (clicked, changed...).
//!
//! ```text
//! ┌──────────────────────────────────────────────────────────────┐
//! │ [ text box: the line                                       ] │
//! │  ┌ suggestions, over what's below ┐                          │
//! │ [ result, error, or the history                     ] [ v ] │
//! └──────────────────────────────────────────────────────────────┘
//! ```

use egui::text::{CCursor, CCursorRange, LayoutJob};
use egui::text_edit::TextEditState;
use egui::{
    Align2, Color32, CornerRadius, Event, FontId, Id, Key, Modifiers, Pos2, Rect, Sense, Stroke,
    TextEdit, TextFormat, Ui, UiBuilder, Vec2, pos2, vec2,
};

use super::presenter::{NO_HISTORY, Output, Presenter, Selection};

/// The window's width, in points (logical pixels).
pub const WIDTH: f32 = 800.0;
/// The window's height without the history or suggestions.
pub const BASE_HEIGHT: f32 = PADDING + INPUT_HEIGHT + GAP + OUTPUT_HEIGHT + PADDING;

const PADDING: f32 = 6.0;
const GAP: f32 = 8.0;
const INPUT_HEIGHT: f32 = 34.0;
const OUTPUT_HEIGHT: f32 = 38.0;
/// How many times its normal height the output is while it shows the history.
const HISTORY_FACTOR: f32 = 8.0;
const BUTTON_WIDTH: f32 = 34.0;
const SUGGESTIONS_WIDTH: f32 = 440.0;
const SUGGESTION_HEIGHT: f32 = 24.0;
/// Rows the suggestions show at once; the list scrolls for more.
const MAX_SUGGESTION_ROWS: usize = 8;
/// How long "Copied" shows, in seconds.
const COPIED_TIME: f64 = 1.2;
/// How far Ctrl+Arrow moves the window, in points; with Shift, the larger step.
const MOVE_STEP: f32 = 10.0;
const BIG_MOVE_STEP: f32 = 120.0;

const HINT: &str = "Enter expression (e.g. 2+2*3)";

const BACKGROUND: Color32 = Color32::from_rgb(0x2B, 0x2B, 0x2B);
const BORDER: Color32 = Color32::from_rgb(0x55, 0x55, 0x55);
const OUTPUT_BACKGROUND: Color32 = Color32::from_rgb(0x1E, 0x1E, 0x1E);
const RESULT_COLOR: Color32 = Color32::from_rgb(0xF5, 0xF5, 0xF5);
const OPERATION_COLOR: Color32 = Color32::from_rgb(0x8C, 0xB4, 0xD8);
const ERROR_COLOR: Color32 = Color32::from_rgb(0xFF, 0x8A, 0x80);

/// What the window itself must do, which egui can't.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Action {
    Hide,
    /// Move the window with the mouse, while its button is down.
    StartDrag,
    /// Move the window by this many points.
    MoveBy(Vec2),
}

/// What the view keeps between frames, besides the presenter.
#[derive(Default)]
pub struct ViewState {
    /// Just shown: select the whole line, so typing replaces it.
    pub just_shown: bool,
    /// Until when (egui's clock, in seconds) "Copied" shows.
    copied_until: Option<f64>,
}

/// The height the window needs, in points, for what it shows now.
pub fn height(presenter: &Presenter) -> f32 {
    let output = if presenter.history_shown() { OUTPUT_HEIGHT * HISTORY_FACTOR } else { OUTPUT_HEIGHT };
    let base = PADDING + INPUT_HEIGHT + GAP + output + PADDING;
    let suggestions = presenter.suggestions().map_or(0.0, |s| {
        suggestions_top() + list_height(s.items.len()) + PADDING
    });
    base.max(suggestions)
}

fn suggestions_top() -> f32 {
    PADDING + INPUT_HEIGHT + 2.0
}

fn list_height(rows: usize) -> f32 {
    rows.min(MAX_SUGGESTION_ROWS) as f32 * SUGGESTION_HEIGHT + 6.0
}

fn input_id() -> Id {
    Id::unique("calculator-input")
}

/// Draws one frame. Gives what the window must do.
pub fn draw(ui: &mut Ui, presenter: &mut Presenter, view: &mut ViewState) -> Vec<Action> {
    let mut actions = Vec::new();
    let ctx = ui.ctx().clone();
    let full = ui.max_rect();

    ui.painter().rect(full, CornerRadius::ZERO, BACKGROUND, Stroke::new(1.0, BORDER), egui::StrokeKind::Inside);

    // Dragging the window anywhere but the text box and the buttons moves it. Added first, so
    // it's under everything else, which gets the mouse first.
    let background = ui.interact(full, Id::unique("calculator-background"), Sense::drag());
    if background.drag_started() {
        actions.push(Action::StartDrag);
    }

    // Keys first, before the text box sees them.
    let mut caret = None;
    let mut selection = None;
    handle_keys(ui, presenter, &mut actions, &mut caret, &mut selection);
    handle_copy(ui, presenter, view);

    if view.just_shown {
        view.just_shown = false;
        selection = Some(Selection { start: 0, length: presenter.expression.chars().count() });
    }
    if let Some(caret) = caret {
        selection = Some(Selection { start: caret, length: 0 });
    }
    if let Some(selection) = selection {
        select(&ctx, selection);
    }

    // The text box keeps the keyboard, whatever was clicked.
    ctx.memory_mut(|memory| memory.request_focus(input_id()));

    let input_rect = Rect::from_min_size(pos2(PADDING, PADDING), vec2(full.width() - 2.0 * PADDING, INPUT_HEIGHT));
    let input = ui
        .scope_builder(UiBuilder::new().max_rect(input_rect), |ui| {
            TextEdit::singleline(&mut presenter.expression)
                .id(input_id())
                .hint_text(HINT)
                .font(FontId::proportional(20.0))
                .desired_width(input_rect.width())
                .margin(vec2(6.0, 5.0))
                .show(ui)
        })
        .inner;
    if input.response.changed() {
        let caret = input.cursor_range.map_or(presenter.expression.chars().count(), |range| range.primary.index.0);
        presenter.expression_changed(caret);
    }

    draw_output(ui, presenter, view, full);
    draw_suggestions(ui, presenter);

    actions
}

/// Enter, Esc, Tab, Up, Down, Ctrl+H and Ctrl+Arrow. While suggestions show, Up and Down go
/// through them, Tab writes the one chosen (else the first), Enter the one chosen (else it
/// calculates), and Esc hides them rather than the calculator.
fn handle_keys(
    ui: &mut Ui,
    presenter: &mut Presenter,
    actions: &mut Vec<Action>,
    caret: &mut Option<usize>,
    selection: &mut Option<Selection>,
) {
    let pressed = |ui: &mut Ui, modifiers: Modifiers, key: Key| ui.input_mut(|i| i.consume_key(modifiers, key));

    if presenter.suggestions().is_some() {
        if pressed(ui, Modifiers::NONE, Key::ArrowDown) {
            presenter.move_choice(false);
        }
        if pressed(ui, Modifiers::NONE, Key::ArrowUp) {
            presenter.move_choice(true);
        }
        if pressed(ui, Modifiers::NONE, Key::Tab) {
            *caret = presenter.choose(presenter.chosen.unwrap_or(0));
        }
        if let Some(chosen) = presenter.chosen
            && pressed(ui, Modifiers::NONE, Key::Enter)
        {
            *caret = presenter.choose(chosen);
        }
        if pressed(ui, Modifiers::NONE, Key::Escape) {
            presenter.hide_suggestions();
        }
    }

    if pressed(ui, Modifiers::NONE, Key::Escape) {
        actions.push(Action::Hide);
    }
    if pressed(ui, Modifiers::NONE, Key::ArrowUp) {
        *caret = presenter.recall(true).or(*caret);
    }
    if pressed(ui, Modifiers::NONE, Key::ArrowDown) {
        *caret = presenter.recall(false).or(*caret);
    }
    if pressed(ui, Modifiers::NONE, Key::Enter) {
        *selection = presenter.evaluate();
    }
    // Tab would move the keyboard away from the text box.
    pressed(ui, Modifiers::NONE, Key::Tab);
    if pressed(ui, Modifiers::CTRL, Key::H) {
        presenter.toggle_history();
    }

    // Ctrl+Arrow moves the window; with Shift, further. The most specific keys go first.
    for (modifiers, step) in [(Modifiers::CTRL | Modifiers::SHIFT, BIG_MOVE_STEP), (Modifiers::CTRL, MOVE_STEP)] {
        for (key, direction) in [
            (Key::ArrowLeft, vec2(-1.0, 0.0)),
            (Key::ArrowRight, vec2(1.0, 0.0)),
            (Key::ArrowUp, vec2(0.0, -1.0)),
            (Key::ArrowDown, vec2(0.0, 1.0)),
        ] {
            if pressed(ui, modifiers, key) {
                actions.push(Action::MoveBy(direction * step));
            }
        }
    }
}

/// Ctrl+C with nothing selected in the line copies the result, and says so for a moment. With
/// text selected, the text box copies that, as usual.
fn handle_copy(ui: &mut Ui, presenter: &Presenter, view: &mut ViewState) {
    let ctx = ui.ctx().clone();
    let nothing_selected = TextEditState::load(&ctx, input_id())
        .and_then(|state| state.cursor.char_range())
        .is_none_or(|range| range.is_empty());
    let Some(result) = presenter.result_to_copy() else { return };
    if !nothing_selected {
        return;
    }

    let copy = ui.input_mut(|i| {
        let asked = i.events.iter().any(|event| matches!(event, Event::Copy));
        i.events.retain(|event| !matches!(event, Event::Copy));
        asked
    });
    if copy {
        ctx.copy_text(result.to_string());
        view.copied_until = Some(ctx.input(|i| i.time) + COPIED_TIME);
    }
}

/// Selects part of the line (or puts the caret somewhere, with a length of 0).
fn select(ctx: &egui::Context, selection: Selection) {
    let mut state = TextEditState::load(ctx, input_id()).unwrap_or_default();
    let range = CCursorRange::two(CCursor::new(selection.start), CCursor::new(selection.start + selection.length));
    state.cursor.set_char_range(Some(range));
    state.store(ctx, input_id());
}

/// The result, the error, or the history, with the button that shows and hides the history.
fn draw_output(ui: &mut Ui, presenter: &mut Presenter, view: &mut ViewState, full: Rect) {
    let top = PADDING + INPUT_HEIGHT + GAP;
    let output_rect = Rect::from_min_max(
        pos2(PADDING, top),
        pos2(full.right() - PADDING - BUTTON_WIDTH - 2.0, full.bottom() - PADDING),
    );
    let button_rect = Rect::from_min_max(
        pos2(output_rect.right() + 2.0, top),
        pos2(full.right() - PADDING, top + OUTPUT_HEIGHT),
    );

    ui.painter().rect_filled(output_rect, CornerRadius::same(2), OUTPUT_BACKGROUND);
    let inner = output_rect.shrink2(vec2(8.0, 6.0));

    ui.scope_builder(UiBuilder::new().max_rect(inner), |ui| {
        // Not selectable, so dragging here moves the window (Ctrl+C copies the result).
        ui.style_mut().interaction.selectable_labels = false;
        if presenter.history_shown() {
            // With a mouse, it scrolls by the wheel or the bar; a drag here moves the window.
            let history = egui::ScrollArea::vertical().auto_shrink(false).stick_to_bottom(true);
            history.show(ui, |ui| {
                if presenter.history().is_empty() {
                    ui.label(colored(NO_HISTORY, OPERATION_COLOR, 17.0));
                }
                for entry in presenter.history() {
                    let mut job = LayoutJob::default();
                    job.append(&format!("{} = ", entry.operation), 0.0, format(OPERATION_COLOR, 17.0));
                    job.append(&entry.result, 0.0, format(RESULT_COLOR, 17.0));
                    ui.label(job);
                }
                // An error goes below the history.
                if let Output::Error(message) = presenter.output() {
                    ui.label(colored(message, ERROR_COLOR, 17.0));
                }
            });
        } else {
            match presenter.output() {
                Output::Nothing => {}
                Output::Result(result) => {
                    ui.label(colored(result, RESULT_COLOR, 20.0));
                }
                Output::Error(message) => {
                    ui.label(colored(message, ERROR_COLOR, 17.0));
                }
            }
        }
    });

    if let Some(until) = view.copied_until {
        let now = ui.input(|i| i.time);
        if now < until {
            ui.painter().text(
                output_rect.right_top() + vec2(-8.0, 6.0),
                Align2::RIGHT_TOP,
                "Copied",
                FontId::proportional(14.0),
                OPERATION_COLOR,
            );
            ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(until - now));
        } else {
            view.copied_until = None;
        }
    }

    // The history button: a chevron pointing down while the history is hidden, up while it shows.
    let button = ui.interact(button_rect, Id::unique("calculator-history"), Sense::click());
    let fill = if button.hovered() { Color32::from_gray(0x5A) } else { OUTPUT_BACKGROUND };
    ui.painter().rect_filled(button_rect, CornerRadius::same(2), fill);
    draw_chevron(ui, button_rect.center(), presenter.history_shown());
    let tip = if presenter.history_shown() { "Hide history (Ctrl+H)" } else { "Show history (Ctrl+H)" };
    if button.on_hover_text(tip).clicked() {
        presenter.toggle_history();
    }
}

fn draw_chevron(ui: &Ui, center: Pos2, up: bool) {
    let (dx, dy) = (5.0, if up { -2.5 } else { 2.5 });
    let points = vec![center + vec2(-dx, -dy), center + vec2(0.0, dy), center + vec2(dx, -dy)];
    ui.painter().add(egui::Shape::line(points, Stroke::new(1.5, RESULT_COLOR)));
}

/// The list of suggestions, under the text box and over the output.
fn draw_suggestions(ui: &mut Ui, presenter: &mut Presenter) {
    let Some(suggestions) = presenter.suggestions() else { return };
    let texts: Vec<String> = suggestions.items.iter().map(|item| item.text.clone()).collect();
    let chosen = presenter.chosen;
    let mut clicked = None;

    egui::Area::new(Id::unique("calculator-suggestions"))
        .order(egui::Order::Foreground)
        .fixed_pos(pos2(PADDING, suggestions_top()))
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).inner_margin(3.0).show(ui, |ui| {
                ui.set_width(SUGGESTIONS_WIDTH);
                let rows_height = list_height(texts.len()) - 6.0;
                egui::ScrollArea::vertical()
                    .max_height(rows_height)
                    .min_scrolled_height(rows_height)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        for (index, text) in texts.iter().enumerate() {
                            // The space that grows after the text pushes it to the left.
                            let row = egui::Button::selectable(chosen == Some(index), (colored(text, RESULT_COLOR, 15.0), egui::Atom::grow()))
                                .frame_when_inactive(false)
                                .min_size(vec2(ui.available_width(), SUGGESTION_HEIGHT));
                            let row = ui.add(row);
                            if chosen == Some(index) {
                                row.scroll_to_me(None);
                            }
                            if row.clicked() {
                                clicked = Some(index);
                            }
                        }
                    });
            });
        });

    if let Some(index) = clicked {
        let caret = presenter.choose(index);
        if let Some(caret) = caret {
            select(ui.ctx(), Selection { start: caret, length: 0 });
        }
    }
}

fn format(color: Color32, size: f32) -> TextFormat {
    TextFormat { font_id: FontId::monospace(size), color, ..Default::default() }
}

fn colored(text: &str, color: Color32, size: f32) -> egui::RichText {
    egui::RichText::new(text).color(color).font(FontId::monospace(size))
}

/// The window driven by simulated keys and clicks, without a screen: `egui_kittest` runs the
/// frames and gives each one the input, as the real window does.
#[cfg(test)]
mod tests {
    use chrono::{DateTime, TimeZone, Utc};
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::*;
    use crate::calculator::engine::testing::{FixedClock, calculator_at};

    /// What the window keeps: the presenter, the view's own state, and the actions asked of
    /// the window so far.
    struct Window {
        presenter: Presenter,
        view: ViewState,
        actions: Vec<Action>,
    }

    /// In winter, so Mexico City is UTC-6.
    fn winter() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 15, 12, 0, 0).unwrap()
    }

    fn window() -> Harness<'static, Window> {
        let calculator = calculator_at(FixedClock::at(2026, 1, 15, 16, 0, "America/Mexico_City"));
        let state = Window {
            presenter: Presenter::new(calculator, winter),
            view: ViewState::default(),
            actions: Vec::new(),
        };
        let mut harness = Harness::builder().with_size(vec2(WIDTH, 450.0)).build_ui_state(
            |ui, window: &mut Window| {
                let actions = draw(ui, &mut window.presenter, &mut window.view);
                window.actions.extend(actions);
            },
            state,
        );
        harness.step();
        harness
    }

    fn type_text(harness: &mut Harness<'_, Window>, text: &str) {
        harness.event(Event::Text(text.to_string()));
        harness.step();
    }

    fn press(harness: &mut Harness<'_, Window>, key: Key) {
        press_with(harness, Modifiers::NONE, key);
    }

    fn press_with(harness: &mut Harness<'_, Window>, modifiers: Modifiers, key: Key) {
        // A real Ctrl press outside macOS also sets `command`, which egui reads for Ctrl+A.
        let command = modifiers.command || (modifiers.ctrl && !cfg!(target_os = "macos"));
        harness.key_press_modifiers(Modifiers { command, ..modifiers }, key);
        harness.step();
    }

    fn line<'a>(harness: &'a Harness<'_, Window>) -> &'a str {
        &harness.state().presenter.expression
    }

    /// The part of the line selected, as (start, end) in characters.
    fn selected(harness: &Harness<'_, Window>) -> Option<(usize, usize)> {
        let state = TextEditState::load(&harness.ctx, input_id())?;
        let range = state.cursor.char_range()?;
        let (start, end) = (range.primary.index.0, range.secondary.index.0);
        Some((start.min(end), start.max(end)))
    }

    fn copied(harness: &Harness<'_, Window>) -> Option<String> {
        harness.output().platform_output.commands.iter().find_map(|command| match command {
            egui::OutputCommand::CopyText(text) => Some(text.clone()),
            _ => None,
        })
    }

    #[test]
    fn typing_then_enter_shows_the_result_and_keeps_the_line() {
        let mut harness = window();
        type_text(&mut harness, "2+2*3");
        press(&mut harness, Key::Enter);

        assert_eq!(harness.state().presenter.output(), &Output::Result("8".into()));
        assert_eq!(line(&harness), "2+2*3");
    }

    #[test]
    fn an_error_selects_the_character_at_fault() {
        let mut harness = window();
        type_text(&mut harness, "1/0");
        press(&mut harness, Key::Enter);

        let error = Output::Error("Division by zero at position 2".into());
        assert_eq!(harness.state().presenter.output(), &error);
        assert_eq!(selected(&harness), Some((1, 2)));
    }

    #[test]
    fn when_shown_the_whole_line_is_selected_so_typing_replaces_it() {
        let mut harness = window();
        type_text(&mut harness, "old line");
        harness.state_mut().view.just_shown = true;
        harness.step();
        assert_eq!(selected(&harness), Some((0, 8)));

        type_text(&mut harness, "7");
        assert_eq!(line(&harness), "7");
    }

    #[test]
    fn tab_writes_the_first_suggestion_and_the_list_goes_on() {
        let mut harness = window();
        type_text(&mut harness, "10 k");
        press(&mut harness, Key::Tab);

        assert_eq!(line(&harness), "10 km to ");
        assert_eq!(selected(&harness), Some((9, 9)), "the caret goes after what was written");
        let next = harness.state().presenter.suggestions().expect("the units to convert to");
        assert!(next.items.iter().all(|item| item.text.ends_with("length)")));
    }

    #[test]
    fn down_goes_through_the_list_and_enter_writes_the_one_chosen() {
        let mut harness = window();
        type_text(&mut harness, "3 ");
        press(&mut harness, Key::ArrowDown);
        press(&mut harness, Key::ArrowDown);
        press(&mut harness, Key::Enter);

        assert_eq!(line(&harness), "3 um to ");
        assert_eq!(harness.state().presenter.output(), &Output::Nothing, "Enter chose; it didn't calculate");
    }

    #[test]
    fn enter_with_nothing_chosen_in_the_list_calculates() {
        let mut harness = window();
        type_text(&mut harness, "10 ");
        assert!(harness.state().presenter.suggestions().is_some());
        press(&mut harness, Key::Enter);

        assert_eq!(harness.state().presenter.output(), &Output::Result("10".into()));
        assert!(harness.state().presenter.suggestions().is_none());
    }

    #[test]
    fn clicking_a_suggestion_writes_it() {
        let mut harness = window();
        type_text(&mut harness, "10:30 ind");
        harness.step(); // a new list is measured, unseen, on its first frame
        harness.get_by_label("India (UTC+5:30)").click();
        harness.step();

        assert_eq!(line(&harness), "10:30 India to ");
    }

    #[test]
    fn esc_first_closes_the_list_then_hides_the_window() {
        let mut harness = window();
        type_text(&mut harness, "10 ");
        press(&mut harness, Key::Escape);
        assert!(harness.state().presenter.suggestions().is_none());
        assert!(harness.state().actions.is_empty());

        press(&mut harness, Key::Escape);
        assert_eq!(harness.state().actions, [Action::Hide]);
    }

    #[test]
    fn up_brings_back_the_line_before_and_down_the_one_being_typed() {
        let mut harness = window();
        type_text(&mut harness, "1+1");
        press(&mut harness, Key::Enter);
        press_with(&mut harness, Modifiers::CTRL, Key::A);
        type_text(&mut harness, "draft");

        press(&mut harness, Key::ArrowUp);
        assert_eq!(line(&harness), "1+1");
        assert_eq!(selected(&harness), Some((3, 3)));

        press(&mut harness, Key::ArrowDown);
        assert_eq!(line(&harness), "draft");
    }

    #[test]
    fn ctrl_c_with_nothing_selected_copies_the_result_and_says_so() {
        let mut harness = window();
        type_text(&mut harness, "6*7");
        press(&mut harness, Key::Enter);
        harness.event(Event::Copy);
        harness.step();

        assert_eq!(copied(&harness).as_deref(), Some("42"));
        assert!(harness.state().view.copied_until.is_some(), "it says Copied");
    }

    #[test]
    fn ctrl_c_with_text_selected_copies_that_text() {
        let mut harness = window();
        type_text(&mut harness, "6*7");
        press(&mut harness, Key::Enter);
        press_with(&mut harness, Modifiers::CTRL, Key::A);
        harness.event(Event::Copy);
        harness.step();

        assert_eq!(copied(&harness).as_deref(), Some("6*7"));
    }

    #[test]
    fn ctrl_c_after_an_error_copies_nothing() {
        let mut harness = window();
        type_text(&mut harness, "1 +");
        press(&mut harness, Key::Enter);
        harness.event(Event::Copy);
        harness.step();

        assert_eq!(copied(&harness), None);
    }

    #[test]
    fn ctrl_h_and_the_arrow_button_show_and_hide_the_history() {
        let mut harness = window();
        type_text(&mut harness, "2+3");
        press(&mut harness, Key::Enter);

        press_with(&mut harness, Modifiers::CTRL, Key::H);
        assert!(harness.state().presenter.history_shown());
        assert!(harness.query_by_label("2+3 = 5").is_some());

        harness.get_by_label("Hide history (Ctrl+H)").click();
        harness.step();
        assert!(!harness.state().presenter.history_shown());
    }

    #[test]
    fn ctrl_arrows_move_the_window_and_shift_moves_it_further() {
        let mut harness = window();
        press_with(&mut harness, Modifiers::CTRL, Key::ArrowLeft);
        press_with(&mut harness, Modifiers::CTRL | Modifiers::SHIFT, Key::ArrowDown);

        assert_eq!(
            harness.state().actions,
            [Action::MoveBy(vec2(-MOVE_STEP, 0.0)), Action::MoveBy(vec2(0.0, BIG_MOVE_STEP))]
        );
        assert_eq!(line(&harness), "", "the keys didn't reach the line");
    }

    #[test]
    fn every_control_has_a_name_for_screen_readers() {
        let mut harness = window();
        press_with(&mut harness, Modifiers::CTRL, Key::H);
        type_text(&mut harness, "10 ");
        harness.step(); // the list shows from its second frame

        harness.check_accessibility();
    }

    #[test]
    fn the_window_grows_for_the_history_and_the_suggestions() {
        let mut harness = window();
        assert_eq!(height(&harness.state().presenter), BASE_HEIGHT);

        type_text(&mut harness, "10 ");
        let with_list = height(&harness.state().presenter);
        assert_eq!(with_list, suggestions_top() + list_height(MAX_SUGGESTION_ROWS) + PADDING);

        press(&mut harness, Key::Escape);
        press_with(&mut harness, Modifiers::CTRL, Key::H);
        assert!(height(&harness.state().presenter) > with_list);
    }
}
