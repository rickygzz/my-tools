//! Windows and macOS tray, via the `tray-icon` crate.

use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use super::{CALCULATOR_LABEL, Command, EXIT_LABEL, ICON_RGBA, ICON_SIZE, TOOLTIP};

/// IDs of the menu items, used to recognise them when they're clicked.
const CALCULATOR_ID: &str = "calculator";
const EXIT_ID: &str = "exit";

/// The icon in the tray. Dropping it removes the icon.
pub struct Tray {
    _icon: TrayIcon,
}

impl Tray {
    /// Puts the icon in the tray. `on_command` runs when a menu entry is clicked.
    /// Must be called after the event loop has started (required on macOS).
    pub fn start(on_command: impl Fn(Command) + Send + Sync + 'static) -> Result<Tray, String> {
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| match event.id.0.as_str() {
            CALCULATOR_ID => on_command(Command::Calculator),
            EXIT_ID => on_command(Command::Exit),
            _ => {}
        }));

        let menu = Menu::new();
        let calculator_item = MenuItem::with_id(CALCULATOR_ID, CALCULATOR_LABEL, true, None);
        let exit_item = MenuItem::with_id(EXIT_ID, EXIT_LABEL, true, None);
        menu.append_items(&[&calculator_item, &PredefinedMenuItem::separator(), &exit_item])
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
