//! Runs before the app is compiled. On Windows it embeds the icon into my-tools.exe,
//! so Explorer, the taskbar and shortcuts show it.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/icons/icon.ico");

    // Check the *target* OS (not the OS doing the build), so cross-compiling works.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("assets/icons/icon.ico");
        resource.compile().expect("failed to embed the Windows icon");
    }
}
