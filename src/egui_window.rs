//! A winit window that egui draws into, through wgpu: what the zone editor and the toasts
//! share. (The calculator has its own, as it hides and shows itself in ways of its own.)
//!
//! - `egui_winit::State` turns winit's events into egui input, and carries out egui's output
//!   (clipboard, cursor shape, links);
//! - `egui_wgpu::winit::Painter` draws egui's triangles on the GPU;
//! - the caller's closure draws the UI, once per frame.

use std::error::Error;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::ViewportId;
use egui_wgpu::winit::Painter;
use egui_wgpu::{RendererOptions, WgpuConfiguration, WgpuSetup};
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowAttributes, WindowId};

pub struct EguiWindow {
    pub window: Arc<Window>,
    egui: egui_winit::State,
    painter: Painter,
    /// When egui wants to draw again: a blinking caret, an animation.
    repaint_at: Option<Instant>,
}

impl EguiWindow {
    pub fn new(event_loop: &ActiveEventLoop, attributes: WindowAttributes) -> Result<Self, Box<dyn Error>> {
        let window = Arc::new(event_loop.create_window(attributes)?);
        let ctx = egui::Context::default();
        ctx.set_visuals(egui::Visuals::dark());
        // wgpu's setup is async (it can wait for the GPU); pollster just waits for it here.
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
        Ok(Self { window, egui, painter, repaint_at: None })
    }

    pub fn id(&self) -> WindowId {
        self.window.id()
    }

    /// Gives egui the event. Returns true for [`WindowEvent::RedrawRequested`], when the caller
    /// should call [`EguiWindow::draw`].
    pub fn on_event(&mut self, event: &WindowEvent) -> bool {
        if let WindowEvent::RedrawRequested = event {
            return true;
        }
        if self.egui.on_window_event(&self.window, event).repaint {
            self.window.request_redraw();
        }
        if let WindowEvent::Resized(size) = event
            && let (Some(width), Some(height)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
        {
            self.painter.on_window_resized(ViewportId::ROOT, width, height);
        }
        false
    }

    /// Runs `ui` for a frame and puts the result on the screen.
    pub fn draw(&mut self, ui: impl FnMut(&mut egui::Ui)) {
        let input = self.egui.take_egui_input(&self.window);
        let ctx = self.egui.egui_ctx().clone();
        let mut output = ctx.run_ui(input, ui);
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
    }

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
}
