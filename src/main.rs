// In release builds on Windows, don't open a console window behind the tray app.
// Debug builds keep the console so `println!` output is visible while developing.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod spotlight;

use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};
use winit::application::ApplicationHandler;
use winit::event::{StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::WindowId;

use spotlight::Spotlight;

/// ID of the "Exit" menu item, used to recognise it when it's clicked.
const EXIT_ID: &str = "exit";

/// Events we send into the event loop from other parts of the program.
enum UserEvent {
    Menu(MenuEvent),
    Spotlight(spotlight::Signal),
}

/// The app's state. winit calls the methods below when things happen.
#[derive(Default)]
struct App {
    /// Kept alive here so the icon stays in the tray. (On Linux it lives on the GTK thread instead.)
    tray: Option<TrayIcon>,
    /// The double-Ctrl mouse spotlight (Windows only for now).
    spotlight: Spotlight,
}

impl ApplicationHandler<UserEvent> for App {
    fn new_events(&mut self, _event_loop: &ActiveEventLoop, cause: StartCause) {
        // The tray icon must be created after the event loop starts (required on macOS).
        if matches!(cause, StartCause::Init) && !cfg!(target_os = "linux") {
            self.tray = Some(build_tray());
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Menu(event) if event.id.0 == EXIT_ID => {
                self.tray.take(); // remove the icon from the tray before quitting
                event_loop.exit();
            }
            UserEvent::Menu(_) => {}
            UserEvent::Spotlight(signal) => self.spotlight.handle(event_loop, signal),
        }
    }

    /// Called when there's nothing left to handle. While the spotlight shows, wake up again
    /// shortly to animate it; otherwise sleep until the next event.
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        match self.spotlight.tick() {
            Some(next) => event_loop.set_control_flow(ControlFlow::WaitUntil(next)),
            None => event_loop.set_control_flow(ControlFlow::Wait),
        }
    }

    /// The only window is the spotlight's overlay.
    fn window_event(&mut self, _event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        self.spotlight.window_event(id, &event);
    }

    // winit requires this one, but there's nothing to do here.
    fn resumed(&mut self, _event_loop: &ActiveEventLoop) {}
}

fn main() {
    let mut builder = EventLoop::<UserEvent>::with_user_event();

    // On macOS, run as a menu-bar-only app (no Dock icon).
    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
        builder.with_activation_policy(ActivationPolicy::Accessory);
    }

    let event_loop = builder.build().expect("failed to create event loop");

    // Forward menu clicks into the event loop so we handle them in one place.
    let proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |event| {
        let _ = proxy.send_event(UserEvent::Menu(event));
    }));

    // Listen for the double Ctrl tap in every program, and forward it the same way.
    let proxy = event_loop.create_proxy();
    spotlight::start_listener(move |signal| {
        let _ = proxy.send_event(UserEvent::Spotlight(signal));
    });

    // On Linux the tray icon is drawn by GTK, which needs its own thread and loop.
    #[cfg(target_os = "linux")]
    std::thread::spawn(|| {
        gtk::init().expect("failed to initialise GTK");
        if !linux_tray::ensure_host() {
            // Without a tray host the icon (and its Exit menu) would be invisible,
            // leaving a process the user can't see or close. Explain, then quit.
            linux_tray::show_missing_host_dialog();
            std::process::exit(1);
        }
        let _tray = build_tray(); // stays alive while gtk::main() runs
        gtk::main();
    });

    event_loop
        .run_app(&mut App::default())
        .expect("event loop failed");
}

/// Creates the tray icon with its right-click menu.
fn build_tray() -> TrayIcon {
    let menu = Menu::new();
    let exit_item = MenuItem::with_id(EXIT_ID, "Exit", true, None);
    menu.append(&exit_item).expect("failed to build tray menu");

    TrayIconBuilder::new()
        .with_tooltip("My Tools")
        .with_icon(make_icon())
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false) // menu opens on right click
        .build()
        .expect("failed to create tray icon")
}

/// Making sure Linux desktops can show the tray icon.
///
/// Linux tray icons register with the `org.kde.StatusNotifierWatcher` D-Bus service.
/// KDE, Cinnamon, XFCE, Ubuntu's GNOME etc. provide it; stock GNOME only does when the
/// "AppIndicator and KStatusNotifierItem Support" extension is enabled.
#[cfg(target_os = "linux")]
mod linux_tray {
    use gtk::gio::{self, BusType, Cancellable, DBusCallFlags, DBusConnection};
    use gtk::glib::{ToVariant, Variant, VariantTy};
    use gtk::prelude::*;
    use std::time::{Duration, Instant};

    const WATCHER: &str = "org.kde.StatusNotifierWatcher";
    const SHELL_EXTENSIONS: &str = "org.gnome.Shell.Extensions";
    const EXTENSION_UUID: &str = "appindicatorsupport@rgcjonas.gmail.com";
    /// How long to wait for the tray service after enabling/installing the extension.
    const WATCHER_WAIT: Duration = Duration::from_secs(10);
    /// D-Bus timeout meaning "wait forever" (G_MAXINT), for calls that wait on the user.
    const NO_TIMEOUT: i32 = i32::MAX;

    /// Returns true if a tray host is available, setting one up on GNOME if needed:
    /// enables the AppIndicator extension if it's installed but off, otherwise asks
    /// GNOME to install it from extensions.gnome.org (GNOME shows its own confirmation).
    /// If D-Bus can't be queried at all, assumes a host exists rather than block startup.
    pub fn ensure_host() -> bool {
        let Ok(bus) = gio::bus_get_sync(BusType::Session, None::<&Cancellable>) else {
            return true;
        };
        if has_owner(&bus, WATCHER).unwrap_or(true) {
            return true;
        }
        // Not GNOME Shell (or a version without the extensions API): nothing we can fix.
        if !has_owner(&bus, SHELL_EXTENSIONS).unwrap_or(false) {
            return false;
        }
        enable_or_install_extension(&bus) && wait_for_watcher(&bus)
    }

    /// Enables the extension if installed, otherwise requests installation.
    /// Returns true if the extension should now be running.
    fn enable_or_install_extension(bus: &DBusConnection) -> bool {
        let Some(info) = shell_call(bus, "GetExtensionInfo", "(a{sv})", -1) else {
            return false;
        };
        let info = gtk::glib::VariantDict::new(Some(&info.child_value(0)));
        let installed = info.contains("uuid");

        if !installed {
            // Blocks until the user answers GNOME's "Install extension?" prompt.
            // GNOME downloads, enables and loads it right away; no logout needed.
            return shell_call(bus, "InstallRemoteExtension", "(s)", NO_TIMEOUT)
                .and_then(|reply| reply.get::<(String,)>())
                .is_some_and(|(result,)| result == "successful");
        }

        let enabled = info.lookup::<bool>("enabled").ok().flatten().unwrap_or(false);
        if enabled {
            // Installed and enabled, yet no tray service: it's failing to load
            // (e.g. not compatible with this GNOME version). Nothing more we can do.
            return false;
        }
        shell_call(bus, "EnableExtension", "(b)", -1)
            .and_then(|reply| reply.get::<(bool,)>())
            .is_some_and(|(ok,)| ok)
    }

    /// Polls until the tray service appears, up to `WATCHER_WAIT`.
    fn wait_for_watcher(bus: &DBusConnection) -> bool {
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
    fn has_owner(bus: &DBusConnection, name: &str) -> Option<bool> {
        call(
            bus,
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "NameHasOwner",
            (name,).to_variant(),
            "(b)",
            -1,
        )?
        .get::<(bool,)>()
        .map(|(owned,)| owned)
    }

    /// Calls a GNOME Shell extensions method that takes the extension's UUID.
    fn shell_call(bus: &DBusConnection, method: &str, reply: &str, timeout_ms: i32) -> Option<Variant> {
        call(
            bus,
            SHELL_EXTENSIONS,
            "/org/gnome/Shell/Extensions",
            SHELL_EXTENSIONS,
            method,
            (EXTENSION_UUID,).to_variant(),
            reply,
            timeout_ms,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn call(
        bus: &DBusConnection,
        dest: &str,
        path: &str,
        interface: &str,
        method: &str,
        args: Variant,
        reply: &str,
        timeout_ms: i32,
    ) -> Option<Variant> {
        bus.call_sync(
            Some(dest),
            path,
            interface,
            method,
            Some(&args),
            Some(VariantTy::new(reply).ok()?),
            DBusCallFlags::NONE,
            timeout_ms,
            None::<&Cancellable>,
        )
        .ok()
    }

    /// Tells the user why no tray icon will appear and how to fix it by hand.
    pub fn show_missing_host_dialog() {
        let dialog = gtk::MessageDialog::new(
            None::<&gtk::Window>,
            gtk::DialogFlags::MODAL,
            gtk::MessageType::Warning,
            gtk::ButtonsType::Ok,
            "My Tools can't show its tray icon",
        );
        dialog.set_title("My Tools");
        dialog.set_secondary_text(Some(
            "Your desktop has no system tray.\n\n\
             On GNOME, install and enable the \"AppIndicator and KStatusNotifierItem \
             Support\" extension, from your distribution's packages or from \
             extensions.gnome.org/extension/615/appindicator-support/\n\n\
             Then start My Tools again.",
        ));
        dialog.run();
        dialog.close();
    }
}

/// Raw 32x32 RGBA pixels of the app icon, generated from `assets/icon.svg` by `icon-gen`
/// and compiled into the program, so no image file needs to ship next to it.
const TRAY_ICON_RGBA: &[u8] = include_bytes!("../assets/icons/tray-32.rgba");
const TRAY_ICON_SIZE: u32 = 32;

/// Loads the tray icon from the embedded pixels.
fn make_icon() -> Icon {
    Icon::from_rgba(TRAY_ICON_RGBA.to_vec(), TRAY_ICON_SIZE, TRAY_ICON_SIZE)
        .expect("failed to create icon")
}
