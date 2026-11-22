//! `cargo xtask dist`: builds the finished, ready-to-share package for the OS it runs on.
//!
//! | OS      | Output in dist/                              | Contents                                  |
//! |---------|----------------------------------------------|-------------------------------------------|
//! | Windows | my-tools-<version>-windows-<arch>.zip        | my-tools.exe (icon embedded)              |
//! | macOS   | my-tools-<version>-macos-universal.zip       | My Tools.app (Apple Silicon + Intel)      |
//! | Linux   | my-tools-<version>-linux-<arch>.tar.gz       | binary, icons, .desktop file, install.sh  |
//!
//! Each OS can only build its own package (macOS needs Apple's SDK, Linux needs GTK),
//! so the GitHub workflow in .github/workflows/release.yml runs this on all three.

use std::env::consts::ARCH;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::icons::PNG_SIZES;
use crate::{Result, project_root, run as run_command};

/// The binary's file name (the package name in Cargo.toml).
const BIN: &str = "my-tools";
/// The name people see, used for the macOS app bundle.
const APP_NAME: &str = "My Tools";

pub fn run() -> Result {
    let root = project_root();
    let version = package_version(&root)?;

    let dist = root.join("dist");
    if dist.exists() {
        fs::remove_dir_all(&dist)?;
    }
    fs::create_dir_all(&dist)?;

    let package = if cfg!(target_os = "windows") {
        windows(&root, &dist, &version)?
    } else if cfg!(target_os = "macos") {
        macos(&root, &dist, &version)?
    } else if cfg!(target_os = "linux") {
        linux(&root, &dist, &version)?
    } else {
        return Err("`cargo xtask dist` supports Windows, macOS and Linux only".into());
    };

    println!("\nPackage ready: {}", package.display());
    Ok(())
}

fn windows(root: &Path, dist: &Path, version: &str) -> Result<PathBuf> {
    build_release(root, None)?;

    let name = format!("{BIN}-{version}-windows-{ARCH}");
    let stage = dist.join(&name);
    fs::create_dir_all(&stage)?;
    copy(&root.join("target/release").join(format!("{BIN}.exe")), &stage)?;

    // Windows' own tar (bsdtar) can write .zip files. Use its full path, because Git's
    // GNU tar may come first on PATH and can't.
    let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    let archive = dist.join(format!("{name}.zip"));
    run_command(
        Command::new(Path::new(&system_root).join(r"System32\tar.exe"))
            .args(["-a", "-c", "-f"])
            .arg(&archive)
            .arg("-C")
            .arg(dist)
            .arg(&name),
    )?;
    Ok(archive)
}

fn macos(root: &Path, dist: &Path, version: &str) -> Result<PathBuf> {
    // Build for both Apple Silicon and Intel, then merge them into one "universal" binary.
    const TARGETS: [&str; 2] = ["aarch64-apple-darwin", "x86_64-apple-darwin"];
    run_command(Command::new("rustup").args(["target", "add"]).args(TARGETS))?;
    for target in TARGETS {
        build_release(root, Some(target))?;
    }

    // An app bundle is a folder with a fixed layout that Finder shows as one app.
    let app = dist.join(format!("{APP_NAME}.app"));
    let contents = app.join("Contents");
    fs::create_dir_all(contents.join("MacOS"))?;
    fs::create_dir_all(contents.join("Resources"))?;

    run_command(
        Command::new("lipo")
            .args(["-create", "-output"])
            .arg(contents.join("MacOS").join(BIN))
            .args(TARGETS.map(|t| root.join("target").join(t).join("release").join(BIN))),
    )?;
    copy(&root.join("assets/icons/icon.icns"), &contents.join("Resources"))?;

    let plist = fs::read_to_string(root.join("packaging/macos/Info.plist"))?
        .replace("@NAME@", APP_NAME)
        .replace("@EXECUTABLE@", BIN)
        .replace("@VERSION@", version);
    fs::write(contents.join("Info.plist"), plist)?;

    // "Ad-hoc" signature: not a paid Apple Developer ID, but required for the app to run
    // on Apple Silicon at all.
    run_command(Command::new("codesign").args(["--force", "--deep", "--sign", "-"]).arg(&app))?;

    // `ditto` is the macOS way to zip an app bundle (keeps permissions and metadata).
    let archive = dist.join(format!("{BIN}-{version}-macos-universal.zip"));
    run_command(
        Command::new("ditto")
            .args(["-c", "-k", "--keepParent"])
            .arg(&app)
            .arg(&archive),
    )?;
    Ok(archive)
}

fn linux(root: &Path, dist: &Path, version: &str) -> Result<PathBuf> {
    build_release(root, None)?;

    let name = format!("{BIN}-{version}-linux-{ARCH}");
    let stage = dist.join(&name);
    let icons = stage.join("icons");
    fs::create_dir_all(&icons)?;

    copy(&root.join("target/release").join(BIN), &stage)?;
    copy(&root.join("packaging/linux/my-tools.desktop"), &stage)?;
    copy(&root.join("packaging/linux/install.sh"), &stage)?;
    make_executable(&stage.join(BIN))?;
    make_executable(&stage.join("install.sh"))?;

    copy(&root.join("assets/icon.svg"), &icons)?;
    for size in PNG_SIZES {
        copy(&root.join(format!("assets/icons/icon-{size}.png")), &icons)?;
    }

    let archive = dist.join(format!("{name}.tar.gz"));
    run_command(
        Command::new("tar")
            .arg("-czf")
            .arg(&archive)
            .arg("-C")
            .arg(dist)
            .arg(&name),
    )?;
    Ok(archive)
}

/// `cargo build --release` for the app, optionally for another target (CPU type).
fn build_release(root: &Path, target: Option<&str>) -> Result {
    // Use the same cargo that is running us.
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let mut command = Command::new(cargo);
    command
        .current_dir(root)
        .args(["build", "--release", "--package", BIN]);
    if let Some(target) = target {
        command.args(["--target", target]);
    }
    run_command(&mut command)
}

/// Reads `version = "..."` from the app's Cargo.toml (the first one is in [package]).
fn package_version(root: &Path) -> Result<String> {
    let manifest = fs::read_to_string(root.join("Cargo.toml"))?;
    manifest
        .lines()
        .find_map(|line| line.trim().strip_prefix("version = \""))
        .and_then(|rest| rest.strip_suffix('"'))
        .map(str::to_owned)
        .ok_or_else(|| "no version found in Cargo.toml".into())
}

/// Copies a file into a folder, keeping its name.
fn copy(file: &Path, into_dir: &Path) -> Result {
    let name = file.file_name().ok_or("not a file")?;
    fs::copy(file, into_dir.join(name))
        .map_err(|e| format!("could not copy {}: {e}", file.display()))?;
    Ok(())
}

/// Marks a file as runnable (chmod 755). Only matters on Linux/macOS.
fn make_executable(path: &Path) -> Result {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
