//! Stand-in for operating systems the spotlight doesn't support (e.g. FreeBSD).
//! The listener never sends `Signal::Show`, so the overlay is never needed.

use std::error::Error;

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use super::{Point, Signal};

pub fn start_listener(_notify: impl Fn(Signal) + Send + 'static) {}

pub fn cursor_position() -> Point {
    (0, 0)
}

/// An enum with no variants can never be created, so the methods below can never run.
/// `match *self {}` tells the compiler exactly that.
pub enum Overlay {}

impl Overlay {
    pub fn new(_event_loop: &ActiveEventLoop) -> Result<Self, Box<dyn Error>> {
        Err("the spotlight is only available on Windows for now".into())
    }
    pub fn owns(&self, _id: WindowId) -> bool {
        match *self {}
    }
    pub fn show(&mut self) {
        match *self {}
    }
    pub fn hide(&mut self) {
        match *self {}
    }
    pub fn draw(&mut self, _center: Point, _radius: i32) {
        match *self {}
    }
    pub fn window_event(&mut self, _event: &WindowEvent) {
        match *self {}
    }
}
