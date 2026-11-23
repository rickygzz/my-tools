//! The calculator's window: a plain winit window that egui draws into, through wgpu.
//!
//! egui itself only produces triangles. Three pieces connect it to the window:
//!
//! - `egui_winit::State` turns winit's events (keys, mouse, resizes) into egui input, and
//!   carries out egui's output (clipboard, cursor shape, opening links);
//! - `egui_wgpu::winit::Painter` draws the triangles on the GPU, with wgpu (DirectX or Vulkan
//!   on Windows, Metal on macOS, Vulkan or OpenGL on Linux);
//! - [`view::draw`] is the calculator's own UI, run once per frame.

use std::error::Error;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::ViewportId;
use egui_wgpu::winit::Painter;
use egui_wgpu::{RendererOptions, WgpuConfiguration, WgpuSetup};
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window as WinitWindow, WindowAttributes, WindowId, WindowLevel};

use super::presenter::Presenter;
use super::view::{self, Action, ViewState};

/// A screen: its top-left corner and size, in physical pixels.
pub type Screen = (PhysicalPosition<i32>, PhysicalSize<u32>);

pub struct Window {
    window: Arc<WinitWindow>,
    egui: egui_winit::State,
    painter: Painter,
    view: ViewState,
    /// The height last asked for, in points, to ask again only when it changes.
    height: f32,
    /// When egui wants to draw again: a blinking caret, "Copied" going away.
    repaint_at: Option<Instant>,
}

impl Window {
    pub fn new(event_loop: &ActiveEventLoop) -> Result<Self, Box<dyn Error>> {
        #[cfg_attr(not(windows), allow(unused_mut))]
        let mut attributes = WindowAttributes::default()
            .with_title("Calculator")
            .with_decorations(false)
            .with_resizable(false)
            .with_visible(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_inner_size(LogicalSize::new(view::WIDTH, view::BASE_HEIGHT));
        #[cfg(windows)]
        {
            use winit::platform::windows::WindowAttributesExtWindows;
            attributes = attributes.with_skip_taskbar(true);
        }
        let window = Arc::new(event_loop.create_window(attributes)?);

        let ctx = egui::Context::default();
        ctx.set_visuals(egui::Visuals::dark());

        // wgpu's setup is async (it can wait for the GPU); pollster just waits for it here.
        let config = WgpuConfiguration {
            wgpu_setup: WgpuSetup::from_display_handle(event_loop.owned_display_handle()),
            ..Default::default()
        };
        let mut painter =
            pollster::block_on(Painter::new(ctx.clone(), config, false, RendererOptions::default()));
        pollster::block_on(painter.set_window(ViewportId::ROOT, Some(window.clone())))?;

        let egui = egui_winit::State::new(
            ctx,
            ViewportId::ROOT,
            event_loop,
            Some(window.scale_factor() as f32),
            event_loop.system_theme(),
            painter.max_texture_side(),
        );

        Ok(Self {
            window,
            egui,
            painter,
            view: ViewState::default(),
            height: view::BASE_HEIGHT,
            repaint_at: None,
        })
    }

    pub fn id(&self) -> WindowId {
        self.window.id()
    }

    /// Shows the window centered on `screen`, with the whole line selected, ready to type.
    pub fn show(&mut self, screen: Option<Screen>) {
        self.view.just_shown = true;
        if let Some((position, size)) = screen {
            let window = self.window.outer_size();
            let x = position.x + (size.width as i32 - window.width as i32) / 2;
            let y = position.y + (size.height as i32 - window.height as i32) / 2;
            self.window.set_outer_position(PhysicalPosition::new(x, y));
        }
        self.window.set_visible(true);
        self.window.focus_window();
        self.window.request_redraw();
    }

    pub fn hide(&mut self) {
        self.window.set_visible(false);
        self.repaint_at = None;
    }

    pub fn window_event(&mut self, presenter: &mut Presenter, event: &WindowEvent) {
        if let WindowEvent::RedrawRequested = event {
            self.redraw(presenter);
            return;
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
            // It goes away when another window is used. Alt+F4 (or the like) hides it too,
            // rather than closing it, so it keeps its history.
            WindowEvent::Focused(false) | WindowEvent::CloseRequested => self.hide(),
            _ => {}
        }
    }

    /// When the window wants to be drawn again, if it does.
    pub fn next_wake(&self) -> Option<Instant> {
        self.repaint_at
    }

    /// Asks for a frame once the time egui asked for has come.
    pub fn wake(&mut self, now: Instant) {
        if self.repaint_at.is_some_and(|at| at <= now) {
            self.repaint_at = None;
            self.window.request_redraw();
        }
    }

    fn redraw(&mut self, presenter: &mut Presenter) {
        let input = self.egui.take_egui_input(&self.window);
        let ctx = self.egui.egui_ctx().clone();
        let mut actions = Vec::new();
        let mut output = ctx.run_ui(input, |ui| actions = view::draw(ui, presenter, &mut self.view));

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

        for action in actions {
            match action {
                Action::Hide => self.hide(),
                Action::StartDrag => {
                    let _ = self.window.drag_window();
                }
                Action::MoveBy(points) => self.move_by(points),
            }
        }

        let height = view::height(presenter);
        if height != self.height {
            self.height = height;
            let _ = self.window.request_inner_size(LogicalSize::new(view::WIDTH, height));
            self.keep_on_screen();
        }
    }

    fn move_by(&self, points: egui::Vec2) {
        let Ok(position) = self.window.outer_position() else { return };
        let scale = self.window.scale_factor();
        let (dx, dy) = ((f64::from(points.x) * scale) as i32, (f64::from(points.y) * scale) as i32);
        self.window.set_outer_position(PhysicalPosition::new(position.x + dx, position.y + dy));
    }

    /// Moves the window up if growing took its bottom off the screen.
    fn keep_on_screen(&self) {
        let (Some(monitor), Ok(position)) = (self.window.current_monitor(), self.window.outer_position()) else {
            return;
        };
        let height = (f64::from(self.height) * self.window.scale_factor()) as i32;
        let screen_bottom = monitor.position().y + monitor.size().height as i32;
        if position.y + height > screen_bottom {
            let y = (screen_bottom - height).max(monitor.position().y);
            self.window.set_outer_position(PhysicalPosition::new(position.x, y));
        }
    }
}
