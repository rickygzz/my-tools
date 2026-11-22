//! Windows and macOS tray, via the `tray-icon` crate.

use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use super::{EXIT_LABEL, ICON_RGBA, ICON_SIZE, TOOLTIP};

/// ID of the "Exit" menu item, used to recognise it when it's clicked.
const EXIT_ID: &str = "exit";

/// The icon in the tray. Dropping it removes the icon.
pub struct Tray {
    _icon: TrayIcon,
}

impl Tray {
    /// Puts the icon in the tray. `on_exit` runs when "Exit" is clicked.
    /// Must be called after the event loop has started (required on macOS).
    pub fn start(on_exit: impl Fn() + Send + Sync + 'static) -> Result<Tray, String> {
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if event.id.0 == EXIT_ID {
                on_exit();
            }
        }));

        let menu = Menu::new();
        let exit_item = MenuItem::with_id(EXIT_ID, EXIT_LABEL, true, None);
        menu.append(&exit_item)
            .map_err(|e| format!("can't build the tray menu: {e}"))?;

        let icon = Icon::from_rgba(ICON_RGBA.to_vec(), ICON_SIZE, ICON_SIZE)
            .map_err(|e| format!("can't load the tray icon: {e}"))?;

        let icon = TrayIconBuilder::new()
            .with_tooltip(TOOLTIP)
            .with_icon(icon)
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false) // menu opens on right click
            .build()
            .map_err(|e| format!("can't create the tray icon: {e}"))?;

        Ok(Tray { _icon: icon })
    }
}
