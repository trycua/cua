use std::{
    any::Any,
    panic::{catch_unwind, AssertUnwindSafe},
    path::Path,
    sync::Mutex,
};

use clipboard_rs::{common::RustImage, Clipboard, ClipboardContext, ContentFormat};
use cua_driver_core::clipboard::ClipboardBackend;

pub struct MacosClipboard {
    context: Mutex<Option<ClipboardContext>>,
    initialize: fn() -> Result<ClipboardContext, String>,
}

impl MacosClipboard {
    pub fn new() -> Self {
        Self {
            context: Mutex::new(None),
            initialize: initialize_context,
        }
    }

    #[cfg(test)]
    fn with_initializer(initialize: fn() -> Result<ClipboardContext, String>) -> Self {
        Self {
            context: Mutex::new(None),
            initialize,
        }
    }

    fn with_context<T>(
        &self,
        operation: impl FnOnce(&ClipboardContext) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut context = self
            .context
            .lock()
            .map_err(|_| "clipboard lock was poisoned".to_owned())?;
        if context.is_none() {
            let initialized =
                catch_unwind(AssertUnwindSafe(|| (self.initialize)())).map_err(|panic| {
                    format!(
                        "clipboard initialization panicked: {}",
                        panic_message(panic)
                    )
                })??;
            *context = Some(initialized);
        }
        operation(context.as_ref().expect("clipboard context was initialized"))
    }
}

fn initialize_context() -> Result<ClipboardContext, String> {
    ClipboardContext::new().map_err(|error| error.to_string())
}

fn panic_message(panic: Box<dyn Any + Send>) -> String {
    if let Some(message) = panic.downcast_ref::<&str>() {
        (*message).to_owned()
    } else if let Some(message) = panic.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown native clipboard failure".to_owned()
    }
}

fn absolute_existing_file(path: &str) -> Result<String, String> {
    let path = Path::new(path);
    if !path.is_absolute() {
        return Err("clipboard file paths must be absolute".into());
    }
    let canonical = path.canonicalize().map_err(|error| error.to_string())?;
    if !canonical.is_file() {
        return Err("clipboard path must identify an existing file".into());
    }
    Ok(canonical.to_string_lossy().into_owned())
}

impl ClipboardBackend for MacosClipboard {
    fn available_formats(&self) -> Result<Vec<String>, String> {
        self.with_context(|context| context.available_formats().map_err(|e| e.to_string()))
    }

    fn read_text(&self) -> Result<Option<String>, String> {
        self.with_context(|context| {
            if context.has(ContentFormat::Text) {
                context.get_text().map(Some).map_err(|e| e.to_string())
            } else {
                Ok(None)
            }
        })
    }

    fn write_text(&self, text: String) -> Result<(), String> {
        self.with_context(|context| context.set_text(text).map_err(|e| e.to_string()))
    }

    fn write_image(&self, absolute_path: &str) -> Result<(), String> {
        let path = absolute_existing_file(absolute_path)?;
        let image = clipboard_rs::RustImageData::from_path(&path).map_err(|e| e.to_string())?;
        self.with_context(|context| context.set_image(image).map_err(|e| e.to_string()))
    }

    fn write_file_url(&self, absolute_path: &str) -> Result<(), String> {
        let path = absolute_existing_file(absolute_path)?;
        self.with_context(|context| context.set_files(vec![path]).map_err(|e| e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static INITIALIZATION_ATTEMPTS: AtomicUsize = AtomicUsize::new(0);

    fn unavailable_context() -> Result<ClipboardContext, String> {
        INITIALIZATION_ATTEMPTS.fetch_add(1, Ordering::SeqCst);
        Err("general pasteboard is unavailable".into())
    }

    fn panicking_context() -> Result<ClipboardContext, String> {
        panic!("unexpected NULL returned from +[NSPasteboard generalPasteboard]")
    }

    #[test]
    fn construction_does_not_initialize_the_native_clipboard() {
        INITIALIZATION_ATTEMPTS.store(0, Ordering::SeqCst);
        let _backend = MacosClipboard::with_initializer(unavailable_context);
        assert_eq!(INITIALIZATION_ATTEMPTS.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn unavailable_initialization_is_retryable() {
        INITIALIZATION_ATTEMPTS.store(0, Ordering::SeqCst);
        let backend = MacosClipboard::with_initializer(unavailable_context);
        for _ in 0..2 {
            assert!(backend
                .available_formats()
                .unwrap_err()
                .contains("general pasteboard is unavailable"));
        }
        assert_eq!(INITIALIZATION_ATTEMPTS.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn native_initialization_panic_becomes_an_error() {
        let backend = MacosClipboard::with_initializer(panicking_context);
        let error = backend.available_formats().unwrap_err();
        assert!(error.contains("clipboard initialization panicked"));
        assert!(error.contains("unexpected NULL returned"));
    }

    #[test]
    fn rejects_relative_local_paths_before_clipboard_access() {
        let backend = MacosClipboard::new();
        assert!(backend
            .write_file_url("relative.txt")
            .unwrap_err()
            .contains("absolute"));
        assert!(backend
            .write_image("relative.png")
            .unwrap_err()
            .contains("absolute"));
    }

    #[test]
    fn native_clipboard_round_trips_text_png_and_file_url_in_ci() {
        if std::env::var_os("CI").is_none() {
            return;
        }
        let backend = MacosClipboard::new();
        let original_text = backend.read_text().ok().flatten();
        backend
            .write_text("cua-driver clipboard test".into())
            .unwrap();
        assert_eq!(
            backend.read_text().unwrap().as_deref(),
            Some("cua-driver clipboard test")
        );

        let png =
            std::env::temp_dir().join(format!("cua-driver-clipboard-{}.png", std::process::id()));
        image::RgbaImage::from_pixel(1, 1, image::Rgba([1, 2, 3, 255]))
            .save(&png)
            .unwrap();
        backend.write_image(png.to_str().unwrap()).unwrap();
        let image_types = backend.available_formats().unwrap();
        assert!(image_types.iter().any(|kind| {
            matches!(kind.as_str(), "public.png" | "public.tiff")
                || kind.to_ascii_lowercase().contains("image")
        }));

        backend.write_file_url(png.to_str().unwrap()).unwrap();
        let file_types = backend.available_formats().unwrap();
        assert!(file_types.iter().any(|kind| {
            kind == "public.file-url"
                || kind.to_ascii_lowercase().contains("file")
                || kind.to_ascii_lowercase().contains("uri")
        }));
        if let Some(text) = original_text {
            backend.write_text(text).unwrap();
        }
        let _ = std::fs::remove_file(png);
    }
}
