// In release builds on Windows, don't open a console window behind the tray app.
// Debug builds keep the console so `println!` output is visible while developing.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod calculator;
mod hotkeys;
mod spotlight;
mod tray;
mod zones;

use std::time::Instant;

use winit::application::ApplicationHandler;
use winit::event::{StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::WindowId;

use calculator::Calculator;
use hotkeys::Action;
use spotlight::Spotlight;
use tray::Tray;
use zones::Zones;

/// The app's reverse-DNS ID. Linux desktops use it to find the app's `.desktop` file, which
/// they need to remember its keyboard shortcut and to label its notifications. Keep it in line
/// with `packaging/linux/<APP_ID>.desktop` and `CFBundleIdentifier` in packaging/macos/Info.plist.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const APP_ID: &str = "com.example.my-tools";

/// Whether this is a Linux Wayland session (even if X11 apps can still run in it). Wayland
/// doesn't let apps see other apps' keys, so shortcuts and the spotlight work differently.
fn is_wayland() -> bool {
    cfg!(target_os = "linux")
        && (std::env::var_os("WAYLAND_DISPLAY").is_some()
            || std::env::var("XDG_SESSION_TYPE").is_ok_and(|session| session == "wayland"))
}

/// Events we send into the event loop from other parts of the program.
enum UserEvent {
    /// "Exit" was clicked in the tray menu.
    Exit,
    /// A keyboard shortcut was pressed, or "Calculator" clicked in the tray menu. The token
    /// (Wayland) lets a new window take the keyboard focus.
    Show(Action, Option<String>),
    Spotlight(spotlight::Signal),
    /// Another app's window is being dragged (window zones).
    Drag(zones::drag::Signal),
}

/// The app's state. winit calls the methods below when things happen.
struct App {
    /// Lets the tray menu send its commands into the event loop.
    proxy: EventLoopProxy<UserEvent>,
    /// Kept alive here so the icon stays in the tray.
    tray: Option<Tray>,
    /// The double-Ctrl mouse spotlight.
    spotlight: Spotlight,
    calculator: Calculator,
    zones: Zones,
}

impl ApplicationHandler<UserEvent> for App {
    fn new_events(&mut self, _event_loop: &ActiveEventLoop, cause: StartCause) {
        // The tray icon must be created after the event loop starts (required on macOS).
        if matches!(cause, StartCause::Init) {
            let proxy = self.proxy.clone();
            let on_command = move |command| {
                let event = match command {
                    tray::Command::Calculator => UserEvent::Show(Action::Calculator, None),
                    tray::Command::Exit => UserEvent::Exit,
                };
                let _ = proxy.send_event(event);
            };
            // Drags of other apps' windows, for the zones. On Windows the hook must be set on
            // this thread, which runs the event loop.
            let proxy = self.proxy.clone();
            zones::start_watching(move |signal| {
                let _ = proxy.send_event(UserEvent::Drag(signal));
            });

            match Tray::start(on_command) {
                Ok(tray) => self.tray = Some(tray),
                Err(reason) => {
                    // Without a tray icon there's no Exit menu, so staying open would leave
                    // a process the user can't see or close.
                    eprintln!("my-tools: {reason}");
                    std::process::exit(1);
                }
            }
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Exit => {
                self.tray.take(); // remove the icon from the tray before quitting
                event_loop.exit();
            }
            UserEvent::Show(Action::Calculator, token) => self.calculator.show(event_loop, token),
            UserEvent::Show(Action::Spotlight, token) => self.spotlight.shortcut_pressed(event_loop, token),
            UserEvent::Show(Action::Zone(number), _) => self.zones.run(zones::Command::MoveTo(number.into())),
            UserEvent::Show(Action::NextZone, _) => self.zones.run(zones::Command::Next),
            UserEvent::Show(Action::PreviousZone, _) => self.zones.run(zones::Command::Previous),
            UserEvent::Show(Action::WindowToMouse, _) => self.zones.run(zones::Command::ToMouse),
            UserEvent::Spotlight(signal) => self.spotlight.handle(event_loop, signal),
            UserEvent::Drag(signal) => self.zones.drag(event_loop, signal),
        }
    }

    /// Called when there's nothing left to handle. Sleep until the next event, or until the
    /// spotlight's next animation step or the calculator's next frame (a blinking caret).
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.calculator.wake(Instant::now());
        let next = [self.spotlight.tick(), self.calculator.next_wake()].into_iter().flatten().min();
        match next {
            Some(next) => event_loop.set_control_flow(ControlFlow::WaitUntil(next)),
            None => event_loop.set_control_flow(ControlFlow::Wait),
        }
    }

    fn window_event(&mut self, _event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if self.calculator.owns(id) {
            self.calculator.window_event(&event);
        } else if self.zones.owns(id) {
            self.zones.window_event(&event);
        } else {
            self.spotlight.window_event(id, &event);
        }
    }

    // winit requires this one, but there's nothing to do here.
    fn resumed(&mut self, _event_loop: &ActiveEventLoop) {}

    /// Close the windows while the event loop still exists: on Wayland the spotlight's pixel
    /// buffers, and the calculator's GPU surface, live on its connection to the compositor.
    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        self.spotlight = Spotlight::default();
        self.calculator.close();
        self.zones.close();
    }
}

fn main() {
    // `my-tools --show calculator` asks the running copy, then exits.
    if let Some(code) = hotkeys::run_command_line() {
        std::process::exit(code);
    }

    let mut builder = EventLoop::<UserEvent>::with_user_event();

    // On macOS, run as a menu-bar-only app (no Dock icon).
    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
        builder.with_activation_policy(ActivationPolicy::Accessory);
    }

    let event_loop = builder.build().expect("failed to create event loop");

    // Listen for the double Ctrl tap in every program, and forward it into the event loop.
    let proxy = event_loop.create_proxy();
    spotlight::start_listener(move |signal| {
        let _ = proxy.send_event(UserEvent::Spotlight(signal));
    });

    // Keyboard shortcuts in every app; they stay registered while `_hotkeys` lives. The
    // spotlight only needs one where the double Ctrl tap can't be seen (Wayland), and the
    // zones only work where apps can move other apps' windows (not Wayland).
    let actions: Vec<Action> = Action::ALL
        .into_iter()
        .filter(|&action| action != Action::Spotlight || spotlight::needs_shortcut())
        .filter(|&action| !action.is_zone() || !is_wayland())
        .collect();
    let proxy = event_loop.create_proxy();
    let _hotkeys = hotkeys::start(&actions, move |action, token| {
        let _ = proxy.send_event(UserEvent::Show(action, token));
    });

    let mut app = App {
        proxy: event_loop.create_proxy(),
        tray: None,
        spotlight: Spotlight::default(),
        calculator: Calculator::default(),
        zones: Zones::default(),
    };
    event_loop.run_app(&mut app).expect("event loop failed");
}
