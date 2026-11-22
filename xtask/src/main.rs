//! Project tasks for my-tools. Run them from anywhere inside the my-tools folder:
//!
//!   cargo xtask icons   regenerate assets/icons/ from assets/icon.svg
//!   cargo xtask dist    build the finished package for this OS into dist/
//!
//! This is the "xtask" pattern: project scripts written in Rust, so they work the
//! same on Windows, macOS and Linux without needing bash or PowerShell.

mod dist;
mod icons;

use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

pub type Result<T = ()> = std::result::Result<T, Box<dyn Error>>;

const USAGE: &str = "\
Usage: cargo xtask <task>

Tasks:
  icons   Regenerate assets/icons/ from assets/icon.svg
  dist    Build the release package for this OS into dist/";

fn main() -> ExitCode {
    let task = std::env::args().nth(1);
    let result = match task.as_deref() {
        Some("icons") => icons::run(),
        Some("dist") => dist::run(),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::FAILURE;
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

/// The my-tools folder (the parent of this xtask folder).
pub fn project_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives inside the project folder")
        .to_path_buf()
}

/// Runs a command, printing it first, and fails if it doesn't succeed.
pub fn run(command: &mut Command) -> Result {
    println!("> {command:?}");
    let status = command
        .status()
        .map_err(|e| format!("could not start {:?}: {e}", command.get_program()))?;
    if !status.success() {
        return Err(format!("command failed ({status}): {command:?}").into());
    }
    Ok(())
}
