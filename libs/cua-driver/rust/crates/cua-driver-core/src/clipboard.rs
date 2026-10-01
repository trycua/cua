//! Cross-platform clipboard tools and platform backend seam.

use std::sync::Arc;

use async_trait::async_trait;
use cua_driver_contract::{
    ClipboardReadInput, ClipboardReadOutput, ClipboardWriteInput, ClipboardWriteOutput,
};
use serde_json::Value;

use crate::{
    protocol::ToolResult,
    tool::{Tool, ToolDef, ToolRegistry},
    tool_args::parse_typed_input,
};

pub trait ClipboardBackend: Send + Sync {
    fn available_formats(&self) -> Result<Vec<String>, String>;
    fn read_text(&self) -> Result<Option<String>, String>;
    fn write_text(&self, text: String) -> Result<(), String>;
    fn write_image(&self, absolute_path: &str) -> Result<(), String>;
    fn write_file_url(&self, absolute_path: &str) -> Result<(), String>;
}

pub fn register_clipboard_tools(registry: &mut ToolRegistry, backend: Arc<dyn ClipboardBackend>) {
    registry.register(Box::new(ClipboardReadTool {
        backend: backend.clone(),
    }));
    registry.register(Box::new(ClipboardWriteTool { backend }));
}

fn contract_def(name: &str) -> ToolDef {
    let contract = cua_driver_contract::tool_contract(name)
        .unwrap_or_else(|| panic!("missing {name} clipboard contract"));
    ToolDef::from_contract(&contract)
}

fn normalize_types(mut types: Vec<String>) -> Vec<String> {
    types.sort();
    types.dedup();
    types
}

fn unavailable(operation: &str, error: impl Into<String>) -> ToolResult {
    let message = error.into();
    ToolResult::error(format!("Clipboard {operation} is unavailable: {message}")).with_structured(
        serde_json::json!({
            "supported": false,
            "capability": format!("clipboard.{operation}"),
            "status": "unavailable",
            "error_code": "clipboard_unavailable",
            "privacy_sensitive": true,
            "content_redacted_from_telemetry": true,
        }),
    )
}

pub struct ClipboardReadTool {
    backend: Arc<dyn ClipboardBackend>,
}

static READ_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for ClipboardReadTool {
    fn def(&self) -> &ToolDef {
        READ_DEF.get_or_init(|| contract_def("clipboard_read"))
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let input = match parse_typed_input::<ClipboardReadInput>("clipboard_read", args) {
            Ok(input) => input,
            Err(result) => return result,
        };
        let backend = self.backend.clone();
        let result = tokio::task::spawn_blocking(move || {
            let types = normalize_types(backend.available_formats()?);
            let text = if input.include_text {
                backend.read_text()?
            } else {
                None
            };
            Ok::<_, String>((types, text))
        })
        .await;

        let (types, text) = match result {
            Ok(Ok(value)) => value,
            Ok(Err(error)) => return unavailable("read", error),
            Err(error) => return unavailable("read", format!("worker failed: {error}")),
        };
        let output = ClipboardReadOutput {
            supported: true,
            types,
            text,
            privacy_sensitive: true,
            content_redacted_from_telemetry: true,
        };
        ToolResult::text(format!(
            "Clipboard read succeeded ({} available type(s); text {}).",
            output.types.len(),
            if input.include_text {
                "requested"
            } else {
                "omitted"
            }
        ))
        .with_structured(serde_json::to_value(output).expect("clipboard output serializes"))
    }
}

pub struct ClipboardWriteTool {
    backend: Arc<dyn ClipboardBackend>,
}

static WRITE_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for ClipboardWriteTool {
    fn def(&self) -> &ToolDef {
        WRITE_DEF.get_or_init(|| contract_def("clipboard_write"))
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let input = match parse_typed_input::<ClipboardWriteInput>("clipboard_write", args) {
            Ok(input) => input,
            Err(result) => return result,
        };
        let supplied = usize::from(input.text.is_some())
            + usize::from(input.image_path.is_some())
            + usize::from(input.file_path.is_some());
        if supplied != 1 {
            return ToolResult::error(
                "clipboard_write requires exactly one of text, image_path, or file_path",
            );
        }

        let backend = self.backend.clone();
        let result = tokio::task::spawn_blocking(move || {
            let written_type = if let Some(text) = input.text {
                backend.write_text(text)?;
                "text"
            } else if let Some(path) = input.image_path {
                backend.write_image(&path)?;
                "image"
            } else if let Some(path) = input.file_path {
                backend.write_file_url(&path)?;
                "file_url"
            } else {
                unreachable!("exactly one clipboard value was checked above")
            };
            let types = normalize_types(backend.available_formats()?);
            Ok::<_, String>((written_type, types))
        })
        .await;

        let (written_type, types) = match result {
            Ok(Ok(value)) => value,
            Ok(Err(error)) => return unavailable("write", error),
            Err(error) => return unavailable("write", format!("worker failed: {error}")),
        };
        let output = ClipboardWriteOutput {
            supported: true,
            written_type: written_type.into(),
            types,
            privacy_sensitive: true,
            content_redacted_from_telemetry: true,
        };
        ToolResult::text(format!(
            "Clipboard write succeeded as {written_type}; inspect structuredContent.types before paste."
        ))
        .with_structured(serde_json::to_value(output).expect("clipboard output serializes"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeClipboard {
        value: Mutex<Option<(String, String)>>,
    }

    struct UnavailableClipboard;

    impl ClipboardBackend for UnavailableClipboard {
        fn available_formats(&self) -> Result<Vec<String>, String> {
            Err("no display clipboard provider".into())
        }

        fn read_text(&self) -> Result<Option<String>, String> {
            Err("no display clipboard provider".into())
        }

        fn write_text(&self, _text: String) -> Result<(), String> {
            Err("no display clipboard provider".into())
        }

        fn write_image(&self, _absolute_path: &str) -> Result<(), String> {
            Err("no display clipboard provider".into())
        }

        fn write_file_url(&self, _absolute_path: &str) -> Result<(), String> {
            Err("no display clipboard provider".into())
        }
    }

    impl ClipboardBackend for FakeClipboard {
        fn available_formats(&self) -> Result<Vec<String>, String> {
            Ok(self
                .value
                .lock()
                .unwrap()
                .as_ref()
                .map(|(kind, _)| vec![kind.clone(), kind.clone()])
                .unwrap_or_default())
        }

        fn read_text(&self) -> Result<Option<String>, String> {
            Ok(self
                .value
                .lock()
                .unwrap()
                .as_ref()
                .filter(|(kind, _)| kind == "text/plain")
                .map(|(_, value)| value.clone()))
        }

        fn write_text(&self, text: String) -> Result<(), String> {
            *self.value.lock().unwrap() = Some(("text/plain".into(), text));
            Ok(())
        }

        fn write_image(&self, absolute_path: &str) -> Result<(), String> {
            *self.value.lock().unwrap() = Some(("image/png".into(), absolute_path.into()));
            Ok(())
        }

        fn write_file_url(&self, absolute_path: &str) -> Result<(), String> {
            *self.value.lock().unwrap() = Some(("text/uri-list".into(), absolute_path.into()));
            Ok(())
        }
    }

    #[tokio::test]
    async fn text_round_trip_returns_types_and_privacy_metadata() {
        let backend: Arc<dyn ClipboardBackend> = Arc::new(FakeClipboard::default());
        let write = ClipboardWriteTool {
            backend: backend.clone(),
        }
        .invoke(serde_json::json!({"text": "private value"}))
        .await;
        assert_ne!(write.is_error, Some(true));
        assert_eq!(
            write.structured_content.as_ref().unwrap()["types"],
            serde_json::json!(["text/plain"])
        );

        let read = ClipboardReadTool { backend }
            .invoke(serde_json::json!({"include_text": true}))
            .await;
        let output = read.structured_content.unwrap();
        assert_eq!(output["text"], "private value");
        assert_eq!(output["privacy_sensitive"], true);
        assert_eq!(output["content_redacted_from_telemetry"], true);
    }

    #[tokio::test]
    async fn write_requires_exactly_one_value() {
        for args in [
            serde_json::json!({}),
            serde_json::json!({"text": "x", "file_path": "/tmp/x"}),
        ] {
            let result = ClipboardWriteTool {
                backend: Arc::new(FakeClipboard::default()),
            }
            .invoke(args)
            .await;
            assert_eq!(result.is_error, Some(true));
        }
    }

    #[tokio::test]
    async fn unavailable_backend_returns_an_explicit_content_free_status() {
        let result = ClipboardReadTool {
            backend: Arc::new(UnavailableClipboard),
        }
        .invoke(serde_json::json!({}))
        .await;
        assert_eq!(result.is_error, Some(true));
        let output = result.structured_content.unwrap();
        assert_eq!(output["supported"], false);
        assert_eq!(output["status"], "unavailable");
        assert_eq!(output["error_code"], "clipboard_unavailable");
        assert_eq!(output["content_redacted_from_telemetry"], true);
        assert!(output
            .to_string()
            .find("no display clipboard provider")
            .is_none());
    }
}
