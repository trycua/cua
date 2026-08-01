use std::{path::Path, sync::Mutex};

use clipboard_rs::{common::RustImage, Clipboard, ClipboardContext, ContentFormat};
use cua_driver_core::clipboard::ClipboardBackend;

pub struct LinuxClipboard {
    context: Result<Mutex<ClipboardContext>, String>,
}

impl LinuxClipboard {
    pub fn new() -> Self {
        Self {
            context: ClipboardContext::new()
                .map(Mutex::new)
                .map_err(|error| error.to_string()),
        }
    }

    fn context(&self) -> Result<std::sync::MutexGuard<'_, ClipboardContext>, String> {
        self.context
            .as_ref()
            .map_err(Clone::clone)?
            .lock()
            .map_err(|_| "clipboard lock was poisoned".to_owned())
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

impl ClipboardBackend for LinuxClipboard {
    fn available_formats(&self) -> Result<Vec<String>, String> {
        self.context()?
            .available_formats()
            .map_err(|e| e.to_string())
    }

    fn read_text(&self) -> Result<Option<String>, String> {
        let context = self.context()?;
        if context.has(ContentFormat::Text) {
            context.get_text().map(Some).map_err(|e| e.to_string())
        } else {
            Ok(None)
        }
    }

    fn write_text(&self, text: String) -> Result<(), String> {
        self.context()?.set_text(text).map_err(|e| e.to_string())
    }

    fn write_image(&self, absolute_path: &str) -> Result<(), String> {
        let path = absolute_existing_file(absolute_path)?;
        let image = clipboard_rs::RustImageData::from_path(&path).map_err(|e| e.to_string())?;
        self.context()?.set_image(image).map_err(|e| e.to_string())
    }

    fn write_file_url(&self, absolute_path: &str) -> Result<(), String> {
        let path = absolute_existing_file(absolute_path)?;
        self.context()?
            .set_files(vec![path])
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_relative_local_paths_before_clipboard_access() {
        let backend = LinuxClipboard::new();
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
    fn native_clipboard_round_trips_text_when_ci_has_a_display() {
        if std::env::var_os("CI").is_none() {
            return;
        }
        let backend = LinuxClipboard::new();
        if backend.available_formats().is_err() {
            return;
        }
        backend
            .write_text("cua-driver clipboard test".into())
            .unwrap();
        assert_eq!(
            backend.read_text().unwrap().as_deref(),
            Some("cua-driver clipboard test")
        );
    }
}
