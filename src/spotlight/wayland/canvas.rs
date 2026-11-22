//! See-through pixel buffers for the overlay windows.
//!
//! winit opens the windows; we attach the pixels ourselves, over winit's own Wayland
//! connection, using shared memory in ARGB format. (softbuffer would do this for us, but on
//! Wayland it only offers opaque XRGB.) Modelled on softbuffer's Wayland backend.
//!
//! Each window has two buffers: while the compositor shows one, we draw into the other.
//! Since the picture is entirely determined by where the circle is, each buffer remembers
//! which circle it holds, and only the area around the old and new circle is repainted.

use std::error::Error;
use std::ffi::c_void;
use std::fs::File;
use std::os::fd::AsFd;
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use memmap2::MmapMut;
use rustix::fs::{MemfdFlags, SealFlags};
use wayland_backend::client::{Backend, ObjectId};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_buffer, wl_registry, wl_shm, wl_shm_pool, wl_surface};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

use super::super::shape::{self, Area, Circle, Colors};
use super::super::Point;

/// Our own view of winit's Wayland connection, with an event queue for our buffers' events.
pub struct Display {
    conn: Connection,
    queue: EventQueue<State>,
    qh: QueueHandle<State>,
    shm: wl_shm::WlShm,
}

impl Display {
    /// # Safety
    /// `display` must be a live `wl_display` that outlives the returned `Display` and every
    /// `Canvas` made with it.
    pub unsafe fn new(display: NonNull<c_void>) -> Result<Self, Box<dyn Error>> {
        // SAFETY: guaranteed by the caller.
        let backend = unsafe { Backend::from_foreign_display(display.as_ptr().cast()) };
        let conn = Connection::from_backend(backend);
        let (globals, queue) = registry_queue_init::<State>(&conn)?;
        let qh = queue.handle();
        let shm = globals.bind(&qh, 1..=1, ())?;
        Ok(Self { conn, queue, qh, shm })
    }
}

/// Where the circle is, in window pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Spot {
    pub center: Point,
    pub radius: i32,
    pub ring: i32,
}

impl Spot {
    fn circle(&self) -> Circle {
        Circle::new(self.center, f64::from(self.radius), f64::from(self.ring))
    }
}

/// What a buffer holds: all dim, or dim with this circle.
type Picture = Option<Spot>;

/// The area a picture's circle covers, if any.
fn circle_area(picture: Picture, width: i32, height: i32) -> Option<Area> {
    picture.map(|spot| spot.circle().bounds(width, height))
}

fn union(a: Option<Area>, b: Option<Area>) -> Option<Area> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.union(&b)),
        (a, b) => a.or(b),
    }
}

/// One window's surface and its two buffers.
pub struct Canvas {
    surface: wl_surface::WlSurface,
    buffers: [Option<Buffer>; 2],
    /// Which buffer the compositor was last given.
    front: Option<usize>,
}

impl Canvas {
    pub fn new(display: &Display, window: &Window) -> Result<Self, Box<dyn Error>> {
        let RawWindowHandle::Wayland(handle) = window.window_handle()?.as_raw() else {
            return Err("not a Wayland window".into());
        };
        // SAFETY: the pointer is winit's live wl_surface for `window`, and the Canvas is
        // dropped before the window (see `Screen`).
        let id = unsafe {
            ObjectId::from_ptr(wl_surface::WlSurface::interface(), handle.surface.as_ptr().cast())
        }?;
        let surface = wl_surface::WlSurface::from_id(&display.conn, id)?;
        Ok(Self { surface, buffers: [None, None], front: None })
    }

    /// Shows `picture` at `width` x `height` pixels. Returns false if the frame was skipped
    /// because the compositor still holds the spare buffer.
    pub fn draw(
        &mut self,
        display: &mut Display,
        width: u32,
        height: u32,
        picture: Picture,
        colors: Colors,
    ) -> bool {
        let (Ok(w), Ok(h)) = (i32::try_from(width), i32::try_from(height)) else {
            return false;
        };
        if w == 0 || h == 0 {
            return false;
        }
        // Handle "buffer released" events that have arrived.
        let _ = display.queue.dispatch_pending(&mut State);

        let front = self.front.and_then(|i| self.buffers[i].as_ref());
        if front.is_some_and(|f| f.size() == (w, h) && f.picture == Some(picture)) {
            return true; // already on screen
        }
        // What the compositor must update, compared with what it shows now.
        let whole = Area::new(0, 0, w, h);
        let changed = match front {
            Some(f) if f.size() == (w, h) => match f.picture {
                Some(shown) => union(circle_area(shown, w, h), circle_area(picture, w, h)),
                None => Some(whole),
            },
            _ => Some(whole),
        };

        let back_index = match self.front {
            Some(i) => 1 - i,
            None => 0,
        };
        let slot = &mut self.buffers[back_index];
        if slot.as_ref().is_some_and(|b| !b.released()) {
            return false;
        }
        if slot.as_ref().is_none_or(|b| b.size() != (w, h)) {
            *slot = match Buffer::new(display, w, h) {
                Ok(buffer) => Some(buffer),
                Err(error) => {
                    eprintln!("spotlight: could not make a pixel buffer: {error}");
                    return false;
                }
            };
        }
        let Some(back) = slot.as_mut() else {
            return false;
        };

        // Repaint what differs between what this buffer holds and the new picture.
        let repaint = match back.picture {
            Some(held) => union(circle_area(held, w, h), circle_area(picture, w, h)),
            None => Some(whole),
        };
        if let Some(area) = repaint {
            let pixels = back.pixels_mut();
            match picture {
                Some(spot) => spot.circle().paint(pixels, whole, area, colors),
                None => shape::fill(pixels, whole, area, colors.dim),
            }
        }
        back.picture = Some(picture);

        back.attach(&self.surface);
        if let Some(area) = changed {
            self.surface.damage_buffer(area.left, area.top, area.width(), area.height());
        }
        self.surface.commit();
        let _ = display.conn.flush();
        self.front = Some(back_index);
        true
    }
}

/// A shared-memory pixel buffer the compositor reads from.
struct Buffer {
    map: MmapMut,
    pool: wl_shm_pool::WlShmPool,
    buffer: wl_buffer::WlBuffer,
    width: i32,
    height: i32,
    /// False while the compositor may be reading it.
    released: Arc<AtomicBool>,
    /// What it holds, or `None` if it hasn't been fully painted yet.
    picture: Option<Picture>,
    /// Keeps the shared memory open.
    _file: File,
}

impl Buffer {
    fn new(display: &Display, width: i32, height: i32) -> Result<Self, Box<dyn Error>> {
        let bytes = i32::try_from(i64::from(width) * i64::from(height) * 4)?;
        let fd = rustix::fs::memfd_create(
            "my-tools-spotlight",
            MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING,
        )?;
        let file = File::from(fd);
        file.set_len(bytes as u64)?;
        // The compositor maps this memory too: promise it never shrinks under it.
        rustix::fs::fcntl_add_seals(&file, SealFlags::SHRINK | SealFlags::SEAL)?;
        // SAFETY: the memory is only shared with the compositor, which only reads it.
        let map = unsafe { MmapMut::map_mut(&file)? };

        let released = Arc::new(AtomicBool::new(true));
        let pool = display.shm.create_pool(file.as_fd(), bytes, &display.qh, ());
        let buffer = pool.create_buffer(
            0,
            width,
            height,
            width * 4,
            wl_shm::Format::Argb8888,
            &display.qh,
            released.clone(),
        );
        Ok(Self { map, pool, buffer, width, height, released, picture: None, _file: file })
    }

    fn size(&self) -> (i32, i32) {
        (self.width, self.height)
    }

    fn released(&self) -> bool {
        self.released.load(Ordering::SeqCst)
    }

    fn attach(&self, surface: &wl_surface::WlSurface) {
        self.released.store(false, Ordering::SeqCst);
        surface.attach(Some(&self.buffer), 0, 0);
    }

    /// The pixels, one `0xAARRGGBB` per pixel, row after row.
    fn pixels_mut(&mut self) -> &mut [u32] {
        let len = self.width as usize * self.height as usize;
        // SAFETY: the map is page-aligned and holds exactly `len` u32 pixels, and `&mut self`
        // means nothing else in this process is using it.
        unsafe { std::slice::from_raw_parts_mut(self.map.as_mut_ptr().cast::<u32>(), len) }
    }
}

impl Drop for Buffer {
    fn drop(&mut self) {
        self.buffer.destroy();
        self.pool.destroy();
    }
}

/// Receives our objects' events. Only "buffer released" matters.
pub struct State;

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_shm::WlShm, ()> for State {
    fn event(
        _: &mut Self,
        _: &wl_shm::WlShm,
        _: wl_shm::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_shm_pool::WlShmPool, ()> for State {
    fn event(
        _: &mut Self,
        _: &wl_shm_pool::WlShmPool,
        _: wl_shm_pool::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_buffer::WlBuffer, Arc<AtomicBool>> for State {
    fn event(
        _: &mut Self,
        _: &wl_buffer::WlBuffer,
        event: wl_buffer::Event,
        released: &Arc<AtomicBool>,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = event {
            released.store(true, Ordering::SeqCst);
        }
    }
}
