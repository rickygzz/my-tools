//! Linux tray, via `ksni`.
//!
//! Linux tray icons are StatusNotifierItems: the app publishes the icon and menu on D-Bus
//! and registers with the `org.kde.StatusNotifierWatcher` service, which the desktop runs.
//! KDE, Cinnamon, XFCE, Ubuntu's GNOME etc. provide it; stock GNOME only does when the
//! "AppIndicator and KStatusNotifierItem Support" extension is enabled, so on GNOME we
//! enable or install that extension if needed.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use ksni::blocking::{Handle, TrayMethods};
use zbus::blocking::Connection;
use zbus::blocking::fdo::DBusProxy;
use zbus::names::BusName;
use zbus::zvariant::{OwnedValue, Value};

use super::{EXIT_LABEL, ICON_RGBA, TOOLTIP, ICON_SIZE};

const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const SHELL_EXTENSIONS: &str = "org.gnome.Shell.Extensions";
const EXTENSION_UUID: &str = "appindicatorsupport@rgcjonas.gmail.com";
/// How long to wait for the tray service after enabling/installing the extension.
const WATCHER_WAIT: Duration = Duration::from_secs(10);

/// The icon in the tray. Dropping it removes the icon.
pub struct Tray {
    handle: Handle<SniTray>,
}

impl Tray {
    /// Puts the icon in the tray. `on_exit` runs when "Exit" is clicked.
    ///
    /// If the desktop has no tray, first tries to fix that on GNOME. If that fails, shows a
    /// desktop notification explaining how to fix it by hand, and returns an error.
    pub fn start(on_exit: impl Fn() + Send + Sync + 'static) -> Result<Tray, String> {
        let bus = Connection::session().map_err(|e| format!("can't connect to D-Bus: {e}"))?;

        if !ensure_host(&bus) {
            notify_no_tray(&bus);
            return Err("the desktop has no system tray".into());
        }

        let tray = SniTray {
            icon: argb_icon(),
            on_exit: Box::new(on_exit),
        };
        match tray.spawn() {
            Ok(handle) => Ok(Tray { handle }),
            Err(e) => {
                notify_no_tray(&bus);
                Err(format!("can't show the tray icon: {e}"))
            }
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        self.handle.shutdown().wait();
    }
}

/// What `ksni` publishes on D-Bus.
struct SniTray {
    icon: ksni::Icon,
    on_exit: Box<dyn Fn() + Send + Sync>,
}

impl ksni::Tray for SniTray {
    fn id(&self) -> String {
        env!("CARGO_PKG_NAME").into()
    }

    fn title(&self) -> String {
        TOOLTIP.into()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        vec![self.icon.clone()]
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: TOOLTIP.into(),
            ..Default::default()
        }
    }

    // Left click does nothing (`activate` isn't overridden); the menu opens on right click.
    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        vec![
            ksni::menu::StandardItem {
                label: EXIT_LABEL.into(),
                activate: Box::new(|tray: &mut Self| (tray.on_exit)()),
                ..Default::default()
            }
            .into(),
        ]
    }
}

/// Converts the embedded RGBA pixels to the ARGB (big-endian) layout D-Bus trays expect.
fn argb_icon() -> ksni::Icon {
    let (pixels, _) = ICON_RGBA.as_chunks::<4>();
    let data = pixels
        .iter()
        .flat_map(|&[r, g, b, a]| [a, r, g, b])
        .collect();
    ksni::Icon {
        width: ICON_SIZE as i32,
        height: ICON_SIZE as i32,
        data,
    }
}

/// Returns true if a tray host is available, setting one up on GNOME if needed:
/// enables the AppIndicator extension if it's installed but off, otherwise asks
/// GNOME to install it from extensions.gnome.org (GNOME shows its own confirmation).
fn ensure_host(bus: &Connection) -> bool {
    // If the D-Bus daemon can't be asked, let ksni try anyway and report what it finds.
    if has_owner(bus, WATCHER).unwrap_or(true) {
        return true;
    }
    // Not GNOME Shell (or a version without the extensions API): nothing we can fix.
    if !has_owner(bus, SHELL_EXTENSIONS).unwrap_or(false) {
        return false;
    }
    enable_or_install_extension(bus) && wait_for_watcher(bus)
}

/// Enables the extension if installed, otherwise requests installation.
/// Returns true if the extension should now be running.
fn enable_or_install_extension(bus: &Connection) -> bool {
    let Some(info) = shell_call(bus, "GetExtensionInfo")
        .and_then(|reply| reply.body().deserialize::<HashMap<String, OwnedValue>>().ok())
    else {
        return false;
    };

    // GNOME answers with an empty dictionary for extensions that aren't installed.
    if !info.contains_key("uuid") {
        // Blocks until the user answers GNOME's "Install extension?" prompt.
        // GNOME downloads, enables and loads it right away; no logout needed.
        return shell_call(bus, "InstallRemoteExtension")
            .and_then(|reply| reply.body().deserialize::<String>().ok())
            .is_some_and(|result| result == "successful");
    }

    let enabled = info
        .get("enabled")
        .and_then(|v| bool::try_from(&**v).ok())
        .unwrap_or(false);
    if enabled {
        // Installed and enabled, yet no tray service: it's failing to load
        // (e.g. not compatible with this GNOME version). Nothing more we can do.
        return false;
    }
    shell_call(bus, "EnableExtension")
        .and_then(|reply| reply.body().deserialize::<bool>().ok())
        .unwrap_or(false)
}

/// Polls until the tray service appears, up to `WATCHER_WAIT`.
fn wait_for_watcher(bus: &Connection) -> bool {
    let deadline = Instant::now() + WATCHER_WAIT;
    while Instant::now() < deadline {
        if has_owner(bus, WATCHER).unwrap_or(false) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    false
}

/// Asks the D-Bus daemon whether some process owns `name`.
fn has_owner(bus: &Connection, name: &str) -> Option<bool> {
    let name = BusName::try_from(name).ok()?;
    DBusProxy::new(bus).ok()?.name_has_owner(name).ok()
}

/// Calls a GNOME Shell extensions method that takes the extension's UUID.
fn shell_call(bus: &Connection, method: &str) -> Option<zbus::Message> {
    bus.call_method(
        Some(SHELL_EXTENSIONS),
        "/org/gnome/Shell/Extensions",
        Some(SHELL_EXTENSIONS),
        method,
        &(EXTENSION_UUID,),
    )
    .ok()
}

/// Tells the user why no tray icon will appear and how to fix it by hand, with a desktop
/// notification (there's no window to show it in) and on stderr.
fn notify_no_tray(bus: &Connection) {
    const SUMMARY: &str = "My Tools can't show its tray icon";
    const BODY: &str = "Your desktop has no system tray. On GNOME, install and enable the \
        \"AppIndicator and KStatusNotifierItem Support\" extension, from your distribution's \
        packages or from extensions.gnome.org/extension/615/appindicator-support/ \
        Then start My Tools again.";

    eprintln!("{SUMMARY}. {BODY}");

    let mut hints: HashMap<&str, Value> = HashMap::new();
    hints.insert("urgency", Value::U8(2)); // critical: stays on screen until dismissed
    hints.insert("desktop-entry", Value::from(env!("CARGO_PKG_NAME")));
    let _ = bus.call_method(
        Some("org.freedesktop.Notifications"),
        "/org/freedesktop/Notifications",
        Some("org.freedesktop.Notifications"),
        "Notify",
        &(
            TOOLTIP,            // app name
            0u32,               // not replacing an earlier notification
            "dialog-warning",   // icon
            SUMMARY,
            BODY,
            Vec::<&str>::new(), // no action buttons
            hints,
            0i32,               // never expire
        ),
    );
}
