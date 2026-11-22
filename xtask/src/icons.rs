//! `cargo xtask icons`: turns `assets/icon.svg` into every icon file the app needs:
//!
//! - `icon-<size>.png`  Linux (and anywhere else a PNG is wanted)
//! - `icon.ico`         Windows (embedded in the .exe by build.rs)
//! - `icon.icns`        macOS (goes in the .app bundle)
//! - `tray-32.rgba`     raw pixels the app loads for its tray icon

use std::fs::{self, File};

use resvg::tiny_skia::{Pixmap, Transform};
use resvg::usvg::{Options, Tree};

use crate::{Result, project_root};

/// Sizes written as PNG files (the standard Linux icon theme sizes, plus 1024 for macOS/stores).
pub const PNG_SIZES: &[u32] = &[16, 24, 32, 48, 64, 128, 256, 512, 1024];
/// Sizes packed into the Windows .ico (256 is the largest Windows uses).
const ICO_SIZES: &[u32] = &[16, 24, 32, 48, 64, 128, 256];
/// Sizes packed into the macOS .icns.
const ICNS_SIZES: &[u32] = &[16, 32, 64, 128, 256, 512, 1024];
/// Size of the tray icon the app embeds.
const TRAY_SIZE: u32 = 32;

pub fn run() -> Result {
    let assets = project_root().join("assets");
    let out = assets.join("icons");
    fs::create_dir_all(&out)?;

    let svg = fs::read(assets.join("icon.svg"))?;
    let tree = Tree::from_data(&svg, &Options::default())?;

    for &size in PNG_SIZES {
        render(&tree, size)?.save_png(out.join(format!("icon-{size}.png")))?;
    }

    let mut ico = ico::IconDir::new(ico::ResourceType::Icon);
    for &size in ICO_SIZES {
        let image = ico::IconImage::from_rgba_data(size, size, rgba(&render(&tree, size)?));
        ico.add_entry(ico::IconDirEntry::encode(&image)?);
    }
    ico.write(File::create(out.join("icon.ico"))?)?;

    let mut icns = icns::IconFamily::new();
    for &size in ICNS_SIZES {
        let pixels = rgba(&render(&tree, size)?);
        let image = icns::Image::from_data(icns::PixelFormat::RGBA, size, size, pixels)?;
        icns.add_icon(&image)?;
    }
    icns.write(File::create(out.join("icon.icns"))?)?;

    fs::write(out.join("tray-32.rgba"), rgba(&render(&tree, TRAY_SIZE)?))?;

    println!("Icons written to {}", out.display());
    Ok(())
}

/// Draws the SVG into a square image of `size` x `size` pixels.
fn render(tree: &Tree, size: u32) -> Result<Pixmap> {
    let mut pixmap = Pixmap::new(size, size).ok_or("invalid icon size")?;
    let scale = size as f32 / tree.size().width().max(tree.size().height());
    resvg::render(tree, Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    Ok(pixmap)
}

/// Plain (non-premultiplied) RGBA bytes, which the ICO/ICNS formats and the tray expect.
fn rgba(pixmap: &Pixmap) -> Vec<u8> {
    pixmap
        .pixels()
        .iter()
        .flat_map(|p| {
            let c = p.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        })
        .collect()
}
