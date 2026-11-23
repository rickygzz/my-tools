//! The calculator's window: a plain winit window that egui draws into, through wgpu.
//!
//! egui itself only produces triangles. Three pieces connect it to the window:
//!
//! - `egui_winit::State` turns winit's events (keys, mouse, resizes) into egui input, and
//!   carries out egui's output (clipboard, cursor shape, opening links);
//! - `egui_wgpu::winit::Painter` draws the triangles on the GPU, with wgpu (DirectX or Vulkan
//!   on Windows, Metal on macOS, Vulkan or OpenGL on Linux);
//! - [`view::draw`] is the calculator's own UI, run once per frame.
//!
//! **Hiding.** Windows, macOS and X11 hide the window and show it again. Wayland has no way to
//! hide a window (winit's `set_visible` does nothing there), so on Wayland the window is closed
//! instead and a new one made the next time. Only the window and its drawing surface go: the
//! GPU device, egui and the presenter stay, so reopening is quick and nothing is forgotten.

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
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::{Window as WinitWindow, WindowAttributes, WindowId, WindowLevel};

use super::presenter::Presenter;
use super::view::{self, Action, ViewState};

/// A screen: its top-left corner and size, in physical pixels.
pub type Screen = (PhysicalPosition<i32>, PhysicalSize<u32>);

pub struct Window {
    /// The OS window; `None` while closed (Wayland's way of hiding it).
    window: Option<Arc<WinitWindow>>,
    egui: egui_winit::State,
    painter: Painter,
    view: ViewState,
    /// The height last asked for, in points, to ask again only when it changes.
    height: f32,
    /// When egui wants to draw again: a blinking caret, "Copied" going away.
    repaint_at: Option<Instant>,
}

impl Window {
    pub fn new(event_loop: &ActiveEventLoop, token: Option<String>) -> Result<Self, Box<dyn Error>> {
        let window = create_window(event_loop, token)?;

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
            window: Some(window),
            egui,
            painter,
            view: ViewState::default(),
            height: view::BASE_HEIGHT,
            repaint_at: None,
        })
    }

    pub fn id(&self) -> Option<WindowId> {
        self.window.as_ref().map(|window| window.id())
    }

    /// Shows the window centered on `screen`, with the whole line selected, ready to type.
    /// `token` is used if a new window has to be made (Wayland, see [`create_window`]).
    pub fn show(
        &mut self,
        event_loop: &ActiveEventLoop,
        screen: Option<Screen>,
        token: Option<String>,
    ) -> Result<(), Box<dyn Error>> {
        let window = match &self.window {
            Some(window) => window.clone(),
            None => {
                let window = create_window(event_loop, token)?;
                pollster::block_on(self.painter.set_window(ViewportId::ROOT, Some(window.clone())))?;
                self.height = view::BASE_HEIGHT;
                self.window = Some(window.clone());
                window
            }
        };

        self.view.just_shown = true;
        if let Some((position, size)) = screen {
            let outer = window.outer_size();
            let x = position.x + (size.width as i32 - outer.width as i32) / 2;
            let y = position.y + (size.height as i32 - outer.height as i32) / 2;
            window.set_outer_position(PhysicalPosition::new(x, y));
        }
        window.set_visible(true);
        window.focus_window();
        window.request_redraw();
        Ok(())
    }

    /// Hides the window, or closes it where it can't be hidden (Wayland).
    pub fn hide(&mut self) {
        self.repaint_at = None;
        let Some(window) = &self.window else { return };
        if can_hide(window) {
            window.set_visible(false);
        } else {
            // Drop the drawing surface first: it refers to the window.
            let _ = pollster::block_on(self.painter.set_window(ViewportId::ROOT, None));
            self.window = None;
        }
    }

    pub fn window_event(&mut self, presenter: &mut Presenter, event: &WindowEvent) {
        let Some(window) = self.window.clone() else { return };

        if let WindowEvent::RedrawRequested = event {
            self.redraw(&window, presenter);
            return;
        }

        let response = self.egui.on_window_event(&window, event);
        if response.repaint {
            window.request_redraw();
        }

        match event {
            WindowEvent::Resized(size) => {
                if let (Some(width), Some(height)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) {
                    self.painter.on_window_resized(ViewportId::ROOT, width, height);
                }
            }
            // It goes away when another window is used. Alt+F4 (or the like) hides it too,
            // rather than closing it for good, so it keeps its history.
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
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
    }

    fn redraw(&mut self, window: &Arc<WinitWindow>, presenter: &mut Presenter) {
        let input = self.egui.take_egui_input(window);
        let ctx = self.egui.egui_ctx().clone();
        let mut actions = Vec::new();
        let mut output = ctx.run_ui(input, |ui| actions = view::draw(ui, presenter, &mut self.view));

        self.egui.handle_platform_output(window, std::mem::take(&mut output.platform_output));
        let primitives = ctx.tessellate(std::mem::take(&mut output.shapes), output.pixels_per_point);
        self.painter.paint_and_update_textures(
            ViewportId::ROOT,
            output.pixels_per_point,
            [0.0, 0.0, 0.0, 1.0],
            &primitives,
            &mut output.textures_delta,
            Vec::new(),
            window,
        );

        let delay = output.viewport_output.get(&ViewportId::ROOT).map_or(Duration::MAX, |viewport| viewport.repaint_delay);
        self.repaint_at = if delay.is_zero() {
            window.request_redraw();
            None
        } else {
            Instant::now().checked_add(delay)
        };

        for action in actions {
            match action {
                Action::Hide => {
                    self.hide();
                    return;
                }
                Action::StartDrag => {
                    let _ = window.drag_window();
                }
                Action::MoveBy(points) => move_by(window, points),
            }
        }

        let height = view::height(presenter);
        if height != self.height {
            self.height = height;
            let _ = window.request_inner_size(LogicalSize::new(view::WIDTH, height));
            keep_on_screen(window, height);
        }
    }
}

/// Makes the (hidden) window. On Wayland, the desktop only lets a new window take the
/// keyboard focus with an activation token from the key press that asked for it; without one,
/// GNOME shows "Calculator is ready" instead.
fn create_window(event_loop: &ActiveEventLoop, token: Option<String>) -> Result<Arc<WinitWindow>, Box<dyn Error>> {
    #[cfg_attr(not(any(windows, target_os = "linux")), allow(unused_mut))]
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
    #[cfg(target_os = "linux")]
    if let Some(token) = token {
        use winit::platform::startup_notify::WindowAttributesExtStartupNotify;
        attributes = attributes.with_activation_token(winit::window::ActivationToken::from_raw(token));
    }
    #[cfg(not(target_os = "linux"))]
    let _ = token;
    Ok(Arc::new(event_loop.create_window(attributes)?))
}

/// Whether the system can hide this window: every one but Wayland.
fn can_hide(window: &WinitWindow) -> bool {
    !matches!(window.window_handle().map(|handle| handle.as_raw()), Ok(RawWindowHandle::Wayland(_)))
}

fn move_by(window: &WinitWindow, points: egui::Vec2) {
    let Ok(position) = window.outer_position() else { return };
    let scale = window.scale_factor();
    let (dx, dy) = ((f64::from(points.x) * scale) as i32, (f64::from(points.y) * scale) as i32);
    window.set_outer_position(PhysicalPosition::new(position.x + dx, position.y + dy));
}

/// Moves the window up if growing to `height` points took its bottom off the screen.
fn keep_on_screen(window: &WinitWindow, height: f32) {
    let (Some(monitor), Ok(position)) = (window.current_monitor(), window.outer_position()) else {
        return;
    };
    let height = (f64::from(height) * window.scale_factor()) as i32;
    let screen_bottom = monitor.position().y + monitor.size().height as i32;
    if position.y + height > screen_bottom {
        let y = (screen_bottom - height).max(monitor.position().y);
        window.set_outer_position(PhysicalPosition::new(position.x, y));
    }
}
