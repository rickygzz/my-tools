//! The window zones editor, opened from the tray menu: a monitor drawn at its shape, with its
//! zones over it, to drag into place, and a toolbar. It saves `zones.cfg`.
//!
//! | File           | What it does                                                    |
//! |----------------|-----------------------------------------------------------------|
//! | `presenter.rs` | what every drag and command does (any OS, unit-tested)          |
//! | `view.rs`      | draws it with egui, and turns mouse and keys into presenter calls |
//! | `mod.rs`       | the window egui draws into, through wgpu, as the calculator's   |
//!
//! Unlike the calculator, it's a normal window, closed for good when done.

mod presenter;
mod view;

use std::error::Error;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::ViewportId;
use egui_wgpu::winit::Painter;
use egui_wgpu::{RendererOptions, WgpuConfiguration, WgpuSetup};
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window as WinitWindow, WindowAttributes, WindowId};

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
    window: Arc<WinitWindow>,
    egui: egui_winit::State,
    painter: Painter,
    presenter: Presenter<FileStore>,
    view: ViewState,
    title: String,
    /// When egui wants to draw again.
    repaint_at: Option<Instant>,
}

impl Editor {
    pub fn open(event_loop: &ActiveEventLoop, monitors: Vec<Monitor>) -> Result<Self, Box<dyn Error>> {
        let attributes = WindowAttributes::default()
            .with_title(TITLE)
            .with_inner_size(LogicalSize::new(900.0, 600.0))
            .with_min_inner_size(LogicalSize::new(520.0, 360.0));
        let window = Arc::new(event_loop.create_window(attributes)?);

        let ctx = egui::Context::default();
        ctx.set_visuals(egui::Visuals::dark());
        let config = WgpuConfiguration {
            wgpu_setup: WgpuSetup::from_display_handle(event_loop.owned_display_handle()),
            ..Default::default()
        };
        let mut painter = pollster::block_on(Painter::new(ctx.clone(), config, false, RendererOptions::default()));
        pollster::block_on(painter.set_window(ViewportId::ROOT, Some(window.clone())))?;
        let egui = egui_winit::State::new(
            ctx,
            ViewportId::ROOT,
            event_loop,
            Some(window.scale_factor() as f32),
            event_loop.system_theme(),
            painter.max_texture_side(),
        );

        let presenter = Presenter::new(FileStore { path: config::file_path() }, monitors);
        window.focus_window();
        Ok(Self { window, egui, painter, presenter, view: ViewState::default(), title: TITLE.into(), repaint_at: None })
    }

    pub fn id(&self) -> WindowId {
        self.window.id()
    }

    /// Brings it to the front, when it's asked for again while open.
    pub fn focus(&self) {
        self.window.set_minimized(false);
        self.window.focus_window();
    }

    /// Returns false once the editor is done and should be closed.
    pub fn window_event(&mut self, event: &WindowEvent) -> bool {
        if let WindowEvent::RedrawRequested = event {
            return self.redraw();
        }
        let response = self.egui.on_window_event(&self.window, event);
        if response.repaint {
            self.window.request_redraw();
        }
        match event {
            WindowEvent::Resized(size) => {
                if let (Some(width), Some(height)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) {
                    self.painter.on_window_resized(ViewportId::ROOT, width, height);
                }
            }
            WindowEvent::CloseRequested => {
                if self.presenter.leave(Leaving::Close) {
                    return false;
                }
                // It asks about the unsaved changes on the next frame.
                self.window.request_redraw();
            }
            _ => {}
        }
        true
    }

    pub fn next_wake(&self) -> Option<Instant> {
        self.repaint_at
    }

    pub fn wake(&mut self, now: Instant) {
        if self.repaint_at.is_some_and(|at| at <= now) {
            self.repaint_at = None;
            self.window.request_redraw();
        }
    }

    fn redraw(&mut self) -> bool {
        let input = self.egui.take_egui_input(&self.window);
        let ctx = self.egui.egui_ctx().clone();
        let mut actions = Vec::new();
        let mut output = ctx.run_ui(input, |ui| actions = view::draw(ui, &mut self.presenter, &mut self.view));

        self.egui.handle_platform_output(&self.window, std::mem::take(&mut output.platform_output));
        let primitives = ctx.tessellate(std::mem::take(&mut output.shapes), output.pixels_per_point);
        self.painter.paint_and_update_textures(
            ViewportId::ROOT,
            output.pixels_per_point,
            [0.0, 0.0, 0.0, 1.0],
            &primitives,
            &mut output.textures_delta,
            Vec::new(),
            &self.window,
        );
        let delay = output.viewport_output.get(&ViewportId::ROOT).map_or(Duration::MAX, |viewport| viewport.repaint_delay);
        self.repaint_at = if delay.is_zero() {
            self.window.request_redraw();
            None
        } else {
            Instant::now().checked_add(delay)
        };

        // The title says when there are unsaved changes.
        let title = if self.presenter.unsaved() { format!("{TITLE} *") } else { TITLE.to_owned() };
        if title != self.title {
            self.window.set_title(&title);
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
