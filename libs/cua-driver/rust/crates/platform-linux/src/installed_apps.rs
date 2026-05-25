//! Linux installed-app enumeration via the XDG Desktop Entry Specification.
//!
//! Scans `.desktop` files in the standard XDG application directories and
//! parses each `[Desktop Entry]` section for `Name`, `Exec`, `NoDisplay`,
//! `Hidden`, and `Type` per
//! <https://specifications.freedesktop.org/desktop-entry-spec/latest/>.

use std::fs;
use std::path::{Path, PathBuf};

/// Parsed metadata for an installed application's `.desktop` launcher.
#[derive(Debug, Clone)]
pub struct InstalledApp {
    /// Display name from the `Name=` key (best localized variant we can read).
    pub name: String,
    /// The freedesktop "desktop file id" — the `.desktop` file's path
    /// relative to its XDG `applications/` root, with the `.desktop`
    /// suffix stripped and path separators replaced with `-`.
    /// E.g. `org.gnome.Calculator.desktop` → `org.gnome.Calculator`;
    /// `kde4/konqbrowser.desktop` → `kde4-konqbrowser`.
    pub bundle_id: String,
    /// Path the launcher would run — the unexpanded first token of `Exec=`
    /// (field codes like `%U`, `%f` stripped). Pass to `launch_app(launch_path=...)`.
    pub launch_path: String,
    /// RFC3339 timestamp from the `.desktop` file's filesystem mtime, or
    /// `None` if the metadata could not be read.
    pub last_used: Option<String>,
}

/// Return every visible application installed on the system.
///
/// Filters out entries with `NoDisplay=true`, `Hidden=true`, or `Type!=Application`.
/// XDG search order: `$XDG_DATA_HOME/applications` (defaults to
/// `~/.local/share/applications`), then each `$XDG_DATA_DIRS` entry's
/// `applications/` subdir (defaults to
/// `/usr/local/share/applications:/usr/share/applications`).
pub fn list_installed_apps() -> Vec<InstalledApp> {
    let mut seen: std::collections::HashMap<String, InstalledApp> =
        std::collections::HashMap::new();
    for root in xdg_application_dirs() {
        let Ok(entries) = fs::read_dir(&root) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("desktop") { continue }
            let id = desktop_file_id(&root, &path);
            if id.is_empty() { continue }
            let Some(app) = parse_desktop_file(&path, &id) else { continue };
            // Per the spec, user-scope files (XDG_DATA_HOME) override
            // system-scope ones with the same desktop file id.
            // xdg_application_dirs already yields user-scope first, so
            // insert-if-absent gives us the precedence right.
            seen.entry(app.bundle_id.clone()).or_insert(app);
        }
    }
    let mut out: Vec<InstalledApp> = seen.into_values().collect();
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

/// Build the search-path list per the XDG Base Directory Specification.
fn xdg_application_dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let home = std::env::var("HOME").unwrap_or_default();
    let xdg_data_home = std::env::var("XDG_DATA_HOME").unwrap_or_else(|_| {
        if home.is_empty() { String::new() } else { format!("{home}/.local/share") }
    });
    if !xdg_data_home.is_empty() {
        out.push(PathBuf::from(format!("{xdg_data_home}/applications")));
    }
    let xdg_data_dirs = std::env::var("XDG_DATA_DIRS")
        .unwrap_or_else(|_| "/usr/local/share:/usr/share".to_owned());
    for d in xdg_data_dirs.split(':').filter(|s| !s.is_empty()) {
        out.push(PathBuf::from(format!("{d}/applications")));
    }
    out
}

/// Compute the canonical "desktop file id" per the freedesktop Desktop Entry
/// spec: the path of the `.desktop` file relative to its XDG `applications/`
/// root, with the `.desktop` suffix stripped and any path separators replaced
/// with `-`. This is what associations/launchers reference and guarantees
/// uniqueness across nested category dirs (e.g. `category/foo.desktop` →
/// `category-foo`, distinct from a sibling `foo.desktop` → `foo`).
///
/// Returns an empty string when `path` is not under `root` or has no
/// `.desktop` suffix.
fn desktop_file_id(root: &Path, path: &Path) -> String {
    let rel = match path.strip_prefix(root) {
        Ok(r) => r,
        Err(_) => return String::new(),
    };
    let rel_str = match rel.to_str() {
        Some(s) => s,
        None => return String::new(),
    };
    let stem = match rel_str.strip_suffix(".desktop") {
        Some(s) => s,
        None => return String::new(),
    };
    // Use forward slash as the canonical separator (Linux only — `Path`
    // separators are always `/` here, but normalize defensively).
    stem.replace(std::path::MAIN_SEPARATOR, "-").replace('/', "-")
}

/// Parse a `.desktop` file. Returns `None` for entries the caller should skip
/// (`NoDisplay=true`, `Hidden=true`, `Type!=Application`, missing `Exec` or `Name`).
///
/// `bundle_id` is the precomputed desktop file id (see `desktop_file_id`).
fn parse_desktop_file(path: &Path, bundle_id: &str) -> Option<InstalledApp> {
    let text = fs::read_to_string(path).ok()?;
    let entry = extract_desktop_entry_section(&text)?;

    if bool_key(&entry, "NoDisplay") || bool_key(&entry, "Hidden") {
        return None;
    }
    let type_ = string_key(&entry, "Type").unwrap_or_default();
    if !type_.is_empty() && type_ != "Application" {
        return None;
    }

    let name = string_key(&entry, "Name")?;
    let exec_raw = string_key(&entry, "Exec")?;
    let launch_path = strip_exec_field_codes(&exec_raw);
    if launch_path.is_empty() {
        return None;
    }

    if bundle_id.is_empty() {
        return None;
    }
    let bundle_id = bundle_id.to_owned();

    let last_used = fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| unix_secs_to_rfc3339(d.as_secs() as i64));

    Some(InstalledApp {
        name,
        bundle_id,
        launch_path,
        last_used,
    })
}

/// Return the `[Desktop Entry]` section's key/value lines as one blob.
/// We only need a flat string lookup, so a Vec<(String, String)> is overkill —
/// pull the section into a `String` and let `string_key` re-scan it.
fn extract_desktop_entry_section(text: &str) -> Option<String> {
    let mut in_section = false;
    let mut out = String::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(header) = trimmed.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
            in_section = header == "Desktop Entry";
            continue;
        }
        if in_section && !trimmed.is_empty() && !trimmed.starts_with('#') {
            out.push_str(line);
            out.push('\n');
        }
    }
    if out.is_empty() { None } else { Some(out) }
}

fn string_key(section: &str, key: &str) -> Option<String> {
    // Prefer a localized variant matching $LANG when present (`Key[xx]=...`).
    // Fall back to the bare `Key=...`. We only look at the language root
    // (everything before the first `.` or `_`), so `en_US.UTF-8` matches `en`.
    let lang = std::env::var("LANG").unwrap_or_default();
    let lang_root: String = lang
        .split(|c: char| c == '.' || c == '@')
        .next()
        .unwrap_or("")
        .to_owned();
    let lang_short: String = lang_root
        .split('_')
        .next()
        .unwrap_or("")
        .to_owned();

    let candidates = [
        format!("{key}[{lang_root}]="),
        format!("{key}[{lang_short}]="),
        format!("{key}="),
    ];
    for cand in &candidates {
        if cand.starts_with(&format!("{key}[]=")) { continue }
        for line in section.lines() {
            if let Some(rest) = line.strip_prefix(cand.as_str()) {
                let value = rest.trim_end_matches('\r').to_owned();
                if !value.is_empty() {
                    return Some(value);
                }
            }
        }
    }
    None
}

fn bool_key(section: &str, key: &str) -> bool {
    string_key(section, key)
        .map(|v| v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// Strip XDG `Exec=` field codes (`%f`, `%F`, `%u`, `%U`, `%i`, `%c`, `%k`)
/// and return only the program token. We deliberately don't try to honor
/// quoted argv reconstruction — the caller passes the result to
/// `launch_app(launch_path=...)`, which spawns it through `sh -c`.
fn strip_exec_field_codes(exec: &str) -> String {
    let mut out = String::with_capacity(exec.len());
    let mut chars = exec.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%' {
            // Consume the field-code letter, drop both.
            chars.next();
            continue;
        }
        out.push(c);
    }
    out.trim().to_owned()
}

/// Format Unix epoch seconds as `YYYY-MM-DDTHH:MM:SSZ` (UTC).
fn unix_secs_to_rfc3339(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let seconds_of_day = secs.rem_euclid(86_400);
    let hour = seconds_of_day / 3600;
    let minute = (seconds_of_day % 3600) / 60;
    let second = seconds_of_day % 60;

    // Howard Hinnant's civil-from-days algorithm.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", y, m, d, hour, minute, second)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_field_codes() {
        assert_eq!(strip_exec_field_codes("firefox %U"), "firefox");
        assert_eq!(strip_exec_field_codes("/usr/bin/gedit %f"), "/usr/bin/gedit");
        assert_eq!(strip_exec_field_codes("xterm -e %F"), "xterm -e");
    }

    #[test]
    fn skips_nodisplay_entry() {
        let body = "\
[Desktop Entry]
Type=Application
Name=Hidden Helper
Exec=/usr/libexec/helper
NoDisplay=true
";
        let dir = std::env::temp_dir().join(format!("cua-driver-rs-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("hidden-helper.desktop");
        std::fs::write(&path, body).unwrap();
        assert!(parse_desktop_file(&path, "hidden-helper").is_none());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn parses_minimal_entry() {
        let body = "\
[Desktop Entry]
Type=Application
Name=Demo App
Exec=/opt/demo/bin/demo %U
";
        let dir = std::env::temp_dir().join(format!("cua-driver-rs-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("demo-app.desktop");
        std::fs::write(&path, body).unwrap();
        let parsed = parse_desktop_file(&path, "demo-app").expect("parses");
        assert_eq!(parsed.name, "Demo App");
        assert_eq!(parsed.launch_path, "/opt/demo/bin/demo");
        assert_eq!(parsed.bundle_id, "demo-app");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn desktop_file_id_flat() {
        let root = Path::new("/usr/share/applications");
        let path = Path::new("/usr/share/applications/firefox.desktop");
        assert_eq!(desktop_file_id(root, path), "firefox");
    }

    #[test]
    fn desktop_file_id_nested_disambiguates() {
        // Two distinct .desktop files that would have collided under the
        // old `file_stem()`-only scheme now get unique ids.
        let root = Path::new("/usr/share/applications");
        let flat = Path::new("/usr/share/applications/foo.desktop");
        let nested = Path::new("/usr/share/applications/category/foo.desktop");
        assert_eq!(desktop_file_id(root, flat), "foo");
        assert_eq!(desktop_file_id(root, nested), "category-foo");
        assert_ne!(
            desktop_file_id(root, flat),
            desktop_file_id(root, nested),
            "nested .desktop entries must not collide with flat ones"
        );
    }

    #[test]
    fn desktop_file_id_returns_empty_outside_root() {
        let root = Path::new("/usr/share/applications");
        let other = Path::new("/opt/something.desktop");
        assert_eq!(desktop_file_id(root, other), "");
    }

    #[test]
    fn rfc3339_known_epoch() {
        // 1234567890 → 2009-02-13T23:31:30Z
        assert_eq!(unix_secs_to_rfc3339(1_234_567_890), "2009-02-13T23:31:30Z");
    }
}
