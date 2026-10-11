use std::fs;
use std::path::Path;
use std::thread::sleep;
use std::time::{Duration, Instant};

/// Read app-owned fixture state until its text value has remained unchanged
/// for a bounded interval. This observes the control model directly, not the
/// driver's post-action accessibility snapshot.
pub fn stable_text(path: &Path, automation_id: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut last_value: Option<String> = None;
    let mut unchanged_since = Instant::now();

    loop {
        if let Ok(bytes) = fs::read(path) {
            if let Ok(state) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                if let Some(value) = state[automation_id]["text"].as_str() {
                    let value = value.to_owned();
                    if last_value.as_ref() != Some(&value) {
                        last_value = Some(value);
                        unchanged_since = Instant::now();
                    } else if unchanged_since.elapsed() >= Duration::from_millis(500) {
                        return value;
                    }
                }
            }
        }

        assert!(
            Instant::now() < deadline,
            "fixture state did not stabilize for {automation_id}; last value: {}",
            last_value
                .as_deref()
                .map(ascii_escape)
                .unwrap_or_else(|| "<missing>".into())
        );
        sleep(Duration::from_millis(25));
    }
}

pub fn normalize_line_endings(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

pub fn ascii_escape(text: &str) -> String {
    text.chars()
        .map(|ch| match ch {
            '\n' => "\\n".to_owned(),
            '\r' => "\\r".to_owned(),
            '\t' => "\\t".to_owned(),
            ch if ch.is_ascii_graphic() || ch == ' ' => ch.to_string(),
            ch => format!("\\u{{{:x}}}", ch as u32),
        })
        .collect()
}
