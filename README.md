# My Tools

A small cross-platform system tray app written in Rust. It runs on **Windows**, **macOS** and **Linux**.

When it starts, it puts an icon in the system tray (the menu bar on macOS). Right-click the icon to open a menu with **Calculator**, **Edit window zones...** and **Exit**.

## Project layout

```
.
├── README.md
├── Cargo.toml             # project settings and dependencies
├── build.rs               # embeds the icon into the Windows .exe
├── src/
│   ├── main.rs            # event loop: starts the tray, spotlight and calculator
│   ├── tray/              # tray icon and its menu (Calculator, Edit window zones, Exit)
│   │   ├── mod.rs         # shared tooltip, label and icon pixels
│   │   ├── windows_macos.rs # Windows, macOS: tray-icon crate
│   │   └── linux.rs       # Linux: ksni over D-Bus, GNOME extension setup
│   ├── calculator/        # calculator window (Ctrl+Alt+C, or the tray menu)
│   │   ├── mod.rs         # opens it on the screen with the mouse
│   │   ├── engine/        # line → result: tokenizer, parser, evaluator, units, dates,
│   │   │                  #   time zones, ipconfig() (any OS, unit-tested)
│   │   ├── suggester.rs   # what to offer while typing (any OS, unit-tested)
│   │   ├── presenter.rs   # history, names, Up/Down, suggestions (any OS, unit-tested)
│   │   ├── view.rs        # draws the window with egui, handles its keys
│   │   ├── window.rs      # the winit window egui draws into, through wgpu
│   │   └── hotkey.rs      # Ctrl+Alt+C in every app
│   ├── zones/             # window zones: hotkeys put the active window in a zone
│   │   ├── layout.rs      # zones in percent, layouts, templates (any OS, unit-tested)
│   │   ├── geometry.rs    # zones in pixels, which zone a window is in (any OS, unit-tested)
│   │   ├── config.rs      # reads and writes zones.cfg (any OS, unit-tested)
│   │   ├── mover.rs       # to zone N, next, previous, other monitor (any OS, unit-tested)
│   │   ├── drag.rs        # Shift+drag: what shows, where the window goes (any OS, unit-tested)
│   │   ├── paint.rs       # the overlay as rectangles, seven-segment numbers (any OS, unit-tested)
│   │   ├── desktop.rs     # the Desktop trait each OS implements, and a fake for tests
│   │   ├── win32.rs       # Windows: SetWindowPos, minus DWM's invisible borders
│   │   ├── win32_drag.rs  # Windows: WinEvent hook for drags, layered overlay window
│   │   ├── macos.rs       # macOS: the Accessibility API
│   │   ├── macos_drag.rs  # macOS: global mouse monitors, a layer-backed window per screen
│   │   ├── x11.rs         # Linux X11: EWMH messages to the window manager, RandR
│   │   ├── x11_drag.rs    # Linux X11: XInput2 raw mouse events, ARGB overlay window
│   │   └── editor/        # the zone editor window (tray menu): presenter (unit-tested), egui view
│   └── spotlight/         # mouse spotlight (double Ctrl; a shortcut on Wayland)
│       ├── mod.rs         # shows/hides/animates it, interprets keys (any OS)
│       ├── detector.rs    # double-tap timing rules (any OS, unit-tested)
│       ├── presenter.rs   # shrink/follow/auto-hide animation (any OS, unit-tested)
│       ├── shape.rs       # draws the circle pixel by pixel (Windows, X11; unit-tested)
│       ├── win32.rs       # Windows: keyboard/mouse hooks, layered overlay window
│       ├── macos.rs       # macOS: CGEventTap, NSWindow + Core Animation overlay
│       ├── linux.rs       # Linux: picks X11 or Wayland
│       ├── x11.rs         # Linux X11: XInput2 raw events, ARGB overlay window
│       ├── wayland/       # Linux Wayland: shortcut portal, maximized ARGB window
│       └── unsupported.rs # stand-in for any other OS
├── assets/
│   ├── icon.svg           # the icon source: edit this one
│   └── icons/             # generated from icon.svg (don't edit by hand)
├── xtask/                 # project tasks: `cargo xtask check`, `icons`, `dist`
├── packaging/
│   ├── linux/             # .desktop menu entry and install.sh
│   └── macos/             # Info.plist template for the .app bundle
└── .github/workflows/
    ├── ci.yml             # every push: clippy + tests on Windows, macOS and Linux
    └── release.yml        # version tags: builds and publishes packages for all three
```

Built with:

- [`winit`](https://crates.io/crates/winit): cross-platform event loop
- [`tray-icon`](https://crates.io/crates/tray-icon) (Windows, macOS): tray icon and context menu
- [`ksni`](https://crates.io/crates/ksni) (Linux): tray icon and context menu over D-Bus (StatusNotifierItem), no GTK needed
- [`zbus`](https://crates.io/crates/zbus) (Linux): D-Bus calls for GNOME extension setup and notifications
- [`egui`](https://crates.io/crates/egui), with [`egui-winit`](https://crates.io/crates/egui-winit) and [`egui-wgpu`](https://crates.io/crates/egui-wgpu): the calculator's window
- [`rust_decimal`](https://crates.io/crates/rust_decimal), [`chrono`](https://crates.io/crates/chrono), [`chrono-tz`](https://crates.io/crates/chrono-tz): the calculator's exact decimals, dates and time zones
- [`global-hotkey`](https://crates.io/crates/global-hotkey): Ctrl+Alt+C in every app

## Prerequisites

Install Rust with [rustup](https://rustup.rs) on every platform. Then install the system packages for your OS.

### Windows

- Install the **Visual Studio C++ Build Tools** with the "Desktop development with C++" workload. rustup offers to install them if they're missing.

### macOS

- Install the Xcode command-line tools:

  ```sh
  xcode-select --install
  ```

### Linux

Nothing extra beyond a C toolchain (`build-essential`, `gcc` or `base-devel`). The tray talks to the desktop over D-Bus in pure Rust, so no GTK or AppIndicator packages are needed.

> **GNOME users:** GNOME doesn't show tray icons by default; it needs the
> [AppIndicator and KStatusNotifierItem Support](https://extensions.gnome.org/extension/615/appindicator-support/)
> extension. If it's missing, the app turns it on (if installed) or asks GNOME to install it
> (GNOME shows a confirmation; no logout needed). If that doesn't work, it shows a
> notification explaining how to install it by hand, then quits. KDE, XFCE, Cinnamon and most
> other desktops work without it.

## Build and run

Run these from the project folder:

```sh
cargo run              # debug build, runs right away
cargo build --release  # optimized build
```

The release binary goes to:

| OS      | Path                          |
|---------|-------------------------------|
| Windows | `target/release/my-tools.exe` |
| macOS   | `target/release/my-tools`     |
| Linux   | `target/release/my-tools`     |

### Check all three OSes before pushing

Each OS has its own code, which normally only compiles on that OS. This lints the Windows, macOS and Linux code from whichever one you're on, then runs the tests:

```sh
rustup target add x86_64-pc-windows-msvc aarch64-apple-darwin x86_64-unknown-linux-gnu  # once
cargo xtask check
```

It only type-checks the other OSes' code (nothing is built or run for them), so it catches compile errors and clippy warnings, not behaviour. CI still runs everything on real Windows, macOS and Linux machines. When it checks the Windows code from another OS, `build.rs` prints a warning that the icon isn't embedded; that's expected.

## Usage

1. Start the app. An orange wrench icon shows up in the tray.
2. **Right-click** the icon to open the menu.
3. Click **Calculator** to open the calculator, **Edit window zones...** to change the zones, or **Exit** to close the app.

### Calculator

Press **Ctrl+Alt+C** in any program (or choose **Calculator** in the tray menu). A small bar opens on the screen with the mouse, ready to type. Type a line and press **Enter**:

| You type | You get |
|---|---|
| `2+2*3` | `8` |
| `0.1+0.2` | `0.3` (exact decimals, up to 28 of them) |
| `sqrt(2)`, `round(3.14159, 2)`, `max(1, 5, 3)` | `1.4142135623730950488016887242`, `3.14`, `5` |
| `10 km to mi`, `100 C to F`, `1 GiB to MB` | `6.213711922 mi`, `212 F`, `1,073.741824 MB` |
| `today + 30 d`, `2026-12-25 - today` | a date with its weekday, a number of days |
| `10:30 Mexico to India`, `3 pm Miami to Sao Paulo` | the time there, with its UTC offset |
| `rate = 1500`, then `rate * 12` | names keep values; `ans` is the last answer |
| `ipconfig()` | this computer's IP address and gateway |

| Key | Does |
|---|---|
| **Enter** | calculates; the line stays, the result shows below it |
| **Esc** | hides the calculator (it also hides when you click elsewhere) |
| **Ctrl+H**, or the arrow at the right | shows or hides the history of this session |
| **Ctrl+C** with nothing selected | copies the result |
| **Up** / **Down** | brings back the lines entered before |
| **Ctrl+Arrow** (with Shift, further) | moves the window; dragging it with the mouse does too |

While you type, a list offers what can come next: functions and names where a value starts, units after a number, places after a time. **Down**/**Up** go through it, **Tab** or **Enter** writes the one chosen, **Esc** closes it. Choosing the first unit or place writes `to` after it, so the list goes on with the second.

An error says what's wrong and selects the character at fault: `1/0` gives `Division by zero at position 2`.

The history, the names and the lines entered last until the app exits. On **Wayland**, apps can't register keys like Ctrl+Alt+C, so open the calculator from the tray menu there. If another app already uses Ctrl+Alt+C, the tray menu works too.

### Window zones

Zones divide each monitor into areas that windows snap into.

**Dragging a window into a zone:** start dragging a window by its title bar, then press **Shift** (or click the **right mouse button**). The zones of every monitor appear, numbered, with the one under the mouse in blue; they stay while you go on dragging, so you can let go of Shift. Let go of the mouse over a zone, and the window fills it. Pressing Shift again hides the zones (the window then drops where it is); **Esc** cancels the drag. Without Shift, dragging works as usual, and resizing a window by its edge shows no zones.

These keys move the window you're working in (**Win** is **Command** on macOS):

| Keys | Does |
|---|---|
| **Ctrl+Alt+Win+1** to **9** | moves it into that zone of its monitor |
| **Ctrl+Alt+Win+Right** / **Left** | moves it to the next or previous zone, around the end; a window in no zone goes to the first or last |
| **Ctrl+Alt+Win+M** | moves it to the monitor the mouse is on, at the same place there |

**The window zones editor:** right-click the tray icon and choose **Edit window zones...**. It shows a monitor at its real shape, with its zones over it, numbered, gaps included.

| To | Do this |
|---|---|
| Resize zones | Drag a border. Every border on the same line moves with it, so a grid stays a grid. A zone can't get smaller than 5% of the screen. |
| Resize one zone | Click it to select it, then drag one of the white handles at its corners and the middles of its sides. Only that zone changes, so it can go over the ones next to it. |
| Move a zone | Drag inside it. |
| Add a zone | Drag on an empty part of the monitor. |
| Split or delete a zone | Right-click it. A split goes where you clicked; near an edge, in the middle. **Delete** also deletes the selected zone. |
| Start over | **Layout**: columns, rows, a wide center or a grid. |
| Change the gap | **Gap**, in pixels of the screen. |
| Give a monitor its own zones | **Edit layout of**: pick the monitor. It shows the default layout until your first change, which gives it its own; **Use default layout** drops it. |

Borders and moved zones snap to the edges of the screen and of the other zones. **Undo** (Ctrl+Z) and **Redo** (Ctrl+Y) cover every change, in every layout. Nothing is used until you **Save** (Ctrl+S); the title shows `*` while there are unsaved changes, and closing asks what to do with them. **Edit as text...** opens `zones.cfg` in your text editor instead.

The zones are in `zones.cfg`, in your config folder (`%APPDATA%\my-tools` on Windows, `~/Library/Application Support/my-tools` on macOS, `~/.config/my-tools` on Linux). It's made with three columns the first time it's needed, explains itself at the top, and changes apply at the next key press:

```
Layout = Columns 3          # or Columns 25 50 25, Rows 2, Grid 3 x 2
Gap = 8                     # pixels between zones and around them

# A monitor's own layout: the lines after a Monitor line (left, top, width, height)
Monitor = 1920, 0, 2560, 1440
Zone = 0, 0, 60, 100        # your own zones, in percent: left, top, width, height
Zone = 60, 0, 40, 100
```

A line that can't be used is skipped, and the reason, with its line number, goes to stderr (run from a terminal to see it).

| OS | Status |
|---|---|
| Windows | Works. Windows only lets programs run as administrator be moved by programs run as administrator. Windows' own snap layouts still show when you drag to the top of the screen; turn off **Settings → System → Multitasking → Snap windows** if they get in the way. |
| macOS | The first time, macOS asks you to allow My Tools under **System Settings → Privacy & Security → Accessibility**; press the keys again after. Shift+drag only works once that's allowed. Full-screen windows can't be moved. |
| Linux, X11 | Works with any window manager that follows EWMH (GNOME, KDE, XFCE, Cinnamon, most others). The zones while dragging need a compositor to be see-through, as the spotlight does. The work area (without panels) is one for all monitors in EWMH, so a panel on one monitor can shrink the zones of another. |
| Linux, Wayland | Doesn't work: Wayland doesn't let apps move other apps' windows. The keys aren't registered there. |

### Mouse spotlight

**Tap Left Ctrl twice** quickly, in any program, to find the mouse pointer. The screens dim, except for a circle around the pointer. The circle starts large and shrinks onto the pointer, then follows it.

- It goes away when you press a key or click, or after the pointer has stayed still for 3 seconds.
- Clicks and typing still reach the programs underneath.
- Each tap must be under 0.3 s, with at most 0.4 s between taps. Holding Ctrl, or shortcuts like Ctrl+C, don't trigger it.

| OS | Status |
|---|---|
| Windows | Works. Nothing to set up. |
| macOS | Uses Left **Control**. The first time, macOS asks you to allow My Tools under **System Settings → Privacy & Security → Input Monitoring**. The spotlight starts working a few seconds after you allow it. |
| Linux, X11 | Works. The dimming needs a compositor; GNOME, KDE, Cinnamon, XFCE and most other desktops have one. |
| Linux, Wayland | Wayland doesn't let apps see other apps' keys, so instead of a double Ctrl tap you press a keyboard shortcut, **Ctrl+Alt+P** by default. The first time, the desktop asks you to confirm or change it; change it later in **Settings → Keyboard → Keyboard Shortcuts** (GNOME). Only the monitor with the pointer is dimmed, and not GNOME's top bar: GNOME paints full-screen windows over solid black, so the spotlight uses a maximized window. While it shows, a click hides it instead of reaching the window underneath. Needs GNOME 48+ or KDE Plasma 5.27+. |

## Icon

All of the app's icons come from one file, [`assets/icon.svg`](assets/icon.svg). You can edit it in any SVG editor, such as Inkscape, Figma or a text editor.

After you change it, regenerate the icon files:

```sh
cargo xtask icons
```

This writes to `assets/icons/`:

| File | Used for |
|---|---|
| `icon.ico` | **Windows:** embedded in `my-tools.exe` by `build.rs` |
| `icon.icns` | **macOS:** goes in `My Tools.app/Contents/Resources/` |
| `icon-<size>.png` (16 to 1024 px) | **Linux** icon themes, and anywhere else a PNG is needed |
| `tray-32.rgba` | the tray icon, compiled into the app |

Commit the generated files, so the app builds without running `cargo xtask icons` first.

Keep the design bold and simple. The tray shows it at only 16 to 32 px.

## Packaging and releases

### One command, on your computer

```sh
cargo xtask dist
```

This builds the finished package **for the OS you run it on** and puts it in `dist/`:

| OS | Package | What's inside |
|---|---|---|
| Windows | `my-tools-<version>-windows-x86_64.zip` | `my-tools.exe`, with the icon embedded |
| macOS | `my-tools-<version>-macos-universal.zip` | `My Tools.app` (runs on Apple Silicon and Intel) |
| Linux | `my-tools-<version>-linux-x86_64.tar.gz` | the program, icons, menu entry and `install.sh` |

The version comes from `Cargo.toml`.

A computer can only build the package for its own OS. Building for macOS needs Apple's tools, and building for Linux needs the GTK libraries. To get all three at once, use GitHub.

### One command, all three OSes (GitHub Actions)

[`.github/workflows/release.yml`](.github/workflows/release.yml) runs `cargo xtask dist` on a Windows, a macOS and a Linux machine on GitHub's servers.

**To publish a release:**

1. Set the new `version` in `Cargo.toml` and commit it.
2. Run:

   ```sh
   git tag v0.1.0
   git push origin v0.1.0
   ```

When the builds finish, the three packages show up on the repository's **Releases** page.

**To build without releasing:** open the **Actions** tab, choose **Release**, then **Run workflow**. Download the packages from the run's **Artifacts**.

This needs the repository on GitHub, with a remote set up (`git remote add origin ...`).

### Installing the packages

- **Windows:** unzip the file and run `my-tools.exe`. Because the app isn't code-signed, SmartScreen may warn about it on first launch. Choose **More info**, then **Run anyway**.
- **macOS:** unzip the file and move `My Tools.app` to Applications. The app isn't notarized by Apple, so the first time you open it, right-click it and choose **Open**. You can also run `xattr -dr com.apple.quarantine "/Applications/My Tools.app"`.
- **Linux:**

  ```sh
  tar xzf my-tools-*-linux-*.tar.gz
  cd my-tools-*-linux-*/
  ./install.sh               # installs to ~/.local, no sudo needed
  ./install.sh --uninstall   # removes it again
  ```

  This also adds the app, with its icon, to your desktop's app menu.

Before publishing, replace the placeholder app ID `com.example.my-tools` with your own reverse-domain name, such as `io.github.yourname.my-tools`, everywhere it appears: `APP_ID` in [`src/main.rs`](src/main.rs), `CFBundleIdentifier` in [`packaging/macos/Info.plist`](packaging/macos/Info.plist), the name of [`packaging/linux/com.example.my-tools.desktop`](packaging/linux/com.example.my-tools.desktop), and the references to that file in `install.sh` and `xtask/src/dist.rs`. macOS uses it to tell apps apart; Linux desktops use it to remember the spotlight shortcut.

When you run the app from a build folder on Linux (`cargo run`) without having installed it, it adds a hidden `~/.local/share/applications/com.example.my-tools.desktop` pointing at that build, because the desktop needs one before it accepts the shortcut. `install.sh` replaces it.

### Platform notes

- **Windows:** Debug builds open a console window, so `println!` output is visible while you develop. Release builds don't open one.
- **macOS:** The app runs as a menu-bar-only app, with no Dock icon.
- **Linux:** The menu opens on right-click; left-click does nothing, as on the other platforms.
