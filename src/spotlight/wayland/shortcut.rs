//! The spotlight's keyboard shortcut, through the desktop's *GlobalShortcuts* portal.
//!
//! The portal (part of xdg-desktop-portal) lets an app ask for a shortcut that works in every
//! app, while the desktop stays in charge of the keys: the first time, it shows a dialog where
//! the user confirms or changes the suggested keys, and it remembers the answer.
//!
//! Portal calls answer through a separate "Request" object that later sends a `Response`
//! signal; `request` below handles that dance.

use std::collections::HashMap;
use std::error::Error;
use std::sync::atomic::{AtomicU32, Ordering};

use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{ObjectPath, OwnedValue, Value};

use crate::APP_ID;

const PORTAL: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const SHORTCUT_ID: &str = "spotlight";
/// Suggested keys, in the portal's format. The user can pick others in the desktop's dialog.
const PREFERRED_TRIGGER: &str = "CTRL+ALT+P";

type Options<'a> = HashMap<&'a str, Value<'a>>;
type Results = HashMap<String, OwnedValue>;

/// Registers the shortcut, then calls `pressed` with the press's activation token (if the
/// desktop sent one) every time it's pressed. Runs until the connection breaks.
pub fn listen(pressed: impl Fn(Option<String>)) -> Result<(), Box<dyn Error>> {
    let conn = Connection::session()?;

    register_app_id(&conn);

    let shortcuts = Proxy::new(&conn, PORTAL, PORTAL_PATH, "org.freedesktop.portal.GlobalShortcuts")
        .map_err(|e| format!("the desktop has no GlobalShortcuts portal: {e}"))?;
    // Subscribe before binding, so no press is missed.
    let activated = shortcuts.receive_signal("Activated")?;

    let session = request(&conn, |token| {
        let options = Options::from([
            ("handle_token", Value::from(token)),
            ("session_handle_token", Value::from("my_tools")),
        ]);
        shortcuts.call_method("CreateSession", &(options,))
    })?;
    let session: String = session
        .get("session_handle")
        .and_then(|v| String::try_from(v.try_clone().ok()?).ok())
        .ok_or("the portal didn't return a session")?;
    let session = ObjectPath::try_from(session.as_str())?;

    // Shows the desktop's dialog the first time; later runs reuse the remembered keys.
    let bound = request(&conn, |token| {
        let properties = Options::from([
            ("description", Value::from("Show the mouse spotlight")),
            ("preferred_trigger", Value::from(PREFERRED_TRIGGER)),
        ]);
        let options = Options::from([("handle_token", Value::from(token))]);
        shortcuts.call_method(
            "BindShortcuts",
            &(&session, vec![(SHORTCUT_ID, properties)], "", options),
        )
    })
    .map_err(|e| format!("the shortcut wasn't set up: {e}"))?;
    match trigger_description(&bound) {
        Some(keys) => eprintln!("spotlight: keyboard shortcut: {keys}"),
        None => eprintln!("spotlight: shortcut set up; see your desktop's keyboard settings"),
    }

    for message in activated {
        let Ok((_session, id, _time, options)) = message
            .body()
            .deserialize::<(ObjectPath<'_>, String, u64, Results)>()
        else {
            continue;
        };
        if id == SHORTCUT_ID {
            let token = options
                .get("activation_token")
                .and_then(|v| String::try_from(v.try_clone().ok()?).ok());
            pressed(token);
        }
    }
    Err("lost the connection to the portal".into())
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
        eprintln!("spotlight: could not add a desktop entry for the shortcut: {error}");
        return;
    }
    for _ in 0..20 {
        std::thread::sleep(std::time::Duration::from_millis(250));
        if register().is_ok() {
            return;
        }
    }
    eprintln!("spotlight: the desktop didn't accept the app ID {APP_ID}");
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
    eprintln!("spotlight: added {} so the desktop can tell My Tools apart", path.display());
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

/// The keys the desktop assigned, as it describes them (e.g. "Ctrl+Alt+P"), if it says.
fn trigger_description(results: &Results) -> Option<String> {
    let Value::Array(shortcuts) = &**results.get("shortcuts")? else {
        return None;
    };
    shortcuts.iter().find_map(|shortcut| {
        let Value::Structure(shortcut) = shortcut else {
            return None;
        };
        let [Value::Str(id), Value::Dict(properties)] = shortcut.fields() else {
            return None;
        };
        if id.as_str() != SHORTCUT_ID {
            return None;
        }
        properties.get::<_, String>(&"trigger_description").ok().flatten()
    })
}
