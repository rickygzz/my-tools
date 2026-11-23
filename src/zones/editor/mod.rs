//! The window zones editor, opened from the tray menu: a monitor drawn at its shape, with its
//! zones over it, to drag into place, and a toolbar. It saves `zones.cfg`.
//!
//! | File           | What it does                                                    |
//! |----------------|-----------------------------------------------------------------|
//! | `presenter.rs` | what every drag and command does (any OS, unit-tested)          |
//! | `view.rs`      | draws it with egui, and turns mouse and keys into presenter calls |
//! | `mod.rs`       | its window, an [`EguiWindow`], and `zones.cfg` as its store      |
//!
//! Unlike the calculator, it's a normal window, closed for good when done.

mod presenter;
mod view;

use std::error::Error;
use std::path::PathBuf;
use std::time::Instant;

use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::{WindowAttributes, WindowId};

use crate::egui_window::EguiWindow;

use super::config;
use super::layout::{Layouts, Monitor};
use presenter::{Leaving, Presenter, Store};
use view::{Action, ViewState};

const TITLE: &str = "Window zones";

/// `zones.cfg` in the user's config folder.
struct FileStore {
    path: Option<PathBuf>,
}

impl Store for FileStore {
    fn load(&mut self) -> Layouts {
        self.path.as_deref().map(|path| config::load(path).layouts).unwrap_or_default()
    }

    fn save(&mut self, layouts: &Layouts) -> Result<(), String> {
        let path = self.path.as_deref().ok_or("There's no config folder to save zones.cfg in.")?;
        config::save(path, layouts)
    }
}

pub struct Editor {
    egui: EguiWindow,
    presenter: Presenter<FileStore>,
    view: ViewState,
    title: String,
}

impl Editor {
    pub fn open(event_loop: &ActiveEventLoop, monitors: Vec<Monitor>) -> Result<Self, Box<dyn Error>> {
        let attributes = WindowAttributes::default()
            .with_title(TITLE)
            .with_inner_size(LogicalSize::new(900.0, 600.0))
            .with_min_inner_size(LogicalSize::new(520.0, 360.0));
        let egui = EguiWindow::new(event_loop, attributes)?;
        egui.window.focus_window();
        let presenter = Presenter::new(FileStore { path: config::file_path() }, monitors);
        Ok(Self { egui, presenter, view: ViewState::default(), title: TITLE.into() })
    }

    pub fn id(&self) -> WindowId {
        self.egui.id()
    }

    /// Brings it to the front, when it's asked for again while open.
    pub fn focus(&self) {
        self.egui.window.set_minimized(false);
        self.egui.window.focus_window();
    }

    /// Returns false once the editor is done and should be closed.
    pub fn window_event(&mut self, event: &WindowEvent) -> bool {
        if self.egui.on_event(event) {
            return self.redraw();
        }
        if let WindowEvent::CloseRequested = event {
            if self.presenter.leave(Leaving::Close) {
                return false;
            }
            // It asks about the unsaved changes on the next frame.
            self.egui.window.request_redraw();
        }
        true
    }

    pub fn next_wake(&self) -> Option<Instant> {
        self.egui.next_wake()
    }

    pub fn wake(&mut self, now: Instant) {
        self.egui.wake(now);
    }

    fn redraw(&mut self) -> bool {
        let mut actions = Vec::new();
        let (presenter, view) = (&mut self.presenter, &mut self.view);
        self.egui.draw(|ui| actions = view::draw(ui, presenter, view));

        // The title says when there are unsaved changes.
        let title = if self.presenter.unsaved() { format!("{TITLE} *") } else { TITLE.to_owned() };
        if title != self.title {
            self.egui.window.set_title(&title);
            self.title = title;
        }

        match actions.first() {
            Some(Action::Leave(Leaving::Close)) => false,
            Some(Action::Leave(Leaving::EditAsText)) => {
                if let Some(path) = config::file_path() {
                    open_in_text_editor(&path);
                }
                false
            }
            None => true,
        }
    }
}

/// Opens the file in the system's text editor, without waiting for it.
fn open_in_text_editor(path: &std::path::Path) {
    #[cfg(windows)]
    let mut command = std::process::Command::new("notepad.exe");
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = std::process::Command::new("open");
        command.arg("-t"); // the default text editor
        command
    };
    #[cfg(target_os = "linux")]
    let mut command = std::process::Command::new("xdg-open");
    if let Err(error) = command.arg(path).spawn() {
        eprintln!("zones: could not open {}: {error}", path.display());
    }
}
