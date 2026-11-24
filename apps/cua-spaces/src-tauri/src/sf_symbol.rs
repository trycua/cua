// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Render SF Symbols to PNG data URLs so the webview can show real macOS system
//! icons (settings, hotspot, search) instead of hand-drawn SVGs. The symbols
//! come from the OS via `NSImage`, rendered as a template (black + alpha) that
//! the renderer masks with `currentColor`. Off macOS — or on any failure — this
//! returns an error and the renderer falls back to its inline SVG glyph.

/// Render the system SF Symbol `name` at `size` points to a `data:image/png`
/// URL. Runs the AppKit work on the main thread.
#[cfg(target_os = "macos")]
#[tauri::command]
pub async fn sf_symbol(app: tauri::AppHandle, name: String, size: f64) -> Result<String, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    app.run_on_main_thread(move || {
        let _ = tx.send(render(&name, size));
    })
    .map_err(|error| error.to_string())?;
    rx.recv().map_err(|error| error.to_string())?
}

#[cfg(target_os = "macos")]
fn render(name: &str, _point_size: f64) -> Result<String, String> {
    use base64::Engine;
    use objc2::AllocAnyThread;
    use objc2_app_kit::{
        NSBitmapImageFileType, NSBitmapImageRep, NSImage, NSImageSymbolConfiguration,
        NSImageSymbolScale,
    };
    use objc2_foundation::{NSDictionary, NSString};

    let ns_name = NSString::from_str(name);
    // Standard AppKit calls (safe in objc2-app-kit 0.3); each returns an owned
    // or autoreleased object we immediately retain via objc2.
    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(&ns_name, None)
        .ok_or_else(|| format!("no SF Symbol named {name:?}"))?;
    // Large scale gives a crisp template; the renderer sizes it via CSS mask.
    let config = NSImageSymbolConfiguration::configurationWithScale(NSImageSymbolScale::Large);
    let image = image
        .imageWithSymbolConfiguration(&config)
        .ok_or("failed to apply symbol configuration")?;
    let tiff = image
        .TIFFRepresentation()
        .ok_or("symbol has no TIFF representation")?;
    let rep = NSBitmapImageRep::initWithData(NSBitmapImageRep::alloc(), &tiff)
        .ok_or("could not build a bitmap rep")?;
    let props = NSDictionary::new();
    let png = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &props) }
        .ok_or("PNG encoding failed")?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(png.to_vec());
    Ok(format!("data:image/png;base64,{encoded}"))
}

#[cfg(not(target_os = "macos"))]
#[tauri::command]
pub async fn sf_symbol(_name: String, _size: f64) -> Result<String, String> {
    Err("SF Symbols are macOS-only".into())
}
