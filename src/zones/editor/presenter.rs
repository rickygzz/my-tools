//! The window zones editor's behaviour, with no window: the view calls these methods as the
//! user works, and reads back what to draw.
//!
//! - A drag on a zone's border moves that border, and every border on the same line with it,
//!   so a grid stays a grid.
//! - A drag on a handle of the selected zone (its corners and the middles of its sides)
//!   resizes only that zone, even over others.
//! - A drag inside a zone moves it; a drag on no zone draws a new one.
//! - Borders and zones snap to the edges of the monitor and of the other zones.
//! - A right-click on a zone splits it there, or deletes it.
//!
//! It edits the default layout or a monitor's own, which a monitor gets at its first change and
//! keeps until the user picks the default one again. Every change can be undone, in whichever
//! layout it was; nothing is saved until the user saves.
//!
//! Points are in percent of the monitor's work area: (0, 0) is its top left corner, (100, 100)
//! its bottom right.

use crate::zones::geometry::nearest;
use crate::zones::layout::{Layout, Layouts, Monitor, Rect, Template, Zone};

/// The smallest a zone can be, in percent of each side.
pub const MIN_SIZE: f64 = 5.0;

/// The gaps to choose from, in pixels; the layout's own is added when it isn't one of them.
const GAPS: [i32; 12] = [0, 2, 4, 6, 8, 10, 12, 16, 20, 24, 32, 48];

/// Borders this close are on the same line, in percent: templates of thirds add up with
/// rounding errors.
const SAME_LINE: f64 = 0.05;

/// When the system tells of no monitors.
const DEFAULT_MONITOR: Rect = Rect::new(0, 0, 1920, 1040);

/// Where the mouse is on the monitor, in percent, and how near counts as on a border.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EditorPointer {
    pub x: f64,
    pub y: f64,
    /// How far from a border, across, still grabs it, in percent of the width.
    pub tolerance_x: f64,
    /// The same, in percent of the height.
    pub tolerance_y: f64,
}

/// What the mouse would do, shown by its cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cursor {
    /// On no zone: a drag draws a new one.
    Draw,
    /// In a zone: a drag moves it.
    Move,
    ResizeLeftRight,
    ResizeUpDown,
    /// On a top left or bottom right handle.
    ResizeDiagonalDown,
    /// On a top right or bottom left handle.
    ResizeDiagonalUp,
}

/// Where the zones are kept: `zones.cfg`, or a fake in tests.
pub trait Store {
    fn load(&mut self) -> Layouts;
    fn save(&mut self, layouts: &Layouts) -> Result<(), String>;
}

/// Why the user wants to leave, which may first need an answer about unsaved changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Leaving {
    Close,
    /// Open `zones.cfg` in a text editor, and close.
    EditAsText,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Edge {
    Left,
    Top,
    Right,
    Bottom,
}

/// The sides a handle moves: one for the middle of a side, two for a corner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Sides {
    left: bool,
    top: bool,
    right: bool,
    bottom: bool,
}

#[derive(Debug, Clone, Copy)]
enum DragKind {
    Border(usize, Edge),
    Handle(usize, Sides),
    Move(usize),
    Draw,
}

/// Every layout, and the one edited then, to undo to or compare with what's saved.
#[derive(Debug, Clone)]
struct Snapshot {
    layouts: Layouts,
    target: Option<usize>,
    is_own: bool,
    zones: Vec<Zone>,
    gap: i32,
}

/// A drag in progress: what it does, how things were before it, and where it started.
struct Drag {
    kind: DragKind,
    before: Snapshot,
    start: (f64, f64),
}

pub struct Presenter<S: Store> {
    store: S,
    monitors: Vec<Monitor>,
    /// The layouts as they were before the one being edited changed.
    default: Layout,
    own: Vec<(Rect, Layout)>,
    /// The layout being edited: the default one when `None`, else this monitor's.
    target: Option<usize>,
    /// Whether the monitor edited has its own layout; it gets one at its first change.
    is_own: bool,
    zones: Vec<Zone>,
    gap: i32,
    selected: Option<usize>,
    saved: Layouts,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    drag: Option<Drag>,
    /// Where the zone menu was asked for, which is where a split goes.
    menu_point: Option<(f64, f64)>,
    /// Something that went wrong, for the view to show until the next change.
    pub problem: Option<String>,
    /// The user wants to leave with unsaved changes: the view asks whether to save them.
    pub asking: Option<Leaving>,
}

impl<S: Store> Presenter<S> {
    pub fn new(mut store: S, monitors: Vec<Monitor>) -> Self {
        let layouts = store.load();
        let monitors = if monitors.is_empty() {
            vec![Monitor { bounds: DEFAULT_MONITOR, work_area: DEFAULT_MONITOR }]
        } else {
            monitors
        };
        let mut presenter = Self {
            store,
            monitors,
            default: layouts.default,
            own: layouts.monitors,
            target: None,
            is_own: false,
            zones: Vec::new(),
            gap: 0,
            selected: None,
            saved: Layouts::default(),
            undo: Vec::new(),
            redo: Vec::new(),
            drag: None,
            menu_point: None,
            problem: None,
            asking: None,
        };
        presenter.load_target();
        presenter.saved = presenter.build();
        presenter
    }

    // ----- What the view shows -----

    /// The layouts to edit: the default one, then each monitor, marked when it has its own.
    pub fn targets(&self) -> Vec<String> {
        let layouts = self.build();
        let mut names = vec!["Default layout".to_owned()];
        for (index, monitor) in self.monitors.iter().enumerate() {
            let own = if layouts.monitors.iter().any(|(bounds, _)| *bounds == monitor.bounds) { ", own layout" } else { "" };
            names.push(format!("Monitor {} ({} x {}){own}", index + 1, monitor.bounds.width, monitor.bounds.height));
        }
        names
    }

    /// The one being edited, in [`Presenter::targets`].
    pub fn target(&self) -> usize {
        self.target.map_or(0, |index| index + 1)
    }

    /// The size of the work area the zones are drawn on, in pixels: the default layout is shown
    /// on the first monitor's.
    pub fn shape(&self) -> (i32, i32) {
        let area = self.monitors[self.target.unwrap_or(0)].work_area;
        (area.width, area.height)
    }

    /// Whether a monitor's layout is edited, and whether it's its own (or the default one,
    /// until its first change).
    pub fn own_layout(&self) -> (bool, bool) {
        (self.target.is_some(), self.is_own)
    }

    /// The gaps to choose from, with the layout's even when it isn't a usual one.
    pub fn gaps(&self) -> Vec<i32> {
        let mut gaps = GAPS.to_vec();
        if !gaps.contains(&self.gap) {
            gaps.push(self.gap);
            gaps.sort_unstable();
        }
        gaps
    }

    pub fn gap(&self) -> i32 {
        self.gap
    }

    /// In percent of the work area, numbered from 1 in this order.
    pub fn zones(&self) -> &[Zone] {
        &self.zones
    }

    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn unsaved(&self) -> bool {
        !same_layouts(&self.build(), &self.saved)
    }

    /// What a drag from here would do.
    pub fn cursor_at(&self, pointer: EditorPointer) -> Cursor {
        if let Some(sides) = self.handle_at(pointer) {
            return match (sides.left, sides.top, sides.right, sides.bottom) {
                (true, true, ..) | (_, _, true, true) => Cursor::ResizeDiagonalDown,
                (_, true, true, _) | (true, _, _, true) => Cursor::ResizeDiagonalUp,
                (true, ..) | (_, _, true, _) => Cursor::ResizeLeftRight,
                _ => Cursor::ResizeUpDown,
            };
        }
        match self.border_at(pointer) {
            Some((_, Edge::Left | Edge::Right)) => Cursor::ResizeLeftRight,
            Some(_) => Cursor::ResizeUpDown,
            None if self.zone_at(pointer.x, pointer.y).is_some() => Cursor::Move,
            None => Cursor::Draw,
        }
    }

    // ----- The mouse -----

    pub fn pressed(&mut self, pointer: EditorPointer) {
        self.menu_point = None;
        let kind = if let (Some(sides), Some(selected)) = (self.handle_at(pointer), self.selected) {
            DragKind::Handle(selected, sides)
        } else if let Some((index, edge)) = self.border_at(pointer) {
            self.selected = Some(index);
            DragKind::Border(index, edge)
        } else if let Some(zone) = self.zone_at(pointer.x, pointer.y) {
            self.selected = Some(zone);
            DragKind::Move(zone)
        } else {
            self.selected = None;
            DragKind::Draw
        };
        let start = match kind {
            DragKind::Draw => (clamp(pointer.x), clamp(pointer.y)),
            _ => (pointer.x, pointer.y),
        };
        self.drag = Some(Drag { kind, before: self.current(), start });
    }

    pub fn moved(&mut self, pointer: EditorPointer) {
        let Some(drag) = &self.drag else { return };
        let mut zones = drag.before.zones.clone();
        match drag.kind {
            DragKind::Border(index, edge) => move_border(&mut zones, index, edge, pointer),
            DragKind::Handle(index, sides) => zones[index] = resized(&zones, index, sides, pointer),
            DragKind::Move(index) => zones[index] = moved(&zones, index, drag.start, pointer),
            DragKind::Draw => {
                zones.push(drawn(&zones, drag.start, pointer));
                self.selected = Some(zones.len() - 1);
            }
        }
        // The monitor's first change gives it its own layout; one that changes nothing leaves
        // it as it was.
        self.is_own = self.target.is_some() && (drag.before.is_own || zones != drag.before.zones);
        self.zones = zones;
    }

    pub fn released(&mut self) {
        let Some(drag) = self.drag.take() else { return };
        // A zone drawn too small to use (or a click on no zone) adds nothing.
        let too_small = matches!(drag.kind, DragKind::Draw)
            && self.zones.len() > drag.before.zones.len()
            && self.zones.last().is_some_and(|zone| zone.width < MIN_SIZE || zone.height < MIN_SIZE);
        if too_small {
            self.zones = drag.before.zones.clone();
            self.is_own = drag.before.is_own;
            self.selected = None;
        }
        self.remember(drag.before);
    }

    /// Selects the zone under a right click, for its menu. Returns whether there's one.
    pub fn right_clicked(&mut self, pointer: EditorPointer) -> bool {
        let Some(zone) = self.zone_at(pointer.x, pointer.y) else { return false };
        self.selected = Some(zone);
        self.menu_point = Some((pointer.x, pointer.y));
        true
    }

    // ----- Commands -----

    pub fn template(&mut self, template: Template) {
        self.change(|presenter| {
            presenter.zones = template.zones();
            presenter.selected = None;
        });
    }

    pub fn set_gap(&mut self, gap: i32) {
        if gap != self.gap {
            self.change(|presenter| presenter.gap = gap);
        }
    }

    /// `index`: 0 for the default layout, then 1 for the first monitor's, and so on.
    pub fn set_target(&mut self, index: usize) {
        let target = index.checked_sub(1);
        if target == self.target || index > self.monitors.len() {
            return;
        }
        // The layout edited so far is kept as it is, unsaved, while another one is edited.
        let layouts = self.build();
        self.default = layouts.default;
        self.own = layouts.monitors;
        self.target = target;
        self.load_target();
    }

    /// The monitor edited uses the default layout again, instead of its own.
    pub fn use_default(&mut self) {
        if self.target.is_none() || !self.is_own {
            return;
        }
        let before = self.current();
        self.is_own = false;
        self.zones = self.default.zones.clone();
        self.gap = self.default.gap;
        self.selected = None;
        self.undo.push(before);
        self.redo.clear();
    }

    pub fn delete(&mut self) {
        if let Some(index) = self.selected {
            self.change(|presenter| {
                presenter.zones.remove(index);
                presenter.selected = None;
            });
        }
    }

    /// Splits the selected zone in two, left and right (`vertically`) or top and bottom, where
    /// it was right-clicked; near an edge, in the middle.
    pub fn split(&mut self, vertically: bool) {
        let Some(index) = self.selected else { return };
        let zone = self.zones[index];
        let (start, size) = if vertically { (zone.left, zone.width) } else { (zone.top, zone.height) };
        if size < 2.0 * MIN_SIZE {
            self.problem = Some(
                if vertically { "This zone is too narrow to split into two." } else { "This zone is too short to split into two." }
                    .into(),
            );
            return;
        }
        let end = start + size;
        let mut at = self.menu_point.map_or(start + size / 2.0, |(x, y)| if vertically { x } else { y });
        if at - start < MIN_SIZE || end - at < MIN_SIZE {
            at = start + size / 2.0;
        }
        let at = round(at);
        self.change(|presenter| {
            let (first, second) = if vertically {
                (Zone { width: at - start, ..zone }, Zone { left: at, width: end - at, ..zone })
            } else {
                (Zone { height: at - start, ..zone }, Zone { top: at, height: end - at, ..zone })
            };
            presenter.zones[index] = first;
            presenter.zones.insert(index + 1, second);
        });
    }

    pub fn undo(&mut self) {
        if let Some(snapshot) = self.undo.pop() {
            self.redo.push(self.current());
            self.restore(snapshot);
        }
    }

    pub fn redo(&mut self) {
        if let Some(snapshot) = self.redo.pop() {
            self.undo.push(self.current());
            self.restore(snapshot);
        }
    }

    /// Returns whether it saved; when it didn't, [`Presenter::problem`] says why.
    pub fn save(&mut self) -> bool {
        let layouts = self.build();
        match self.store.save(&layouts) {
            Ok(()) => {
                self.saved = layouts;
                self.problem = None;
                true
            }
            Err(problem) => {
                self.problem = Some(problem);
                false
            }
        }
    }

    /// Returns whether the editor can go now: nothing is unsaved. Otherwise it asks first
    /// ([`Presenter::asking`]), and [`Presenter::answer`] says whether to go then.
    pub fn leave(&mut self, why: Leaving) -> bool {
        if self.unsaved() {
            self.asking = Some(why);
            false
        } else {
            true
        }
    }

    /// The user's answer about unsaved changes: save them (`Some(true)`), drop them
    /// (`Some(false)`), or stay (`None`). Returns why to leave now, if it's time to.
    pub fn answer(&mut self, save: Option<bool>) -> Option<Leaving> {
        let why = self.asking.take()?;
        match save {
            Some(true) => self.save().then_some(why),
            Some(false) => Some(why),
            None => None,
        }
    }

    // ----- Inside -----

    /// The layout being edited, from the layouts kept, and whether it's the monitor's own.
    fn load_target(&mut self) {
        let own = self.target.and_then(|index| {
            let bounds = self.monitors[index].bounds;
            self.own.iter().find(|(monitor, _)| *monitor == bounds).map(|(_, layout)| layout.clone())
        });
        self.is_own = own.is_some();
        let layout = own.unwrap_or_else(|| self.default.clone());
        self.zones = layout.zones;
        self.gap = layout.gap;
        self.selected = None;
    }

    /// Every layout, with the one being edited as it is now.
    fn build(&self) -> Layouts {
        let editing = Layout { zones: self.zones.clone(), gap: self.gap };
        let mut own = self.own.clone();
        if let Some(index) = self.target {
            let bounds = self.monitors[index].bounds;
            own.retain(|(monitor, _)| *monitor != bounds);
            if self.is_own {
                own.push((bounds, editing.clone()));
            }
        }
        own.sort_by_key(|(bounds, _)| (bounds.left, bounds.top));
        Layouts { default: if self.target.is_none() { editing } else { self.default.clone() }, monitors: own }
    }

    fn current(&self) -> Snapshot {
        Snapshot { layouts: self.build(), target: self.target, is_own: self.is_own, zones: self.zones.clone(), gap: self.gap }
    }

    /// Makes a change that can be undone.
    fn change(&mut self, change: impl FnOnce(&mut Self)) {
        let before = self.current();
        change(self);
        self.is_own = self.target.is_some() && (before.is_own || self.gap != before.gap || self.zones != before.zones);
        self.remember(before);
    }

    /// Keeps `before` to undo to, if anything changed since.
    fn remember(&mut self, before: Snapshot) {
        if !same_layouts(&self.build(), &before.layouts) {
            self.undo.push(before);
            self.redo.clear();
            self.problem = None;
        }
    }

    /// Back to the layout the snapshot was taken in, so the user sees what was undone.
    fn restore(&mut self, snapshot: Snapshot) {
        self.default = snapshot.layouts.default;
        self.own = snapshot.layouts.monitors;
        self.target = snapshot.target;
        self.is_own = snapshot.is_own;
        self.zones = snapshot.zones;
        self.gap = snapshot.gap;
        self.selected = None;
    }

    /// The handle of the selected zone under the pointer, the nearest one.
    fn handle_at(&self, pointer: EditorPointer) -> Option<Sides> {
        let zone = self.zones[self.selected?];
        let (middle_x, middle_y) = (zone.left + zone.width / 2.0, zone.top + zone.height / 2.0);
        let sides = |left, top, right, bottom| Sides { left, top, right, bottom };
        let handles = [
            (sides(true, true, false, false), zone.left, zone.top),
            (sides(false, true, false, false), middle_x, zone.top),
            (sides(false, true, true, false), zone.right(), zone.top),
            (sides(false, false, true, false), zone.right(), middle_y),
            (sides(false, false, true, true), zone.right(), zone.bottom()),
            (sides(false, false, false, true), middle_x, zone.bottom()),
            (sides(true, false, false, true), zone.left, zone.bottom()),
            (sides(true, false, false, false), zone.left, middle_y),
        ];
        let mut nearest = None;
        let mut best = f64::MAX;
        for (sides, x, y) in handles {
            // In units of the tolerance, so it's the same on both axes.
            let dx = (pointer.x - x).abs() / pointer.tolerance_x;
            let dy = (pointer.y - y).abs() / pointer.tolerance_y;
            if dx <= 1.0 && dy <= 1.0 && dx + dy < best {
                nearest = Some(sides);
                best = dx + dy;
            }
        }
        nearest
    }

    /// The border nearest the pointer, of the selected zone first, then of the others from the
    /// top one down.
    fn border_at(&self, pointer: EditorPointer) -> Option<(usize, Edge)> {
        let mut order: Vec<usize> = (0..self.zones.len()).rev().collect();
        if let Some(selected) = self.selected {
            order.retain(|&index| index != selected);
            order.insert(0, selected);
        }
        let (px, py, tx, ty) = (pointer.x, pointer.y, pointer.tolerance_x, pointer.tolerance_y);
        order.into_iter().find_map(|index| {
            let zone = self.zones[index];
            let across = py >= zone.top - ty && py <= zone.bottom() + ty;
            let along = px >= zone.left - tx && px <= zone.right() + tx;
            [
                (Edge::Left, (px - zone.left).abs(), tx, across),
                (Edge::Right, (px - zone.right()).abs(), tx, across),
                (Edge::Top, (py - zone.top).abs(), ty, along),
                (Edge::Bottom, (py - zone.bottom()).abs(), ty, along),
            ]
            .into_iter()
            .filter(|&(_, distance, tolerance, in_range)| in_range && distance <= tolerance)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(edge, ..)| (index, edge))
        })
    }

    /// The zone the point is in whose center is nearest, so a zone over another can be
    /// reached.
    fn zone_at(&self, x: f64, y: f64) -> Option<usize> {
        let indexes: Vec<usize> = (0..self.zones.len())
            .filter(|&index| {
                let zone = self.zones[index];
                x >= zone.left && x <= zone.right() && y >= zone.top && y <= zone.bottom()
            })
            .collect();
        let found: Vec<[f64; 4]> = indexes
            .iter()
            .map(|&index| {
                let zone = self.zones[index];
                [zone.left, zone.top, zone.width, zone.height]
            })
            .collect();
        nearest(&found, x, y).map(|index| indexes[index])
    }
}

/// Whether both have the same zones and gaps, for the same monitors, in any order.
fn same_layouts(one: &Layouts, other: &Layouts) -> bool {
    one.default == other.default
        && one.monitors.len() == other.monitors.len()
        && one.monitors.iter().all(|monitor| other.monitors.contains(monitor))
}

fn edge_of(zone: &Zone, edge: Edge) -> f64 {
    match edge {
        Edge::Left => zone.left,
        Edge::Top => zone.top,
        Edge::Right => zone.right(),
        Edge::Bottom => zone.bottom(),
    }
}

fn with_edge(zone: Zone, edge: Edge, at: f64) -> Zone {
    match edge {
        Edge::Left => Zone { left: at, width: zone.right() - at, ..zone },
        Edge::Top => Zone { top: at, height: zone.bottom() - at, ..zone },
        Edge::Right => Zone { width: at - zone.left, ..zone },
        Edge::Bottom => Zone { height: at - zone.top, ..zone },
    }
}

/// Every border on the line of the one grabbed moves with it, within what keeps each zone
/// large enough.
fn move_border(zones: &mut [Zone], index: usize, edge: Edge, pointer: EditorPointer) {
    let across_x = matches!(edge, Edge::Left | Edge::Right);
    let line = edge_of(&zones[index], edge);
    let edges = if across_x { [Edge::Left, Edge::Right] } else { [Edge::Top, Edge::Bottom] };
    let moving: Vec<(usize, Edge)> = (0..zones.len())
        .flat_map(|index| edges.map(|edge| (index, edge)))
        .filter(|&(index, edge)| (edge_of(&zones[index], edge) - line).abs() < SAME_LINE)
        .collect();

    let (mut low, mut high) = (0.0_f64, 100.0_f64);
    for &(index, edge) in &moving {
        let zone = &zones[index];
        if matches!(edge, Edge::Left | Edge::Top) {
            high = high.min(edge_of(zone, edges[1]) - MIN_SIZE);
        } else {
            low = low.max(edge_of(zone, edges[0]) + MIN_SIZE);
        }
    }
    if low > high {
        return;
    }
    let others: Vec<f64> = (0..zones.len())
        .filter(|index| !moving.iter().any(|(moving, _)| moving == index))
        .flat_map(|index| edges.map(|edge| edge_of(&zones[index], edge)))
        .collect();
    let (value, tolerance) = if across_x { (pointer.x, pointer.tolerance_x) } else { (pointer.y, pointer.tolerance_y) };
    let to = round(snap(value, &others, tolerance).clamp(low, high));
    for (index, edge) in moving {
        zones[index] = with_edge(zones[index], edge, to);
    }
}

/// The edges of the zones but one, across x or y, to snap to.
fn other_edges(zones: &[Zone], except: Option<usize>, across_x: bool) -> Vec<f64> {
    zones
        .iter()
        .enumerate()
        .filter(|(index, _)| Some(*index) != except)
        .flat_map(|(_, zone)| if across_x { [zone.left, zone.right()] } else { [zone.top, zone.bottom()] })
        .collect()
}

/// Only this zone's sides move, so it can go over the zones next to it.
fn resized(zones: &[Zone], index: usize, sides: Sides, pointer: EditorPointer) -> Zone {
    let zone = zones[index];
    let x = snap(clamp(pointer.x), &other_edges(zones, Some(index), true), pointer.tolerance_x);
    let y = snap(clamp(pointer.y), &other_edges(zones, Some(index), false), pointer.tolerance_y);
    let (mut left, mut top, mut right, mut bottom) = (zone.left, zone.top, zone.right(), zone.bottom());
    if sides.left {
        left = x.min(right - MIN_SIZE);
    }
    if sides.right {
        right = x.max(left + MIN_SIZE);
    }
    if sides.top {
        top = y.min(bottom - MIN_SIZE);
    }
    if sides.bottom {
        bottom = y.max(top + MIN_SIZE);
    }
    let (left, top) = (round(left), round(top));
    Zone::new(left, top, round(right) - left, round(bottom) - top)
}

fn moved(zones: &[Zone], index: usize, start: (f64, f64), pointer: EditorPointer) -> Zone {
    let zone = zones[index];
    let left = (zone.left + pointer.x - start.0).clamp(0.0, 100.0 - zone.width);
    let top = (zone.top + pointer.y - start.1).clamp(0.0, 100.0 - zone.height);
    let left = snap_span(left, zone.width, &other_edges(zones, Some(index), true), pointer.tolerance_x);
    let top = snap_span(top, zone.height, &other_edges(zones, Some(index), false), pointer.tolerance_y);
    Zone { left: round(left), top: round(top), ..zone }
}

fn drawn(zones: &[Zone], start: (f64, f64), pointer: EditorPointer) -> Zone {
    let x = snap(clamp(pointer.x), &other_edges(zones, None, true), pointer.tolerance_x);
    let y = snap(clamp(pointer.y), &other_edges(zones, None, false), pointer.tolerance_y);
    let (left, top) = (round(x.min(start.0)), round(y.min(start.1)));
    Zone::new(left, top, round(x.max(start.0)) - left, round(y.max(start.1)) - top)
}

/// To the nearest of the edges, or of the monitor's, when one is near enough.
fn snap(value: f64, edges: &[f64], tolerance: f64) -> f64 {
    let (mut best, mut distance) = (value, tolerance);
    for &edge in edges.iter().chain(&[0.0, 100.0]) {
        if (edge - value).abs() <= distance {
            best = edge;
            distance = (edge - value).abs();
        }
    }
    best
}

/// A zone's start, so that its start or its end is on the nearest edge, when one is near
/// enough.
fn snap_span(start: f64, size: f64, edges: &[f64], tolerance: f64) -> f64 {
    let (mut best, mut distance) = (start, tolerance);
    for &edge in edges.iter().chain(&[0.0, 100.0]) {
        for candidate in [edge, edge - size] {
            if (candidate - start).abs() <= distance && candidate >= 0.0 && candidate + size <= 100.0 {
                best = candidate;
                distance = (candidate - start).abs();
            }
        }
    }
    best
}

fn clamp(percent: f64) -> f64 {
    percent.clamp(0.0, 100.0)
}

/// To hundredths of a percent, less than a pixel, as the zones are kept.
fn round(percent: f64) -> f64 {
    (percent * 100.0).round_ties_even() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zones::desktop::Desktop;
    use crate::zones::desktop::testing::{FakeDesktop, LEFT, RIGHT};

    fn zone(left: f64, top: f64, width: f64, height: f64) -> Zone {
        Zone::new(left, top, width, height)
    }

    #[derive(Default)]
    struct FakeStore {
        layouts: Layouts,
        saved: Vec<Layouts>,
        problem: Option<String>,
    }

    impl Store for &mut FakeStore {
        fn load(&mut self) -> Layouts {
            self.layouts.clone()
        }

        fn save(&mut self, layouts: &Layouts) -> Result<(), String> {
            if let Some(problem) = &self.problem {
                return Err(problem.clone());
            }
            self.saved.push(layouts.clone());
            Ok(())
        }
    }

    fn pointer(x: f64, y: f64) -> EditorPointer {
        EditorPointer { x, y, tolerance_x: 1.0, tolerance_y: 1.0 }
    }

    /// An editor on the fake desktop's two monitors of 1000 x 600, with these zones as the
    /// default layout and a gap of 8.
    fn start<'a>(store: &'a mut FakeStore, zones: &[Zone]) -> Presenter<&'a mut FakeStore> {
        store.layouts.default = Layout { zones: zones.to_vec(), gap: 8 };
        Presenter::new(store, FakeDesktop::default().monitors())
    }

    fn two_columns(store: &mut FakeStore) -> Presenter<&mut FakeStore> {
        start(store, &[zone(0.0, 0.0, 50.0, 100.0), zone(50.0, 0.0, 50.0, 100.0)])
    }

    trait Mouse {
        fn drag(&mut self, from: (f64, f64), to: (f64, f64));
        fn click(&mut self, x: f64, y: f64) {
            self.drag((x, y), (x, y));
        }
    }

    impl<S: Store> Mouse for Presenter<S> {
        fn drag(&mut self, from: (f64, f64), to: (f64, f64)) {
            self.pressed(pointer(from.0, from.1));
            self.moved(pointer(to.0, to.1));
            self.released();
        }
    }

    #[test]
    fn it_starts_with_the_saved_zones_the_monitors_and_the_gap() {
        let mut store = FakeStore::default();
        let editor = two_columns(&mut store);
        assert_eq!(editor.zones(), [zone(0.0, 0.0, 50.0, 100.0), zone(50.0, 0.0, 50.0, 100.0)]);
        assert_eq!(editor.targets(), ["Default layout", "Monitor 1 (1000 x 600)", "Monitor 2 (1000 x 600)"]);
        assert_eq!((editor.target(), editor.shape(), editor.own_layout()), (0, (1000, 600), (false, false)));
        assert_eq!(editor.gap(), 8);
        assert!(editor.gaps().contains(&8));
        assert!(!editor.unsaved() && !editor.can_undo() && !editor.can_redo());
    }

    #[test]
    fn a_gap_that_isnt_a_choice_is_added() {
        let layouts = Layouts { default: Layout { zones: vec![zone(0.0, 0.0, 100.0, 100.0)], gap: 7 }, monitors: vec![] };
        let mut store = FakeStore { layouts, ..FakeStore::default() };
        let editor = Presenter::new(&mut store, FakeDesktop::default().monitors());
        assert!(editor.gaps().contains(&7));
        assert_eq!(editor.gap(), 7);
    }

    #[test]
    fn dragging_the_border_between_columns_moves_both_and_can_be_undone() {
        let mut store = FakeStore::default();
        let mut editor = two_columns(&mut store);
        editor.drag((50.0, 50.0), (70.0, 50.0));
        assert_eq!(editor.zones(), [zone(0.0, 0.0, 70.0, 100.0), zone(70.0, 0.0, 30.0, 100.0)]);
        assert!(editor.unsaved());
        editor.undo();
        assert_eq!(editor.zones(), [zone(0.0, 0.0, 50.0, 100.0), zone(50.0, 0.0, 50.0, 100.0)]);
        assert!(!editor.unsaved());
        assert_eq!((editor.can_undo(), editor.can_redo()), (false, true));
    }

    #[test]
    fn dragging_a_border_of_a_grid_moves_the_whole_line() {
        let mut store = FakeStore::default();
        let mut editor = start(
            &mut store,
            &[zone(0.0, 0.0, 50.0, 50.0), zone(50.0, 0.0, 50.0, 50.0), zone(0.0, 50.0, 50.0, 50.0), zone(50.0, 50.0, 50.0, 50.0)],
        );
        editor.drag((50.0, 20.0), (40.0, 20.0));
        assert_eq!(
            editor.zones(),
            [zone(0.0, 0.0, 40.0, 50.0), zone(40.0, 0.0, 60.0, 50.0), zone(0.0, 50.0, 40.0, 50.0), zone(40.0, 50.0, 60.0, 50.0)]
        );
    }

    #[test]
    fn a_border_stops_where_a_zone_would_be_too_small() {
        let mut store = FakeStore::default();
        let mut editor = two_columns(&mut store);
        editor.drag((50.0, 50.0), (99.0, 50.0));
        assert_eq!(editor.zones(), [zone(0.0, 0.0, 95.0, 100.0), zone(95.0, 0.0, 5.0, 100.0)]);
    }

    #[test]
    fn a_border_snaps_to_the_edge_of_another_zone_near_it() {
        let mut store = FakeStore::default();
        let mut editor = start(&mut store, &[zone(0.0, 0.0, 50.0, 50.0), zone(0.0, 50.0, 60.0, 50.0)]);
        editor.drag((50.0, 20.0), (59.5, 20.0));
        assert_eq!(editor.zones()[0], zone(0.0, 0.0, 60.0, 50.0));
    }

    #[test]
    fn a_corner_handle_resizes_only_its_zone_over_the_one_next_to_it() {
        let mut store = FakeStore::default();
        let mut editor = two_columns(&mut store);
        editor.click(20.0, 50.0);
        editor.drag((50.0, 100.0), (70.0, 80.0));
        assert_eq!(editor.zones(), [zone(0.0, 0.0, 70.0, 80.0), zone(50.0, 0.0, 50.0, 100.0)]);
        editor.undo();
        assert_eq!(editor.zones(), [zone(0.0, 0.0, 50.0, 100.0), zone(50.0, 0.0, 50.0, 100.0)]);
    }

    #[test]
    fn a_handle_in_the_middle_of_a_side_moves_only_that_side() {
        let mut store = FakeStore::default();
        let mut editor = two_columns(&mut store);
        editor.click(70.0, 50.0);
        editor.drag((75.0, 0.0), (90.0, 30.0));
        assert_eq!(editor.zones(), [zone(0.0, 0.0, 50.0, 100.0), zone(50.0, 30.0, 50.0, 70.0)]);
    }

    #[test]
    fn a_handle_stops_where_the_zone_would_be_too_small() {
        let mut store = FakeStore::default();
        let mut editor = two_columns(&mut store);
        editor.click(20.0, 50.0);
        editor.drag((50.0, 50.0), (1.0, 50.0));
        assert_eq!(editor.zones()[0], zone(0.0, 0.0, 5.0, 100.0));
    }

    #[test]
    fn a_handle_snaps_to_the_edge_of_another_zone_near_it() {
        let mut store = FakeStore::default();
        let mut editor = start(&mut store, &[zone(0.0, 0.0, 40.0, 100.0), zone(60.0, 0.0, 40.0, 100.0)]);
        editor.click(20.0, 50.0);
        editor.drag((40.0, 50.0), (59.5, 50.0));
        assert_eq!(editor.zones()[0], zone(0.0, 0.0, 60.0, 100.0));
    }

    #[test]
    fn handles_are_only_on_the_selected_zone() {
        let mut store = FakeStore::default();
        let mut editor = two_columns(&mut store);
        assert_ne!(editor.cursor_at(pointer(0.5, 0.5)), Cursor::ResizeDiagonalDown);
        editor.click(20.0, 50.0);
        assert_eq!(editor.cursor_at(pointer(0.5, 0.5)), Cursor::ResizeDiagonalDown);
        assert_eq!(editor.cursor_at(pointer(49.5, 0.5)), Cursor::ResizeDiagonalUp);
        assert_eq!(editor.cursor_at(pointer(25.0, 99.5)), Cursor::ResizeUpDown);
    }

    #[test]
    fn a_click_where_zones_overlap_selects_the_one_whose_center_is_nearest() {
        for (x, y, expected) in [(20.0, 20.0, 0), (50.0, 50.0, 1)] {
            let mut store = FakeStore::default();
            let mut editor = start(&mut store, &[zone(0.0, 0.0, 100.0, 100.0), zone(30.0, 30.0, 40.0, 40.0)]);
            editor.click(x, y);
            assert_eq!(editor.selected(), Some(expected));
        }
    }

    #[test]
    fn a_monitor_without_its_own_layout_shows_the_default_until_its_first_change() {
        let mut store = FakeStore::default();
        let mut editor = two_columns(&mut store);
        editor.set_target(2);
        assert_eq!(editor.zones(), [zone(0.0, 0.0, 50.0, 100.0), zone(50.0, 0.0, 50.0, 100.0)]);
        assert_eq!(editor.own_layout(), (true, false));
        assert!(!editor.unsaved());
        editor.template(Template::Rows2);
        assert_eq!(editor.own_layout(), (true, true));
        assert_eq!(editor.targets()[2], "Monitor 2 (1000 x 600), own layout");
        assert!(editor.unsaved());
    }

    #[test]
    fn a_click_on_a_monitor_without_its_own_layout_doesnt_give_it_one() {
        let mut store = FakeStore::default();
        let mut editor = two_columns(&mut store);
        editor.set_target(1);
        editor.click(20.0, 50.0);
        assert_eq!(editor.own_layout(), (true, false));
        assert!(!editor.unsaved());
    }

    #[test]
    fn saving_keeps_the_default_layout_and_the_monitors_own() {
        let mut store = FakeStore::default();
        let mut editor = two_columns(&mut store);
        editor.set_target(2);
        editor.template(Template::Rows2);
        editor.set_gap(16);
        assert!(editor.save());
        drop(editor);
        let [saved] = &store.saved[..] else { panic!("saved {} times", store.saved.len()) };
        assert_eq!(saved.default, Layout { zones: vec![zone(0.0, 0.0, 50.0, 100.0), zone(50.0, 0.0, 50.0, 100.0)], gap: 8 });
        assert_eq!(saved.monitors, [(RIGHT, Layout { zones: vec![zone(0.0, 0.0, 100.0, 50.0), zone(0.0, 50.0, 100.0, 50.0)], gap: 16 })]);
    }

    #[test]
    fn changes_to_one_layout_are_kept_while_another_is_edited() {
        let mut store = FakeStore::default();
        let mut editor = two_columns(&mut store);
        editor.set_target(1);
        editor.template(Template::Rows2);
        editor.set_target(0);
        editor.template(Template::Columns3);
        editor.set_target(1);
        assert_eq!(editor.zones(), [zone(0.0, 0.0, 100.0, 50.0), zone(0.0, 50.0, 100.0, 50.0)]);
        // The other monitor uses the default layout as changed.
        editor.set_target(2);
        assert_eq!(editor.zones().len(), 3);
    }

    #[test]
    fn undo_goes_back_to_the_layout_the_change_was_in() {
        let mut store = FakeStore::default();
        let mut editor = two_columns(&mut store);
        editor.set_target(2);
        editor.template(Template::Rows2);
        editor.set_target(0);
        editor.undo();
        assert_eq!(editor.target(), 2);
        assert_eq!(editor.own_layout(), (true, false));
        assert!(!editor.unsaved());
    }

    #[test]
    fn use_default_drops_the_monitors_own_layout() {
        let mut store = FakeStore { layouts: Layouts {
            default: Layout { zones: vec![zone(0.0, 0.0, 50.0, 100.0), zone(50.0, 0.0, 50.0, 100.0)], gap: 8 },
            monitors: vec![(LEFT, Layout { zones: vec![zone(0.0, 0.0, 100.0, 100.0)], gap: 0 })],
        }, ..FakeStore::default() };
        let mut editor = Presenter::new(&mut store, FakeDesktop::default().monitors());
        editor.set_target(1);
        assert_eq!(editor.own_layout(), (true, true));
        assert_eq!(editor.zones(), [zone(0.0, 0.0, 100.0, 100.0)]);
        editor.use_default();
        editor.save();
        assert_eq!(editor.own_layout(), (true, false));
        assert_eq!(editor.zones(), [zone(0.0, 0.0, 50.0, 100.0), zone(50.0, 0.0, 50.0, 100.0)]);
        drop(editor);
        assert!(store.saved[0].monitors.is_empty());
    }

    #[test]
    fn saving_keeps_the_layout_of_a_monitor_not_connected() {
        let away = (Rect::new(-1920, 0, 1920, 1080), Layout { zones: vec![zone(0.0, 0.0, 100.0, 100.0)], gap: 0 });
        let mut store = FakeStore { layouts: Layouts { default: Layout::default(), monitors: vec![away.clone()] }, ..FakeStore::default() };
        let mut editor = Presenter::new(&mut store, FakeDesktop::default().monitors());
        editor.template(Template::Rows2);
        editor.save();
        drop(editor);
        assert_eq!(store.saved[0].monitors, [away]);
    }

    #[test]
    fn dragging_inside_a_zone_moves_it_within_the_monitor() {
        let mut store = FakeStore::default();
        let mut editor = start(&mut store, &[zone(10.0, 10.0, 30.0, 30.0)]);
        editor.drag((20.0, 20.0), (90.0, 35.0));
        assert_eq!(editor.zones()[0], zone(70.0, 25.0, 30.0, 30.0));
        assert_eq!(editor.selected(), Some(0));
    }

    #[test]
    fn dragging_on_no_zone_draws_a_new_one_selected() {
        let mut store = FakeStore::default();
        let mut editor = start(&mut store, &[zone(0.0, 0.0, 50.0, 50.0)]);
        editor.drag((60.0, 60.0), (80.0, 90.0));
        assert_eq!(editor.zones()[1], zone(60.0, 60.0, 20.0, 30.0));
        assert_eq!(editor.selected(), Some(1));
        assert_eq!((editor.can_undo(), editor.can_redo()), (true, false));
    }

    #[test]
    fn a_click_on_no_zone_draws_nothing_and_selects_nothing() {
        let mut store = FakeStore::default();
        let mut editor = start(&mut store, &[zone(0.0, 0.0, 50.0, 50.0)]);
        editor.drag((20.0, 20.0), (20.0, 20.0));
        editor.drag((70.0, 70.0), (71.0, 71.0));
        assert_eq!(editor.zones().len(), 1);
        assert_eq!(editor.selected(), None);
        assert!(!editor.can_undo() && !editor.can_redo());
    }

    #[test]
    fn splitting_left_and_right_splits_where_it_was_right_clicked() {
        let mut store = FakeStore::default();
        let mut editor = two_columns(&mut store);
        assert!(editor.right_clicked(pointer(20.0, 50.0)));
        editor.split(true);
        assert_eq!(editor.zones(), [zone(0.0, 0.0, 20.0, 100.0), zone(20.0, 0.0, 30.0, 100.0), zone(50.0, 0.0, 50.0, 100.0)]);
    }

    #[test]
    fn splitting_top_and_bottom_near_an_edge_splits_in_the_middle() {
        let mut store = FakeStore::default();
        let mut editor = two_columns(&mut store);
        editor.right_clicked(pointer(70.0, 2.0));
        editor.split(false);
        assert_eq!(editor.zones(), [zone(0.0, 0.0, 50.0, 100.0), zone(50.0, 0.0, 50.0, 50.0), zone(50.0, 50.0, 50.0, 50.0)]);
    }

    #[test]
    fn splitting_a_zone_too_narrow_says_so() {
        let mut store = FakeStore::default();
        let mut editor = start(&mut store, &[zone(0.0, 0.0, 8.0, 100.0), zone(8.0, 0.0, 92.0, 100.0)]);
        editor.right_clicked(pointer(4.0, 50.0));
        editor.split(true);
        assert_eq!(editor.zones().len(), 2);
        assert_eq!(editor.problem.as_deref(), Some("This zone is too narrow to split into two."));
    }

    #[test]
    fn a_right_click_on_no_zone_opens_no_menu() {
        let mut store = FakeStore::default();
        let mut editor = start(&mut store, &[zone(0.0, 0.0, 50.0, 50.0)]);
        assert!(!editor.right_clicked(pointer(80.0, 80.0)));
    }

    #[test]
    fn delete_removes_the_selected_zone() {
        let mut store = FakeStore::default();
        let mut editor = two_columns(&mut store);
        editor.right_clicked(pointer(70.0, 50.0));
        editor.delete();
        assert_eq!(editor.zones(), [zone(0.0, 0.0, 50.0, 100.0)]);
        assert_eq!(editor.selected(), None);
    }

    #[test]
    fn a_template_replaces_the_zones() {
        let mut store = FakeStore::default();
        let mut editor = two_columns(&mut store);
        editor.template(Template::Grid2x2);
        assert_eq!(
            editor.zones(),
            [zone(0.0, 0.0, 50.0, 50.0), zone(50.0, 0.0, 50.0, 50.0), zone(0.0, 50.0, 50.0, 50.0), zone(50.0, 50.0, 50.0, 50.0)]
        );
        assert!(editor.unsaved());
    }

    #[test]
    fn a_gap_is_saved() {
        let mut store = FakeStore::default();
        let mut editor = two_columns(&mut store);
        editor.set_gap(16);
        editor.save();
        assert_eq!(editor.gap(), 16);
        assert!(!editor.unsaved());
        drop(editor);
        assert_eq!(store.saved[0].default.gap, 16);
    }

    #[test]
    fn saving_that_fails_says_why_and_stays_unsaved() {
        let mut store = FakeStore { problem: Some("The file is read-only.".into()), ..FakeStore::default() };
        let mut editor = two_columns(&mut store);
        editor.template(Template::Rows2);
        assert!(!editor.save());
        assert_eq!(editor.problem.as_deref(), Some("The file is read-only."));
        assert!(editor.unsaved());
    }

    #[test]
    fn closing_with_changes_asks_what_to_do_with_them() {
        for (answer, leaves, saves) in [(None, false, 0), (Some(false), true, 0), (Some(true), true, 1)] {
            let mut store = FakeStore::default();
            let mut editor = two_columns(&mut store);
            editor.template(Template::Rows2);
            assert!(!editor.leave(Leaving::Close));
            assert_eq!(editor.asking, Some(Leaving::Close));
            assert_eq!(editor.answer(answer), leaves.then_some(Leaving::Close), "{answer:?}");
            assert_eq!(editor.asking, None);
            drop(editor);
            assert_eq!(store.saved.len(), saves);
        }
    }

    #[test]
    fn closing_without_changes_asks_nothing() {
        let mut store = FakeStore::default();
        let mut editor = two_columns(&mut store);
        assert!(editor.leave(Leaving::EditAsText));
        assert_eq!(editor.asking, None);
    }

    #[test]
    fn hovering_shows_what_a_drag_would_do() {
        let mut store = FakeStore::default();
        let editor = two_columns(&mut store);
        assert_eq!(editor.cursor_at(pointer(50.0, 50.0)), Cursor::ResizeLeftRight);
        assert_eq!(editor.cursor_at(pointer(20.0, 0.5)), Cursor::ResizeUpDown);
        assert_eq!(editor.cursor_at(pointer(20.0, 50.0)), Cursor::Move);
        let mut store = FakeStore::default();
        let editor = start(&mut store, &[zone(0.0, 0.0, 50.0, 50.0)]);
        assert_eq!(editor.cursor_at(pointer(80.0, 80.0)), Cursor::Draw);
    }
}
