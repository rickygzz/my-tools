//! `my-tools --show <action>`: lets any desktop bind keys to the app's actions, even without
//! the shortcut portal (Sway and other wlroots desktops).
//!
//! The running copy owns the D-Bus name [`APP_ID`] and offers the standard
//! `org.freedesktop.Application` interface on it. Its `ActivateAction` method does an action
//! by its ID, so `gapplication action com.example.my-tools calculator` works as well.
//!
//! The desktop runs the bound command with `XDG_ACTIVATION_TOKEN` set; passing that on lets
//! the window take the keyboard focus.

use std::collections::HashMap;

use zbus::blocking::Connection;
use zbus::blocking::connection::Builder;
use zbus::zvariant::{OwnedValue, Value};

use super::{Action, Pressed};
use crate::APP_ID;

const INTERFACE: &str = "org.freedesktop.Application";
const TOKEN_KEY: &str = "activation-token";

/// The object path that goes with the app ID: `/com/example/my_tools`.
fn object_path() -> String {
    format!("/{}", APP_ID.replace('.', "/").replace('-', "_"))
}

/// Starts answering `--show` requests. The service stops when the connection is dropped.
pub fn serve(pressed: Pressed) -> Option<Connection> {
    let connection = Builder::session()
        .and_then(|builder| builder.name(APP_ID))
        .and_then(|builder| builder.serve_at(object_path(), Application { pressed }))
        .and_then(Builder::build);
    match connection {
        Ok(connection) => Some(connection),
        Err(error) => {
            // Most likely another copy of the app is running and already owns the name.
            eprintln!("hotkeys: `my-tools --show` won't reach this copy: {error}");
            None
        }
    }
}

/// Asks the running copy to do `action`.
pub fn send(action: Action) -> Result<(), String> {
    let connection = Connection::session().map_err(|e| format!("can't connect to D-Bus: {e}"))?;

    let mut platform_data: HashMap<&str, Value> = HashMap::new();
    if let Ok(token) = std::env::var("XDG_ACTIVATION_TOKEN") {
        platform_data.insert(TOKEN_KEY, Value::from(token));
    }
    connection
        .call_method(
            Some(APP_ID),
            object_path().as_str(),
            Some(INTERFACE),
            "ActivateAction",
            &(action.id(), Vec::<Value>::new(), platform_data),
        )
        .map(|_| ())
        .map_err(|e| match e {
            zbus::Error::MethodError(name, _, _) if name.as_str().ends_with("ServiceUnknown") => {
                "My Tools isn't running".to_string()
            }
            e => format!("couldn't reach My Tools: {e}"),
        })
}

struct Application {
    pressed: Pressed,
}

#[zbus::interface(name = "org.freedesktop.Application")]
impl Application {
    /// The app was started again with no action. There's no main window, so nothing to do.
    fn activate(&self, _platform_data: HashMap<String, OwnedValue>) {}

    /// Files to open: the app doesn't open files.
    fn open(&self, _uris: Vec<String>, _platform_data: HashMap<String, OwnedValue>) {}

    fn activate_action(
        &self,
        name: String,
        _parameter: Vec<OwnedValue>,
        platform_data: HashMap<String, OwnedValue>,
    ) -> zbus::fdo::Result<()> {
        let action = Action::from_id(&name)
            .ok_or_else(|| zbus::fdo::Error::InvalidArgs(format!("no action called {name:?}")))?;
        let token = platform_data
            .get(TOKEN_KEY)
            .and_then(|v| String::try_from(v.try_clone().ok()?).ok());
        (self.pressed)(action, token);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn object_path_is_valid() {
        let path = super::object_path();
        assert!(zbus::zvariant::ObjectPath::try_from(path.as_str()).is_ok(), "{path}");
    }
}
