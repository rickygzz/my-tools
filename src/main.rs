// In release builds on Windows, don't open a console window behind the tray app.
// Debug builds keep the console so `println!` output is visible while developing.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod spotlight;
mod tray;

use winit::application::ApplicationHandler;
use winit::event::{StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::WindowId;

use spotlight::Spotlight;
use tray::Tray;

/// Events we send into the event loop from other parts of the program.
enum UserEvent {
    /// "Exit" was clicked in the tray menu.
    Exit,
    Spotlight(spotlight::Signal),
}

/// The app's state. winit calls the methods below when things happen.
struct App {
    /// Lets the tray menu send `UserEvent::Exit` into the event loop.
    proxy: EventLoopProxy<UserEvent>,
    /// Kept alive here so the icon stays in the tray.
    tray: Option<Tray>,
    /// The double-Ctrl mouse spotlight.
    spotlight: Spotlight,
}

impl ApplicationHandler<UserEvent> for App {
    fn new_events(&mut self, _event_loop: &ActiveEventLoop, cause: StartCause) {
        // The tray icon must be created after the event loop starts (required on macOS).
        if matches!(cause, StartCause::Init) {
            let proxy = self.proxy.clone();
            match Tray::start(move || {
                let _ = proxy.send_event(UserEvent::Exit);
            }) {
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

    // Listen for the double Ctrl tap in every program, and forward it into the event loop.
    let proxy = event_loop.create_proxy();
    spotlight::start_listener(move |signal| {
        let _ = proxy.send_event(UserEvent::Spotlight(signal));
    });

    let mut app = App {
        proxy: event_loop.create_proxy(),
        tray: None,
        spotlight: Spotlight::default(),
    };
    event_loop.run_app(&mut app).expect("event loop failed");
}
