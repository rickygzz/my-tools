# My Tools

A small cross-platform system tray app written in Rust. It runs on **Windows**, **macOS** and **Linux**.

When it starts, it puts an icon in the system tray (the menu bar on macOS). Right-click the icon to open a menu with an **Exit** option.

## Project layout

```
.
├── README.md
├── Cargo.toml             # project settings and dependencies
├── build.rs               # embeds the icon into the Windows .exe
├── src/
│   └── main.rs            # tray icon, menu and event loop
├── assets/
│   ├── icon.svg           # the icon source: edit this one
│   └── icons/             # generated from icon.svg (don't edit by hand)
├── xtask/                 # project tasks: `cargo xtask icons`, `cargo xtask dist`
├── packaging/
│   ├── linux/             # .desktop menu entry and install.sh
│   └── macos/             # Info.plist template for the .app bundle
└── .github/workflows/     # builds and publishes releases for all three OSes
```

Built with:

- [`tray-icon`](https://crates.io/crates/tray-icon): tray icon and context menu
- [`winit`](https://crates.io/crates/winit): cross-platform event loop
- [`gtk`](https://crates.io/crates/gtk) (Linux only): runs the tray icon on its own GTK thread

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

The tray uses GTK and AppIndicator, so install their development packages.

**Debian / Ubuntu**

```sh
sudo apt install build-essential libgtk-3-dev libxdo-dev libayatana-appindicator3-dev
```

**Fedora**

```sh
sudo dnf install gcc gtk3-devel libxdo-devel libayatana-appindicator-gtk3-devel
```

**Arch**

```sh
sudo pacman -S base-devel gtk3 xdotool libayatana-appindicator
```

> **GNOME users:** GNOME doesn't show tray icons by default. Install and enable the
> [AppIndicator and KStatusNotifierItem Support](https://extensions.gnome.org/extension/615/appindicator-support/)
> extension. KDE, XFCE, Cinnamon and most other desktops work without it.

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

## Usage

1. Start the app. An orange wrench icon shows up in the tray.
2. **Right-click** the icon to open the menu.
3. Click **Exit** to close the app.

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

Before publishing for macOS, change `CFBundleIdentifier` in [`packaging/macos/Info.plist`](packaging/macos/Info.plist) to your own name, such as `io.github.yourname.my-tools`.

### Platform notes

- **Windows:** Debug builds open a console window, so `println!` output is visible while you develop. Release builds don't open one.
- **macOS:** The app runs as a menu-bar-only app, with no Dock icon.
- **Linux:** On most desktops, AppIndicator opens the menu on left-click *and* right-click.
