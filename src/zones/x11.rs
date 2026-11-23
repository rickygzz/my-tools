//! The monitors and windows on Linux **X11**, through the window manager's EWMH properties
//! and messages (the standard every desktop's window manager speaks), and RandR for the
//! monitors. Wayland doesn't let one app move another's windows, so there's nothing there.
//!
//! Apps don't move other apps' windows themselves here: they ask the window manager, with a
//! `_NET_MOVERESIZE_WINDOW` message, as a pager or taskbar would.
//!
//! What the user sees of a window is its frame: the client window plus the title bar and
//! borders the window manager draws (`_NET_FRAME_EXTENTS`), or, for apps that draw their own
//! title bar (GTK's "client-side decorations"), the client window without the shadow around it
//! (`_GTK_FRAME_EXTENTS`).

use x11rb::connection::Connection as _;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::xproto::{AtomEnum, ClientMessageEvent, ConnectionExt as _, EventMask, Window};
use x11rb::rust_connection::RustConnection;

use super::desktop::{Desktop, nearest_monitor};
use super::layout::{Monitor, Rect};

x11rb::atom_manager! {
    Atoms: AtomsCookie {
        _NET_ACTIVE_WINDOW,
        _NET_WORKAREA,
        _NET_FRAME_EXTENTS,
        _GTK_FRAME_EXTENTS,
        _NET_MOVERESIZE_WINDOW,
        _NET_WM_STATE,
        _NET_WM_STATE_MAXIMIZED_VERT,
        _NET_WM_STATE_MAXIMIZED_HORZ,
        _NET_WM_WINDOW_TYPE,
        _NET_WM_WINDOW_TYPE_NORMAL,
        _NET_WM_WINDOW_TYPE_DIALOG,
    }
}

pub struct SystemDesktop {
    pub(super) conn: RustConnection,
    pub(super) root: Window,
    atoms: Atoms,
}

impl SystemDesktop {
    pub fn new() -> Result<Self, String> {
        let (conn, screen) = x11rb::connect(None).map_err(|error| format!("can't reach the X server: {error}"))?;
        let root = conn.setup().roots[screen].root;
        let atoms = Atoms::new(&conn)
            .map_err(|error| error.to_string())?
            .reply()
            .map_err(|error| error.to_string())?;
        Ok(Self { conn, root, atoms })
    }

    /// A property of 32-bit values; empty when the window doesn't have it.
    fn cardinals(&self, window: Window, property: u32, kind: impl Into<u32>) -> Vec<u32> {
        let reply = self
            .conn
            .get_property(false, window, property, kind.into(), 0, 1024)
            .ok()
            .and_then(|cookie| cookie.reply().ok());
        reply.and_then(|reply| reply.value32().map(Iterator::collect)).unwrap_or_default()
    }

    /// Left, right, top and bottom.
    fn extents(&self, window: Window, property: u32) -> [i32; 4] {
        match self.cardinals(window, property, AtomEnum::CARDINAL)[..] {
            [left, right, top, bottom] => [left, right, top, bottom].map(|side| side as i32),
            _ => [0; 4],
        }
    }

    /// The client window, on the screen.
    fn client_bounds(&self, window: Window) -> Option<Rect> {
        let geometry = self.conn.get_geometry(window).ok()?.reply().ok()?;
        let origin = self.conn.translate_coordinates(window, self.root, 0, 0).ok()?.reply().ok()?;
        Some(Rect::new(
            i32::from(origin.dst_x),
            i32::from(origin.dst_y),
            i32::from(geometry.width),
            i32::from(geometry.height),
        ))
    }

    /// The desktop's work area, without the panels; one for all monitors.
    fn work_area(&self) -> Option<Rect> {
        match self.cardinals(self.root, self.atoms._NET_WORKAREA, AtomEnum::CARDINAL)[..] {
            [left, top, width, height, ..] => Some(Rect::new(left as i32, top as i32, width as i32, height as i32)),
            _ => None,
        }
    }

    /// Asks the window manager to do something to the window, as a pager would.
    fn send(&self, window: Window, message: u32, data: [u32; 5]) -> Result<(), String> {
        let event = ClientMessageEvent::new(32, window, message, data);
        let mask = EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY;
        self.conn.send_event(false, self.root, mask, event).map_err(|error| error.to_string())?;
        self.conn.flush().map_err(|error| error.to_string())
    }
}

/// `_NET_WM_STATE` actions.
const REMOVE: u32 = 0;
const ADD: u32 = 1;
/// "The request comes from a pager", which window managers obey without second thoughts.
const FROM_PAGER: u32 = 2;

impl Desktop for SystemDesktop {
    type Window = Window;

    fn monitors(&self) -> Vec<Monitor> {
        let Some(reply) = self.conn.randr_get_monitors(self.root, true).ok().and_then(|cookie| cookie.reply().ok())
        else {
            return Vec::new();
        };
        let work_area = self.work_area();
        reply
            .monitors
            .iter()
            .map(|monitor| {
                let bounds =
                    Rect::new(monitor.x.into(), monitor.y.into(), monitor.width.into(), monitor.height.into());
                // EWMH has one work area for the whole desktop; its part on this monitor.
                let work_area = work_area.and_then(|area| intersection(area, bounds)).unwrap_or(bounds);
                Monitor { bounds, work_area }
            })
            .collect()
    }

    fn monitor_of(&self, window: Window) -> Monitor {
        nearest_monitor(self.monitors(), self.window_bounds(window))
    }

    fn active_window(&self) -> Option<Window> {
        let window = *self.cardinals(self.root, self.atoms._NET_ACTIVE_WINDOW, AtomEnum::WINDOW).first()?;
        if window == 0 || window == self.root {
            return None;
        }
        // Only windows the user moves around: not the desktop, docks or menus.
        let types = self.cardinals(window, self.atoms._NET_WM_WINDOW_TYPE, AtomEnum::ATOM);
        let movable = [self.atoms._NET_WM_WINDOW_TYPE_NORMAL, self.atoms._NET_WM_WINDOW_TYPE_DIALOG];
        (types.is_empty() || types.iter().any(|kind| movable.contains(kind))).then_some(window)
    }

    fn window_bounds(&self, window: Window) -> Rect {
        let Some(client) = self.client_bounds(window) else { return Rect::default() };
        let [left, right, top, bottom] = self.extents(window, self.atoms._NET_FRAME_EXTENTS);
        let [shadow_left, shadow_right, shadow_top, shadow_bottom] = self.extents(window, self.atoms._GTK_FRAME_EXTENTS);
        Rect::from_edges(
            client.left - left + shadow_left,
            client.top - top + shadow_top,
            client.right() + right - shadow_right,
            client.bottom() + bottom - shadow_bottom,
        )
    }

    fn move_window(&mut self, window: Window, bounds: Rect) -> Result<(), String> {
        if self.is_maximized(window) {
            self.set_maximized(window, false);
        }
        let [left, right, top, bottom] = self.extents(window, self.atoms._NET_FRAME_EXTENTS);
        let [shadow_left, shadow_right, shadow_top, shadow_bottom] = self.extents(window, self.atoms._GTK_FRAME_EXTENTS);
        // With north-west gravity, x and y are where the frame's top left corner goes, and the
        // size is the client window's.
        const NORTH_WEST: u32 = 1;
        const X_Y_WIDTH_HEIGHT: u32 = 0b1111 << 8;
        let width = bounds.width - left - right + shadow_left + shadow_right;
        let height = bounds.height - top - bottom + shadow_top + shadow_bottom;
        let data = [
            NORTH_WEST | X_Y_WIDTH_HEIGHT | FROM_PAGER << 12,
            (bounds.left - shadow_left) as u32,
            (bounds.top - shadow_top) as u32,
            width.max(1) as u32,
            height.max(1) as u32,
        ];
        self.send(window, self.atoms._NET_MOVERESIZE_WINDOW, data)
            .map_err(|error| format!("Could not move the window: {error}"))
    }

    fn pointer(&self) -> (i32, i32) {
        self.conn
            .query_pointer(self.root)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .map_or((0, 0), |reply| (reply.root_x.into(), reply.root_y.into()))
    }

    fn is_maximized(&self, window: Window) -> bool {
        let state = self.cardinals(window, self.atoms._NET_WM_STATE, AtomEnum::ATOM);
        state.contains(&self.atoms._NET_WM_STATE_MAXIMIZED_VERT) && state.contains(&self.atoms._NET_WM_STATE_MAXIMIZED_HORZ)
    }

    fn set_maximized(&mut self, window: Window, maximized: bool) {
        let action = if maximized { ADD } else { REMOVE };
        let data = [action, self.atoms._NET_WM_STATE_MAXIMIZED_VERT, self.atoms._NET_WM_STATE_MAXIMIZED_HORZ, FROM_PAGER, 0];
        if let Err(error) = self.send(window, self.atoms._NET_WM_STATE, data) {
            eprintln!("zones: could not change the window: {error}");
        }
    }
}

/// Where the two overlap; `None` when they don't.
fn intersection(one: Rect, other: Rect) -> Option<Rect> {
    let (left, top) = (one.left.max(other.left), one.top.max(other.top));
    let (right, bottom) = (one.right().min(other.right()), one.bottom().min(other.bottom()));
    (left < right && top < bottom).then(|| Rect::from_edges(left, top, right, bottom))
}

/// Every monitor connected now, for the zone editor.
pub fn monitors() -> Vec<Monitor> {
    SystemDesktop::new().map(|desktop| desktop.monitors()).unwrap_or_default()
}
