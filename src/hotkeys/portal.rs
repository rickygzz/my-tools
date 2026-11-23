//! Shortcuts through the desktop's *GlobalShortcuts* portal, for Wayland.
//!
//! The portal (part of xdg-desktop-portal) lets an app ask for shortcuts that work in every
//! app, while the desktop stays in charge of the keys: the first time, it shows a dialog where
//! the user confirms or changes the suggested keys, and it remembers the answer. Needs GNOME
//! 48+ or KDE Plasma 5.27+.
//!
//! All shortcuts are bound in one call: the portal allows one per session, and the desktop
//! shows its dialog once for the whole list.
//!
//! Portal calls answer through a separate "Request" object that later sends a `Response`
//! signal; `request` below handles that dance.

use std::collections::HashMap;
use std::error::Error;
use std::sync::atomic::{AtomicU32, Ordering};

use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{ObjectPath, OwnedValue, Value};

use super::{Action, Pressed, command_for};
use crate::APP_ID;

const PORTAL: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";

type Options<'a> = HashMap<&'a str, Value<'a>>;
type Results = HashMap<String, OwnedValue>;

/// Binds the shortcuts on a thread of their own, which then waits for presses.
pub fn start(actions: Vec<Action>, pressed: Pressed) {
    if actions.is_empty() {
        return;
    }
    let spawned = std::thread::Builder::new().name("hotkeys-portal".into()).spawn(move || {
        if let Err(error) = listen(&actions, &pressed) {
            eprintln!("hotkeys: no keyboard shortcuts from the desktop: {error}");
            eprintln!("hotkeys: bind these commands to keys in your desktop's settings instead:");
            for &action in &actions {
                eprintln!("  {}: {}", action.description(), command_for(action));
            }
        }
    });
    if let Err(error) = spawned {
        eprintln!("hotkeys: could not start the shortcut thread: {error}");
    }
}

/// Binds the shortcuts, then calls `pressed` every time one is pressed. Runs until the
/// connection breaks.
fn listen(actions: &[Action], pressed: &Pressed) -> Result<(), Box<dyn Error>> {
    let conn = Connection::session()?;

    register_app_id(&conn);

    let shortcuts = Proxy::new(&conn, PORTAL, PORTAL_PATH, "org.freedesktop.portal.GlobalShortcuts")?;
    // Subscribe before binding, so no press is missed.
    let signals = shortcuts.receive_all_signals()?;

    let session = request(&conn, |token| {
        let options = Options::from([
            ("handle_token", Value::from(token)),
            ("session_handle_token", Value::from("my_tools")),
        ]);
        shortcuts.call_method("CreateSession", &(options,))
    })
    .map_err(|e| format!("the desktop has no shortcut portal ({e})"))?;
    let session: String = session
        .get("session_handle")
        .and_then(|v| String::try_from(v.try_clone().ok()?).ok())
        .ok_or("the portal didn't return a session")?;
    let session = ObjectPath::try_from(session.as_str())?;

    // Shows the desktop's dialog the first time; later runs reuse the remembered keys.
    let list: Vec<(&str, Options)> = actions
        .iter()
        .map(|&action| {
            let properties = Options::from([
                ("description", Value::from(action.description())),
                ("preferred_trigger", Value::from(portal_trigger(action.keys()))),
            ]);
            (action.id(), properties)
        })
        .collect();
    let bound = request(&conn, |token| {
        let options = Options::from([("handle_token", Value::from(token))]);
        shortcuts.call_method("BindShortcuts", &(&session, list, "", options))
    })
    .map_err(|e| format!("the shortcuts weren't set up: {e}"))?;
    // GNOME also sends `ShortcutsChanged` right after binding: only print what's new.
    let mut shown = Vec::new();
    match bound.get("shortcuts") {
        Some(list) => show_triggers(triggers(list), &mut shown),
        None => eprintln!("hotkeys: shortcuts set up; see your desktop's keyboard settings"),
    }

    for message in signals {
        let header = message.header();
        match header.member().map(|member| member.as_str()) {
            Some("Activated") => {
                let Ok((_session, id, _time, options)) =
                    message.body().deserialize::<(ObjectPath<'_>, String, u64, Results)>()
                else {
                    continue;
                };
                if let Some(action) = Action::from_id(&id) {
                    let token = options
                        .get("activation_token")
                        .and_then(|v| String::try_from(v.try_clone().ok()?).ok());
                    pressed(action, token);
                }
            }
            // The user changed the keys in the desktop's settings.
            Some("ShortcutsChanged") => show_triggers(changed_triggers(&message), &mut shown),
            _ => {}
        }
    }
    Err("lost the connection to the portal".into())
}

/// Turns `Ctrl+Alt+C` into the portal's format (the XDG shortcuts spec): `CTRL+ALT+c`.
fn portal_trigger(keys: &str) -> String {
    let mut parts: Vec<String> = keys.split('+').map(str::to_owned).collect();
    let key = parts.pop().unwrap_or_default();
    let mut out: Vec<String> = parts
        .iter()
        .map(|modifier| match modifier.to_uppercase().as_str() {
            "SUPER" | "META" | "CMD" => "LOGO".to_owned(),
            other => other.to_owned(),
        })
        .collect();
    // Keys are xkb key names, where letters are lowercase.
    out.push(if key.chars().count() == 1 { key.to_lowercase() } else { key });
    out.join("+")
}

/// Tells the portal our app ID. Apps outside Flatpak/Snap have none by default, so the portal
/// would file the shortcut under whichever app started us (a terminal, an editor...).
/// This must be the first portal call on the connection.
///
/// The portal only accepts IDs with an installed `.desktop` file. `install.sh` puts one in
/// place; when running from a build folder there's none, so we write a hidden one, pointing
/// at this executable, and try again while the desktop notices it.
fn register_app_id(conn: &Connection) {
    let register = || {
        conn.call_method(
            Some(PORTAL),
            PORTAL_PATH,
            Some("org.freedesktop.host.portal.Registry"),
            "Register",
            &(APP_ID, Options::new()),
        )
    };
    let Err(error) = register() else {
        return;
    };
    // Older portals don't have the Registry: nothing to do.
    if !error.to_string().contains("App info not found") {
        return;
    }
    if let Err(error) = write_desktop_entry() {
        eprintln!("hotkeys: could not add a desktop entry for the shortcuts: {error}");
        return;
    }
    for _ in 0..20 {
        std::thread::sleep(std::time::Duration::from_millis(250));
        if register().is_ok() {
            return;
        }
    }
    eprintln!("hotkeys: the desktop didn't accept the app ID {APP_ID}");
}

/// Writes `~/.local/share/applications/<APP_ID>.desktop`, hidden from the app menu, pointing
/// at this executable. Never overwrites an existing one.
fn write_desktop_entry() -> Result<(), Box<dyn Error>> {
    let data_dir = match std::env::var_os("XDG_DATA_HOME") {
        Some(dir) if !dir.is_empty() => std::path::PathBuf::from(dir),
        _ => std::path::PathBuf::from(std::env::var_os("HOME").ok_or("no HOME")?)
            .join(".local/share"),
    };
    let path = data_dir.join("applications").join(format!("{APP_ID}.desktop"));
    if path.exists() {
        return Ok(());
    }
    let exe = std::env::current_exe()?;
    let exe = exe.to_str().ok_or("the executable's path isn't valid UTF-8")?;
    // Desktop-entry quoting: inside double quotes, escape \ " ` and $.
    let quoted: String = exe
        .chars()
        .flat_map(|c| match c {
            '\\' | '"' | '`' | '$' => vec!['\\', c],
            c => vec![c],
        })
        .collect();
    std::fs::create_dir_all(path.parent().ok_or("bad path")?)?;
    std::fs::write(
        &path,
        format!(
            "[Desktop Entry]\nType=Application\nName=My Tools\nExec=\"{quoted}\"\n\
             Icon=my-tools\nTerminal=false\nNoDisplay=true\n"
        ),
    )?;
    eprintln!("hotkeys: added {} so the desktop can tell My Tools apart", path.display());
    Ok(())
}

/// Calls a portal method that answers through a Request object, and waits for the answer.
/// `call` gets the `handle_token` to pass in the method's options.
fn request(
    conn: &Connection,
    call: impl FnOnce(&str) -> zbus::Result<zbus::Message>,
) -> Result<Results, Box<dyn Error>> {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let token = format!("my_tools_{}", NEXT.fetch_add(1, Ordering::Relaxed));

    // The Request's path is predictable, so we can listen for its answer before calling.
    let sender = conn
        .unique_name()
        .ok_or("no D-Bus name")?
        .trim_start_matches(':')
        .replace('.', "_");
    let path = format!("{PORTAL_PATH}/request/{sender}/{token}");
    let request = Proxy::new(conn, PORTAL, path, "org.freedesktop.portal.Request")?;
    let mut responses = request.receive_signal("Response")?;

    call(&token)?;

    let message = responses.next().ok_or("no answer from the portal")?;
    let (code, results) = message.body().deserialize::<(u32, Results)>()?;
    match code {
        0 => Ok(results),
        1 => Err("cancelled".into()),
        _ => Err("the desktop refused".into()),
    }
}

/// Prints `lines` unless they're the ones printed last.
fn show_triggers(lines: Vec<String>, shown: &mut Vec<String>) {
    if !lines.is_empty() && lines != *shown {
        for line in &lines {
            eprintln!("hotkeys: {line}");
        }
        *shown = lines;
    }
}

/// The keys from a `ShortcutsChanged` signal, whose body is (session, shortcuts).
fn changed_triggers(message: &zbus::Message) -> Vec<String> {
    let body = message.body();
    match body.deserialize::<zbus::zvariant::Structure<'_>>().as_ref().map(|b| b.fields()) {
        Ok([_session, list]) => triggers(list),
        _ => Vec::new(),
    }
}

/// The keys the desktop assigned to each shortcut, as it describes them (e.g. "Press
/// <Control><Alt>c"), one line per shortcut. `list` is the portal's array of (id, properties).
fn triggers(list: &Value<'_>) -> Vec<String> {
    let Value::Array(shortcuts) = list else {
        return Vec::new();
    };
    shortcuts
        .iter()
        .filter_map(|shortcut| {
            let Value::Structure(shortcut) = shortcut else {
                return None;
            };
            let [Value::Str(id), Value::Dict(properties)] = shortcut.fields() else {
                return None;
            };
            let action = Action::from_id(id.as_str())?;
            Some(match properties.get::<_, String>(&"trigger_description").ok().flatten() {
                Some(keys) if !keys.is_empty() => format!("{}: {keys}", action.description()),
                _ => format!("{}: no keys (set them in your desktop's settings)", action.description()),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_shortcuts_changed() {
        let properties = HashMap::from([("trigger_description", Value::from("Press <Control><Alt>c"))]);
        let session = ObjectPath::try_from("/org/freedesktop/portal/desktop/session/1_2/s").unwrap();
        let message = zbus::Message::signal(PORTAL_PATH, "org.freedesktop.portal.GlobalShortcuts", "ShortcutsChanged")
            .unwrap()
            .build(&(session, vec![("calculator", properties)]))
            .unwrap();
        assert_eq!(changed_triggers(&message), ["Show the calculator: Press <Control><Alt>c"]);
    }

    #[test]
    fn triggers_in_portal_format() {
        assert_eq!(portal_trigger("Ctrl+Alt+C"), "CTRL+ALT+c");
        assert_eq!(portal_trigger("Super+Shift+F1"), "LOGO+SHIFT+F1");
    }
}
