//! `cargo xtask check`: catches compile errors and clippy warnings on all three OSes from
//! whichever one you're on, then runs the tests here.
//!
//! Each OS has its own code (`#[cfg(...)]`), which normally only gets compiled on that OS.
//! `cargo clippy --target ...` type-checks it without building or linking anything, so it
//! works across OSes once the targets' standard libraries are installed with rustup.
//! (It can't run that code: CI still runs the tests on each OS.)

use std::process::Command;

use crate::{Result, project_root, run};

/// One target per OS. Which CPU doesn't matter for type-checking.
const TARGETS: [(&str, &str); 3] = [
    ("Windows", "x86_64-pc-windows-msvc"),
    ("macOS", "aarch64-apple-darwin"),
    ("Linux", "x86_64-unknown-linux-gnu"),
];

pub fn run_all() -> Result {
    let root = project_root();
    let installed = installed_targets();

    let missing: Vec<&str> = TARGETS
        .iter()
        .map(|&(_, target)| target)
        .filter(|target| installed.as_ref().is_some_and(|list| !list.iter().any(|t| t == target)))
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "missing Rust targets; install them once with:\n  rustup target add {}",
            missing.join(" ")
        )
        .into());
    }

    for (os, target) in TARGETS {
        println!("\n=== {os} ({target}) ===");
        // The app only: xtask is a build tool that runs on this machine.
        run(Command::new("cargo")
            .current_dir(&root)
            .args(["clippy", "--package", "my-tools", "--all-targets", "--target", target])
            .args(["--", "-D", "warnings"]))?;
    }

    println!("\n=== This machine: xtask lints and all tests ===");
    run(Command::new("cargo")
        .current_dir(&root)
        .args(["clippy", "--package", "xtask", "--all-targets", "--", "-D", "warnings"]))?;
    run(Command::new("cargo").current_dir(&root).args(["test", "--workspace"]))?;

    println!("\nAll good on Windows, macOS and Linux.");
    Ok(())
}

/// The targets rustup has installed, or `None` without rustup (then just try them all).
fn installed_targets() -> Option<Vec<String>> {
    let output = Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).lines().map(str::to_owned).collect())
}
