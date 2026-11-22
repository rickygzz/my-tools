// In release builds on Windows, don't open a console window behind the tray app.
// Debug builds keep the console so `println!` output is visible while developing.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};
use winit::application::ApplicationHandler;
use winit::event::{StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::WindowId;

/// ID of the "Exit" menu item, used to recognise it when it's clicked.
const EXIT_ID: &str = "exit";

/// Events we send into the event loop from other parts of the program.
enum UserEvent {
    Menu(MenuEvent),
}

/// The app's state. winit calls the methods below when things happen.
#[derive(Default)]
struct App {
    /// Kept alive here so the icon stays in the tray. (On Linux it lives on the GTK thread instead.)
    tray: Option<TrayIcon>,
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
        }
    }

    // winit requires these two, but this app has no windows, so they do nothing.
    fn resumed(&mut self, _event_loop: &ActiveEventLoop) {}
    fn window_event(&mut self, _event_loop: &ActiveEventLoop, _id: WindowId, _event: WindowEvent) {}
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

    // On Linux the tray icon is drawn by GTK, which needs its own thread and loop.
    #[cfg(target_os = "linux")]
    std::thread::spawn(|| {
        gtk::init().expect("failed to initialise GTK");
        if !linux_tray_host_available() {
            // Without a tray host the icon (and its Exit menu) would be invisible,
            // leaving a process the user can't see or close. Explain, then quit.
            show_missing_tray_host_dialog();
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

/// Checks whether the desktop can show tray icons. Linux tray icons register with the
/// `org.kde.StatusNotifierWatcher` D-Bus service; KDE, Cinnamon, XFCE etc. provide it,
/// but stock GNOME only does when the AppIndicator extension is enabled.
/// If D-Bus can't be queried we assume a host exists rather than block startup.
#[cfg(target_os = "linux")]
fn linux_tray_host_available() -> bool {
    use gtk::gio::{self, BusType, Cancellable, DBusCallFlags};
    use gtk::glib::{ToVariant, VariantTy};

    let Ok(bus) = gio::bus_get_sync(BusType::Session, None::<&Cancellable>) else {
        return true;
    };
    bus.call_sync(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
        "NameHasOwner",
        Some(&("org.kde.StatusNotifierWatcher",).to_variant()),
        Some(VariantTy::new("(b)").unwrap()),
        DBusCallFlags::NONE,
        -1,
        None::<&Cancellable>,
    )
    .ok()
    .and_then(|reply| reply.get::<(bool,)>())
    .is_none_or(|(has_owner,)| has_owner)
}

/// Tells the user why no tray icon will appear and how to fix it.
#[cfg(target_os = "linux")]
fn show_missing_tray_host_dialog() {
    use gtk::prelude::*;

    let dialog = gtk::MessageDialog::new(
        None::<&gtk::Window>,
        gtk::DialogFlags::MODAL,
        gtk::MessageType::Warning,
        gtk::ButtonsType::Ok,
        "My Tools can't show its tray icon",
    );
    dialog.set_title("My Tools");
    dialog.set_secondary_text(Some(
        "Your desktop has no system tray. On GNOME, install the AppIndicator extension:\n\n\
         sudo dnf install gnome-shell-extension-appindicator\n\n\
         Log out and back in, then run:\n\n\
         gnome-extensions enable appindicatorsupport@rgcjonas.gmail.com",
    ));
    dialog.run();
    dialog.close();
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
