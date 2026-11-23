//! Toasts: a small message in the corner of the main screen for a few seconds, such as "The
//! window stays on top" or why a window couldn't be moved. It's our own egui window, so it
//! looks and works the same on every OS, with no notification permission or setup; a click
//! closes it.
//!
//! Any part of the app, on any thread, calls [`post`]; the event loop shows what was posted
//! (`App::about_to_wait` calls [`Toaster::show_posted`]).

use std::error::Error;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::{WindowAttributes, WindowId, WindowLevel};

use crate::egui_window::EguiWindow;

/// How long a toast shows.
const SHOWN_FOR: Duration = Duration::from_secs(5);
/// Its width, and its distance from the screen's edges, in points.
const WIDTH: f32 = 380.0;
const MARGIN: f64 = 16.0;
/// Above the taskbar or Dock, which the screen's size includes.
const BOTTOM_MARGIN: f64 = 72.0;

static POSTED: Mutex<Vec<Message>> = Mutex::new(Vec::new());

#[derive(Debug, Clone, PartialEq, Eq)]
struct Message {
    title: String,
    body: String,
}

/// Shows `body` under `title` in a toast, soon. Also printed on stderr, for when there's a
/// terminal.
pub fn post(title: &str, body: &str) {
    eprintln!("{title}: {body}");
    let message = Message { title: title.to_owned(), body: body.to_owned() };
    POSTED.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(message);
}

#[derive(Default)]
pub struct Toaster {
    /// Made at the first toast. On Wayland, where windows can't be hidden, it's closed instead.
    window: Option<EguiWindow>,
    message: Option<Message>,
    hide_at: Option<Instant>,
    /// The height last asked for, in points.
    height: f32,
}

impl Toaster {
    /// Shows the last message posted, if any, in place of the one showing.
    pub fn show_posted(&mut self, event_loop: &ActiveEventLoop) {
        let message = {
            let mut posted = POSTED.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let last = posted.pop();
            posted.clear();
            last
        };
        let Some(message) = message else { return };
        if let Err(error) = self.show(event_loop, message) {
            eprintln!("toast: could not show it: {error}");
        }
    }

    fn show(&mut self, event_loop: &ActiveEventLoop, message: Message) -> Result<(), Box<dyn Error>> {
        if self.window.is_none() {
            #[cfg_attr(not(windows), allow(unused_mut))]
            let mut attributes = WindowAttributes::default()
                .with_title("My Tools")
                .with_decorations(false)
                .with_resizable(false)
                .with_visible(false)
                .with_active(false)
                .with_window_level(WindowLevel::AlwaysOnTop)
                .with_inner_size(LogicalSize::new(WIDTH, 80.0));
            #[cfg(windows)]
            {
                use winit::platform::windows::WindowAttributesExtWindows;
                attributes = attributes.with_skip_taskbar(true);
            }
            self.window = Some(EguiWindow::new(event_loop, attributes)?);
            self.height = 80.0;
        }
        self.message = Some(message);
        self.hide_at = Instant::now().checked_add(SHOWN_FOR);
        if let Some(egui) = &self.window {
            place(egui, event_loop, self.height);
            egui.window.set_visible(true);
            egui.window.request_redraw();
        }
        Ok(())
    }

    fn hide(&mut self) {
        self.message = None;
        self.hide_at = None;
        if crate::is_wayland() {
            self.window = None;
        } else if let Some(egui) = &self.window {
            egui.window.set_visible(false);
        }
    }

    pub fn owns(&self, id: WindowId) -> bool {
        self.window.as_ref().is_some_and(|egui| egui.id() == id)
    }

    pub fn window_event(&mut self, event_loop: &ActiveEventLoop, event: &WindowEvent) {
        let Some(egui) = &mut self.window else { return };
        if !egui.on_event(event) {
            return;
        }
        let Some(message) = &self.message else { return };
        let mut clicked = false;
        let mut height = self.height;
        egui.draw(|ui| {
            let frame = egui::Frame::new()
                .fill(egui::Color32::from_rgb(36, 36, 40))
                .stroke(egui::Stroke::new(1.0, egui::Color32::from_gray(90)))
                .inner_margin(egui::Margin::same(12));
            let response = frame
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(egui::RichText::new(&message.title).strong().size(15.0));
                    ui.label(egui::RichText::new(&message.body).color(egui::Color32::from_gray(210)));
                })
                .response
                .interact(egui::Sense::click());
            clicked = response.clicked();
            height = response.rect.height();
        });
        if clicked {
            self.hide();
            return;
        }
        // As tall as its text.
        if (height - self.height).abs() > 1.0 {
            self.height = height;
            let _ = egui.window.request_inner_size(LogicalSize::new(WIDTH, height));
            place(egui, event_loop, height);
        }
    }

    pub fn next_wake(&self) -> Option<Instant> {
        [self.hide_at, self.window.as_ref().and_then(EguiWindow::next_wake)].into_iter().flatten().min()
    }

    pub fn wake(&mut self, now: Instant) {
        if self.hide_at.is_some_and(|at| at <= now) {
            self.hide();
        }
        if let Some(egui) = &mut self.window {
            egui.wake(now);
        }
    }

    /// Closes the window while the event loop still exists.
    pub fn close(&mut self) {
        self.window = None;
    }
}

/// In the bottom right corner of the main screen.
fn place(egui: &EguiWindow, event_loop: &ActiveEventLoop, height: f32) {
    let Some(monitor) = event_loop.primary_monitor().or_else(|| event_loop.available_monitors().next()) else {
        return;
    };
    let scale = monitor.scale_factor();
    let (position, size) = (monitor.position(), monitor.size());
    let x = f64::from(position.x) + f64::from(size.width) - (f64::from(WIDTH) + MARGIN) * scale;
    let y = f64::from(position.y) + f64::from(size.height) - (f64::from(height) + BOTTOM_MARGIN) * scale;
    egui.window.set_outer_position(PhysicalPosition::new(x as i32, y as i32));
}
