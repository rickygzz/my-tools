//! Draws the zones editor with egui, and turns the mouse and keys into presenter calls: a
//! toolbar, a monitor drawn at its real shape with the zones over it, numbered, gaps included,
//! and a line for problems.

use egui::{
    Align2, Color32, CornerRadius, CursorIcon, FontId, Key, Modifiers, PointerButton, Pos2, Rect, Sense, Stroke,
    StrokeKind, Vec2, pos2, vec2,
};

use super::presenter::{Cursor, EditorPointer, Leaving, Presenter, Store};
use crate::zones::geometry::drawing_order;
use crate::zones::layout::Template;

/// How near a border, in points of the window, counts as on it.
const GRAB: f32 = 6.0;
const HANDLE: f32 = 8.0;

const MONITOR: Color32 = Color32::from_rgb(24, 24, 28);
const ZONE: Color32 = Color32::from_rgb(58, 58, 64);
const SELECTED: Color32 = Color32::from_rgb(0, 120, 215);
const PROBLEM: Color32 = Color32::from_rgb(255, 120, 110);

/// What the window must do after a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Leave(Leaving),
}

/// What the view keeps between frames.
#[derive(Default)]
pub struct ViewState {
    /// The last right click was on a zone, so its menu has something to offer.
    menu_on_zone: bool,
}

pub fn draw(ui: &mut egui::Ui, editor: &mut Presenter<impl Store>, view: &mut ViewState) -> Vec<Action> {
    let mut actions = Vec::new();
    keys(ui, editor, &mut actions);

    egui::Panel::top("toolbar").show(ui, |ui| toolbar(ui, editor, &mut actions));
    egui::Panel::bottom("status").show(ui, |ui| status(ui, editor));
    canvas(ui, editor, view);
    ask(ui, editor, &mut actions);
    actions
}

fn keys(ui: &mut egui::Ui, editor: &mut Presenter<impl Store>, actions: &mut Vec<Action>) {
    let pressed = |ui: &mut egui::Ui, modifiers: Modifiers, key: Key| ui.input_mut(|i| i.consume_key(modifiers, key));
    if pressed(ui, Modifiers::COMMAND | Modifiers::SHIFT, Key::Z) || pressed(ui, Modifiers::COMMAND, Key::Y) {
        editor.redo();
    }
    if pressed(ui, Modifiers::COMMAND, Key::Z) {
        editor.undo();
    }
    if pressed(ui, Modifiers::COMMAND, Key::S) {
        editor.save();
    }
    if pressed(ui, Modifiers::NONE, Key::Delete) {
        editor.delete();
    }
    if pressed(ui, Modifiers::NONE, Key::Escape) && editor.leave(Leaving::Close) {
        actions.push(Action::Leave(Leaving::Close));
    }
}

fn toolbar(ui: &mut egui::Ui, editor: &mut Presenter<impl Store>, actions: &mut Vec<Action>) {
    ui.horizontal_wrapped(|ui| {
        ui.label("Edit layout of");
        let targets = editor.targets();
        let mut target = editor.target();
        egui::ComboBox::from_id_salt("target").selected_text(targets[target].clone()).show_ui(ui, |ui| {
            for (index, name) in targets.iter().enumerate() {
                ui.selectable_value(&mut target, index, name);
            }
        });
        editor.set_target(target);

        ui.menu_button("Layout", |ui| {
            for template in Template::ALL {
                if ui.button(template.label()).clicked() {
                    editor.template(template);
                    ui.close();
                }
            }
        });

        ui.label("Gap");
        let mut gap = editor.gap();
        egui::ComboBox::from_id_salt("gap").selected_text(format!("{gap} px")).show_ui(ui, |ui| {
            for choice in editor.gaps() {
                ui.selectable_value(&mut gap, choice, format!("{choice} px"));
            }
        });
        editor.set_gap(gap);

        ui.separator();
        if ui.add_enabled(editor.can_undo(), egui::Button::new("Undo")).on_hover_text("Ctrl+Z").clicked() {
            editor.undo();
        }
        if ui.add_enabled(editor.can_redo(), egui::Button::new("Redo")).on_hover_text("Ctrl+Y").clicked() {
            editor.redo();
        }
        if ui.add_enabled(editor.unsaved(), egui::Button::new("Save")).on_hover_text("Ctrl+S").clicked() {
            editor.save();
        }
        if ui.button("Edit as text...").on_hover_text("Opens zones.cfg in your text editor").clicked()
            && editor.leave(Leaving::EditAsText)
        {
            actions.push(Action::Leave(Leaving::EditAsText));
        }
    });
    match editor.own_layout() {
        (true, true) => {
            if ui.button("Use default layout").clicked() {
                editor.use_default();
            }
        }
        (true, false) => {
            ui.weak("Uses the default; a change gives it its own.");
        }
        _ => {}
    }
}

fn status(ui: &mut egui::Ui, editor: &Presenter<impl Store>) {
    match &editor.problem {
        Some(problem) => ui.colored_label(PROBLEM, problem),
        None => ui.weak("Drag a border to resize, inside a zone to move it, on empty space to add one. Right-click a zone to split or delete it."),
    };
}

/// The monitor at its shape, as large as fits, with the zones over it.
fn canvas(ui: &mut egui::Ui, editor: &mut Presenter<impl Store>, view: &mut ViewState) {
    let (width, height) = editor.shape();
    let (width, height) = (width.max(1) as f32, height.max(1) as f32);
    let space = ui.available_rect_before_wrap().shrink(12.0);
    let scale = (space.width() / width).min(space.height() / height).max(0.01);
    let rect = Rect::from_center_size(space.center(), vec2(width * scale, height * scale));
    let response = ui.allocate_rect(rect, Sense::click_and_drag());

    let to_pointer = |pos: Pos2| EditorPointer {
        x: f64::from((pos.x - rect.min.x) / rect.width() * 100.0),
        y: f64::from((pos.y - rect.min.y) / rect.height() * 100.0),
        tolerance_x: f64::from(GRAB / rect.width() * 100.0),
        tolerance_y: f64::from(GRAB / rect.height() * 100.0),
    };

    if response.drag_started_by(PointerButton::Primary)
        && let Some(origin) = ui.input(|i| i.pointer.press_origin())
    {
        editor.pressed(to_pointer(origin));
    }
    if response.dragged_by(PointerButton::Primary)
        && let Some(pos) = response.interact_pointer_pos()
    {
        editor.moved(to_pointer(pos));
    }
    if response.drag_stopped_by(PointerButton::Primary) {
        editor.released();
    }
    if response.clicked()
        && let Some(pos) = response.interact_pointer_pos()
    {
        editor.pressed(to_pointer(pos));
        editor.released();
    }
    if response.secondary_clicked()
        && let Some(pos) = response.interact_pointer_pos()
    {
        view.menu_on_zone = editor.right_clicked(to_pointer(pos));
    }
    response.context_menu(|ui| {
        if !view.menu_on_zone {
            ui.close();
            return;
        }
        if ui.button("Split into left and right here").clicked() {
            editor.split(true);
            ui.close();
        }
        if ui.button("Split into top and bottom here").clicked() {
            editor.split(false);
            ui.close();
        }
        if ui.button("Delete").clicked() {
            editor.delete();
            ui.close();
        }
    });
    if let Some(pos) = response.hover_pos()
        && !response.dragged()
    {
        ui.ctx().set_cursor_icon(match editor.cursor_at(to_pointer(pos)) {
            Cursor::Draw => CursorIcon::Crosshair,
            Cursor::Move => CursorIcon::Move,
            Cursor::ResizeLeftRight => CursorIcon::ResizeHorizontal,
            Cursor::ResizeUpDown => CursorIcon::ResizeVertical,
            Cursor::ResizeDiagonalDown => CursorIcon::ResizeNwSe,
            Cursor::ResizeDiagonalUp => CursorIcon::ResizeNeSw,
        });
    }

    paint(ui, editor, rect, scale);
}

fn paint(ui: &egui::Ui, editor: &Presenter<impl Store>, rect: Rect, scale: f32) {
    let painter = ui.painter_at(rect.expand(HANDLE));
    painter.rect_filled(rect, CornerRadius::ZERO, MONITOR);
    // Half the gap inside the work area, and half around each zone, as on the screen.
    let half = editor.gap() as f32 / 2.0 * scale;
    let area = rect.shrink(half);
    let at = |x: f64, y: f64| pos2(area.min.x + x as f32 / 100.0 * area.width(), area.min.y + y as f32 / 100.0 * area.height());

    let zones = editor.zones();
    for index in drawing_order(zones.len(), editor.selected()) {
        let zone = zones[index];
        let bounds = Rect::from_min_max(at(zone.left, zone.top), at(zone.right(), zone.bottom())).shrink(half);
        let selected = editor.selected() == Some(index);
        painter.rect(
            bounds,
            CornerRadius::same(3),
            if selected { SELECTED } else { ZONE },
            Stroke::new(1.5, Color32::WHITE),
            StrokeKind::Inside,
        );
        let size = (bounds.width().min(bounds.height()) / 3.0).clamp(10.0, 64.0);
        painter.text(bounds.center(), Align2::CENTER_CENTER, (index + 1).to_string(), FontId::proportional(size), Color32::WHITE);
    }

    // The selected zone's handles, at its corners and the middles of its sides.
    if let Some(selected) = editor.selected() {
        let zone = zones[selected];
        let (middle_x, middle_y) = (zone.left + zone.width / 2.0, zone.top + zone.height / 2.0);
        for (x, y) in [
            (zone.left, zone.top),
            (middle_x, zone.top),
            (zone.right(), zone.top),
            (zone.right(), middle_y),
            (zone.right(), zone.bottom()),
            (middle_x, zone.bottom()),
            (zone.left, zone.bottom()),
            (zone.left, middle_y),
        ] {
            let center = at(x, y);
            painter.rect(
                Rect::from_center_size(center, Vec2::splat(HANDLE)),
                CornerRadius::ZERO,
                Color32::WHITE,
                Stroke::new(1.0, Color32::BLACK),
                StrokeKind::Inside,
            );
        }
    }
}

/// Asks what to do with unsaved changes, when the user wants to leave.
fn ask(ui: &mut egui::Ui, editor: &mut Presenter<impl Store>, actions: &mut Vec<Action>) {
    if editor.asking.is_none() {
        return;
    }
    let mut answer = None;
    let modal = egui::Modal::new(egui::Id::unique("save-changes")).show(ui.ctx(), |ui| {
        ui.heading("Save the changes?");
        ui.label("The window zones have changes that aren't saved.");
        if let Some(problem) = &editor.problem {
            ui.colored_label(PROBLEM, problem);
        }
        ui.horizontal(|ui| {
            if ui.button("Save").clicked() {
                answer = Some(Some(true));
            }
            if ui.button("Don't save").clicked() {
                answer = Some(Some(false));
            }
            if ui.button("Cancel").clicked() {
                answer = Some(None);
            }
        });
    });
    if modal.should_close() && answer.is_none() {
        answer = Some(None);
    }
    if let Some(answer) = answer
        && let Some(why) = editor.answer(answer)
    {
        actions.push(Action::Leave(why));
    }
}
