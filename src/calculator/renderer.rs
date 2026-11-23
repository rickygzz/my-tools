//! Puts egui's triangles on the screen: on the GPU with wgpu when there's a usable GPU, or
//! through OpenGL otherwise.
//!
//! wgpu needs Metal on macOS, Vulkan or OpenGL ES on Linux, DirectX or Vulkan on Windows.
//! Virtual machines often have none of those (a macOS VM has no Metal), so then the window is
//! drawn through plain OpenGL instead, which every desktop OS still has, if need be in
//! software (macOS's Apple Software Renderer, Mesa's llvmpipe, Windows' GDI renderer).
//!
//! Set `MY_TOOLS_RENDERER=opengl` to skip wgpu, to try the OpenGL path on any machine.

use std::error::Error;
use std::ffi::CString;
use std::num::NonZeroU32;
use std::sync::Arc;

use egui::ViewportId;
use egui_glow::glow;
use egui_wgpu::winit::Painter;
use egui_wgpu::{RendererOptions, WgpuConfiguration, WgpuSetup};
use glutin::config::{Config, ConfigTemplateBuilder, GlConfig};
use glutin::context::{ContextApi, ContextAttributesBuilder, PossiblyCurrentContext, Version};
use glutin::display::{GetGlDisplay, GlDisplay};
use glutin::prelude::{NotCurrentGlContext, PossiblyCurrentGlContext};
use glutin::surface::{GlSurface, Surface, SwapInterval, WindowSurface};
use glutin_winit::{DisplayBuilder, GlWindow};
use winit::dpi::PhysicalSize;
use winit::event_loop::ActiveEventLoop;
use winit::raw_window_handle::HasWindowHandle;
use winit::window::{Window, WindowAttributes};

/// The background, behind egui's own (opaque) panel.
const CLEAR_COLOR: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

// Only one ever exists, so the variants' sizes don't matter.
#[allow(clippy::large_enum_variant)]
pub enum Renderer {
    Wgpu(Painter),
    OpenGl(OpenGl),
}

impl Renderer {
    /// Makes the window from `attributes`, and the renderer that draws into it.
    pub fn new(
        event_loop: &ActiveEventLoop,
        ctx: &egui::Context,
        attributes: WindowAttributes,
    ) -> Result<(Self, Arc<Window>), Box<dyn Error>> {
        let forced = std::env::var("MY_TOOLS_RENDERER").is_ok_and(|name| name.eq_ignore_ascii_case("opengl"));
        if !forced {
            match Self::new_wgpu(event_loop, ctx, attributes.clone()) {
                Ok(made) => return Ok(made),
                Err(error) => eprintln!("calculator: no GPU drawing ({error}); drawing with OpenGL instead"),
            }
        }
        let (gl, window) = OpenGl::new(event_loop, attributes)
            .map_err(|e| format!("can't draw the window: no usable GPU, and OpenGL failed too: {e}"))?;
        Ok((Self::OpenGl(gl), window))
    }

    fn new_wgpu(
        event_loop: &ActiveEventLoop,
        ctx: &egui::Context,
        attributes: WindowAttributes,
    ) -> Result<(Self, Arc<Window>), Box<dyn Error>> {
        let window = Arc::new(event_loop.create_window(attributes)?);
        // wgpu's setup is async (it can wait for the GPU); pollster just waits for it here.
        let config = WgpuConfiguration {
            wgpu_setup: WgpuSetup::from_display_handle(event_loop.owned_display_handle()),
            ..Default::default()
        };
        let mut painter = pollster::block_on(Painter::new(ctx.clone(), config, false, RendererOptions::default()));
        pollster::block_on(painter.set_window(ViewportId::ROOT, Some(window.clone())))?;
        Ok((Self::Wgpu(painter), window))
    }

    /// Makes a new window for this renderer, after the last one was closed (Wayland's way of
    /// hiding it).
    pub fn new_window(
        &mut self,
        event_loop: &ActiveEventLoop,
        attributes: WindowAttributes,
    ) -> Result<Arc<Window>, Box<dyn Error>> {
        match self {
            Self::Wgpu(painter) => {
                let window = Arc::new(event_loop.create_window(attributes)?);
                pollster::block_on(painter.set_window(ViewportId::ROOT, Some(window.clone())))?;
                Ok(window)
            }
            Self::OpenGl(gl) => gl.new_window(event_loop, attributes),
        }
    }

    /// Lets go of the window, before it's closed.
    pub fn drop_window(&mut self) {
        match self {
            Self::Wgpu(painter) => {
                let _ = pollster::block_on(painter.set_window(ViewportId::ROOT, None));
            }
            Self::OpenGl(gl) => gl.drop_window(),
        }
    }

    pub fn resized(&mut self, size: PhysicalSize<u32>) {
        let (Some(width), Some(height)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else {
            return;
        };
        match self {
            Self::Wgpu(painter) => painter.on_window_resized(ViewportId::ROOT, width, height),
            Self::OpenGl(gl) => gl.resized(width, height),
        }
    }

    /// The largest texture egui may use, in pixels.
    pub fn max_texture_side(&self) -> Option<usize> {
        match self {
            Self::Wgpu(painter) => painter.max_texture_side(),
            Self::OpenGl(gl) => Some(gl.painter.max_texture_side()),
        }
    }

    pub fn paint(
        &mut self,
        window: &Arc<Window>,
        pixels_per_point: f32,
        primitives: &[egui::ClippedPrimitive],
        textures_delta: &mut egui::TexturesDelta,
    ) {
        match self {
            Self::Wgpu(painter) => {
                painter.paint_and_update_textures(
                    ViewportId::ROOT,
                    pixels_per_point,
                    CLEAR_COLOR,
                    primitives,
                    textures_delta,
                    Vec::new(),
                    window,
                );
            }
            Self::OpenGl(gl) => gl.paint(window, pixels_per_point, primitives, textures_delta),
        }
    }
}

/// Drawing through OpenGL: `glutin` makes the OpenGL context and the window's surface,
/// `egui_glow` draws into it.
pub struct OpenGl {
    painter: egui_glow::Painter,
    /// The window's drawing surface; `None` while the window is closed.
    surface: Option<Surface<WindowSurface>>,
    context: PossiblyCurrentContext,
    /// What kind of pixels the window has, to make new windows like it.
    config: Config,
}

impl OpenGl {
    fn new(event_loop: &ActiveEventLoop, attributes: WindowAttributes) -> Result<(Self, Arc<Window>), Box<dyn Error>> {
        // Some systems (X11, Windows) need the window made to suit the OpenGL pixel format,
        // so glutin makes the window itself.
        let (window, config) = DisplayBuilder::new()
            .with_window_attributes(Some(attributes))
            .build(event_loop, ConfigTemplateBuilder::new(), |configs| {
                // Prefer the plainest: no multisampling (egui smooths edges itself).
                configs.min_by_key(GlConfig::num_samples).expect("glutin always offers a config")
            })?;
        let window = Arc::new(window.ok_or("glutin didn't make the window")?);

        let display = config.display();
        let handle = Some(window.window_handle()?.as_raw());
        // OpenGL 3+ first; else OpenGL ES 2, which some software renderers offer instead.
        let context = unsafe {
            display
                .create_context(&config, &ContextAttributesBuilder::new().build(handle))
                .or_else(|_| {
                    let gles = ContextAttributesBuilder::new()
                        .with_context_api(ContextApi::Gles(Some(Version::new(2, 0))))
                        .build(handle);
                    display.create_context(&config, &gles)
                })?
        };
        let surface = make_surface(&config, &window)?;
        let context = context.make_current(&surface)?;
        // Wait for the screen's refresh, so frames don't tear; not every system can.
        let _ = surface.set_swap_interval(&context, SwapInterval::Wait(NonZeroU32::MIN));

        let gl = unsafe {
            glow::Context::from_loader_function(|name| {
                CString::new(name).map_or(std::ptr::null(), |name| display.get_proc_address(&name))
            })
        };
        {
            use glow::HasContext;
            let (renderer, version) =
                unsafe { (gl.get_parameter_string(glow::RENDERER), gl.get_parameter_string(glow::VERSION)) };
            eprintln!("calculator: drawing with OpenGL: {renderer}, {version}");
        }
        let painter = egui_glow::Painter::new(Arc::new(gl), "", None, false)?;

        Ok((Self { painter, surface: Some(surface), context, config }, window))
    }

    fn new_window(
        &mut self,
        event_loop: &ActiveEventLoop,
        attributes: WindowAttributes,
    ) -> Result<Arc<Window>, Box<dyn Error>> {
        let window = Arc::new(glutin_winit::finalize_window(event_loop, attributes, &self.config)?);
        let surface = make_surface(&self.config, &window)?;
        self.context.make_current(&surface)?;
        self.surface = Some(surface);
        Ok(window)
    }

    fn drop_window(&mut self) {
        let _ = self.context.make_not_current_in_place();
        self.surface = None;
    }

    fn resized(&mut self, width: NonZeroU32, height: NonZeroU32) {
        if let Some(surface) = &self.surface {
            surface.resize(&self.context, width, height);
        }
    }

    fn paint(
        &mut self,
        window: &Window,
        pixels_per_point: f32,
        primitives: &[egui::ClippedPrimitive],
        textures_delta: &mut egui::TexturesDelta,
    ) {
        let Some(surface) = &self.surface else { return };
        if let Err(error) = self.context.make_current(surface) {
            eprintln!("calculator: can't draw: {error}");
            return;
        }
        let size = window.inner_size();
        let size = [size.width, size.height];
        self.painter.clear(size, CLEAR_COLOR);
        self.painter.paint_and_update_textures(size, pixels_per_point, primitives, textures_delta);
        if let Err(error) = surface.swap_buffers(&self.context) {
            eprintln!("calculator: can't show what was drawn: {error}");
        }
    }
}

impl Drop for OpenGl {
    /// egui_glow's textures and shaders have to be freed while the context is current.
    fn drop(&mut self) {
        if let Some(surface) = &self.surface {
            let _ = self.context.make_current(surface);
        }
        self.painter.destroy();
    }
}

fn make_surface(config: &Config, window: &Window) -> Result<Surface<WindowSurface>, Box<dyn Error>> {
    let attributes = window.build_surface_attributes(Default::default())?;
    Ok(unsafe { config.display().create_window_surface(config, &attributes)? })
}
