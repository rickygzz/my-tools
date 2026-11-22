//! Runs before the app is compiled. On Windows it embeds the icon into my-tools.exe,
//! so Explorer, the taskbar and shortcuts show it.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/icons/icon.ico");

    // Check the *target* OS (not the OS doing the build), so cross-compiling works.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // Embedding needs Windows' resource compiler. Elsewhere (e.g. `cargo xtask check`
        // type-checking the Windows code from Linux or macOS) skip it: the icon only matters in
        // a real .exe, and those are built on Windows.
        if !std::env::var("HOST").is_ok_and(|host| host.contains("windows")) {
            println!("cargo:warning=not building on Windows: the .exe won't have its icon");
            return;
        }
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("assets/icons/icon.ico");
        resource.compile().expect("failed to embed the Windows icon");
    }
}
