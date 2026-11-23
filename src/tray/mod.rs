//! The tray icon and its right-click menu: **Calculator**, **Edit window zones...** and **Exit**.
//!
//! Each backend exposes the same small API: `Tray::start(on_command)` puts the icon in the
//! tray and calls `on_command` when a menu entry is clicked, and dropping the `Tray` removes it.
//!
//! - Windows and macOS use the `tray-icon` crate.
//! - Linux uses `ksni`, which speaks the StatusNotifierItem D-Bus protocol itself, so no
//!   GTK or libayatana-appindicator is needed.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::Tray;

#[cfg(not(target_os = "linux"))]
mod windows_macos;
#[cfg(not(target_os = "linux"))]
pub use windows_macos::Tray;

/// A menu entry that was clicked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Calculator,
    EditZones,
    Exit,
}

/// Shown when hovering over the icon.
const TOOLTIP: &str = "My Tools";
const CALCULATOR_LABEL: &str = "Calculator";
const ZONES_LABEL: &str = "Edit window zones...";
const EXIT_LABEL: &str = "Exit";

/// Raw 32x32 RGBA pixels of the app icon, generated from `assets/icon.svg` by `icon-gen`
/// and compiled into the program, so no image file needs to ship next to it.
const ICON_RGBA: &[u8] = include_bytes!("../../assets/icons/tray-32.rgba");
const ICON_SIZE: u32 = 32;
