//! Window zones: areas of each monitor that windows snap into. Shift while dragging a window
//! shows them, and dropping it on one fills that zone; hotkeys move the active window to a
//! zone, or to the next one.
//!
//! | File          | What it does                                                         |
//! |---------------|----------------------------------------------------------------------|
//! | `layout.rs`   | zones in percent of a monitor, layouts, the editor's templates       |
//! | `geometry.rs` | where the zones are on a monitor, which one a point or window is in  |
//! | `config.rs`   | reads and writes `zones.cfg`                                         |
//! | `desktop.rs`  | the [`desktop::Desktop`] trait: what the zones need from the OS      |
//! | `mover.rs`    | moves the active window to a zone, the next one, or another monitor  |
//!
//! Everything here is plain Rust, the same on every OS, and tested with a fake desktop.

pub mod config;
pub mod desktop;
pub mod geometry;
pub mod layout;
pub mod mover;
