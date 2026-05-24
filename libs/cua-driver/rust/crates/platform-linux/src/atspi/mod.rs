//! AT-SPI accessibility tree walking for Linux.
//!
//! AT-SPI2 is accessible via D-Bus. Rather than linking the full D-Bus library,
//! we query AT-SPI via the `gdbus` or `dbus-send` CLI tool as a subprocess,
//! or we use the `atspi` Rust crate if available.
//!
//! For a pure-Rust implementation without D-Bus library deps, we shell out to:
//!   python3 -c "import pyatspi; ..." (if available)
//!   OR
//!   Use x11rb to read basic XA_WM_* properties as a fallback tree.
//!
//! The fallback produces a simplified tree with window title and role.

use anyhow::Result;
use std::process::Command;

pub mod cache;
pub use cache::ElementCache;

#[derive(Clone, Debug)]
pub struct AtspiNode {
    pub element_index: Option<usize>,
    pub role: String,
    pub name: Option<String>,
    pub value: Option<String>,
    pub description: Option<String>,
    pub actions: Vec<String>,
    /// For pyatspi path: element_key = element_index as u64.
    /// For X11 fallback: element_key = xid.
    pub element_key: u64,
}

pub struct AtspiTreeResult {
    pub tree_markdown: String,
    pub nodes: Vec<AtspiNode>,
}

/// Walk the AT-SPI tree for a window identified by (pid, xid).
/// Falls back to a minimal X11 property tree if AT-SPI is unavailable.
pub fn walk_tree(pid: u32, xid: u64, query: Option<&str>) -> AtspiTreeResult {
    // Try pyatspi bridge first (most complete).
    if let Ok((raw_md, nodes)) = walk_via_pyatspi(pid) {
        let md = if let Some(q) = query { filter_tree(&raw_md, q) } else { raw_md };
        return AtspiTreeResult { tree_markdown: md, nodes };
    }

    // Fallback: X11 window properties as minimal tree.
    walk_via_x11_properties(xid, query)
}

/// Perform the first advertised action on element `idx` within pid's app tree.
/// Returns Ok(action_name) on success.
pub fn perform_action(pid: u32, idx: usize) -> Result<String> {
    let script = format!(r#"
import pyatspi, sys

elements = []
def collect(acc):
    try:
        ai = acc.queryAction()
        if ai.nActions > 0:
            elements.append((acc, [ai.getName(i) for i in range(ai.nActions)]))
    except: pass
    for child in acc:
        collect(child)

desktop = pyatspi.Registry.getDesktop(0)
for app in desktop:
    if app.get_process_id() == {pid}:
        for win in app:
            collect(win)
        break

if {idx} < len(elements):
    elem, actions = elements[{idx}]
    elem.queryAction().doAction(0)
    print(actions[0])
else:
    print("ERROR: element {idx} not found (total: " + str(len(elements)) + ")", file=sys.stderr)
    sys.exit(1)
"#, pid = pid, idx = idx);

    let out = Command::new("python3").arg("-c").arg(&script).output()?;
    if !out.status.success() {
        anyhow::bail!("{}", String::from_utf8_lossy(&out.stderr).trim().to_owned());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// Set the text value of element `idx` within pid's app tree via AT-SPI.
/// Tries `queryEditableText().setTextContents(value)` first,
/// then `queryValue().setCurrentValue(float)`.
pub fn set_value(pid: u32, idx: usize, value: &str) -> Result<()> {
    // Escape value for Python string literal: replace \ and ' to be safe.
    let safe_value = value.replace('\\', "\\\\").replace('\'', "\\'");
    let script = format!(r#"
import pyatspi, sys

elements = []
def collect(acc):
    try:
        ai = acc.queryAction()
        if ai.nActions > 0:
            elements.append(acc)
    except: pass
    for child in acc:
        collect(child)

desktop = pyatspi.Registry.getDesktop(0)
for app in desktop:
    if app.get_process_id() == {pid}:
        for win in app:
            collect(win)
        break

if {idx} >= len(elements):
    print(f"ERROR: element {idx} not found (total: {{len(elements)}})", file=sys.stderr)
    sys.exit(1)

elem = elements[{idx}]
try:
    et = elem.queryEditableText()
    et.setTextContents('{safe_value}')
    print("ok:text")
except:
    try:
        v = elem.queryValue()
        v.setCurrentValue(float('{safe_value}'))
        print("ok:value")
    except Exception as e:
        print(f"ERROR: {{e}}", file=sys.stderr)
        sys.exit(1)
"#, pid = pid, idx = idx, safe_value = safe_value);

    let out = Command::new("python3").arg("-c").arg(&script).output()?;
    if !out.status.success() {
        anyhow::bail!("{}", String::from_utf8_lossy(&out.stderr).trim().to_owned());
    }
    Ok(())
}

/// Get the screen-coordinate bounding box (x, y, width, height) of element `idx`.
pub fn get_element_bounds(pid: u32, idx: usize) -> Result<(i32, i32, u32, u32)> {
    let script = format!(r#"
import pyatspi, sys

elements = []
def collect(acc):
    try:
        ai = acc.queryAction()
        if ai.nActions > 0:
            elements.append(acc)
    except: pass
    for child in acc:
        collect(child)

desktop = pyatspi.Registry.getDesktop(0)
for app in desktop:
    if app.get_process_id() == {pid}:
        for win in app:
            collect(win)
        break

if {idx} >= len(elements):
    print(f"ERROR: element {idx} not found", file=sys.stderr)
    sys.exit(1)

elem = elements[{idx}]
try:
    comp = elem.queryComponent()
    ext = comp.getExtents(pyatspi.DESKTOP_COORDS)
    print(f"{{ext.x}},{{ext.y}},{{ext.width}},{{ext.height}}")
except Exception as e:
    print(f"ERROR: {{e}}", file=sys.stderr)
    sys.exit(1)
"#, pid = pid, idx = idx);

    let out = Command::new("python3").arg("-c").arg(&script).output()?;
    if !out.status.success() {
        anyhow::bail!("{}", String::from_utf8_lossy(&out.stderr).trim().to_owned());
    }
    let line = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    let parts: Vec<i64> = line.split(',')
        .filter_map(|s| s.parse().ok())
        .collect();
    if parts.len() < 4 { anyhow::bail!("unexpected bounds output: {line}"); }
    Ok((parts[0] as i32, parts[1] as i32, parts[2] as u32, parts[3] as u32))
}

// ── Internal helpers ─────────────────────────────────────────────────────────

/// Walk via pyatspi subprocess. Returns (markdown, nodes) on success.
fn walk_via_pyatspi(pid: u32) -> Result<(String, Vec<AtspiNode>)> {
    let script = format!(r#"
import pyatspi, sys

def walk(acc, depth=0, idx=[0]):
    role = acc.getRoleName()
    name = acc.name or ""
    n_actions = 0
    actions = []
    try:
        ai = acc.queryAction()
        n_actions = ai.nActions
        actions = [ai.getName(i) for i in range(n_actions)]
    except:
        pass

    try:
        vobj = acc.queryValue()
        value_str = str(vobj.currentValue)
    except:
        value_str = ""

    indent = "  " * depth
    if n_actions > 0:
        act_str = ','.join(actions)
        val_part = f' value="{{value_str}}"' if value_str else ''
        print(f"{{indent}}- [{{idx[0]}}] {{role}} \"{{name}}\"{{val_part}} [actions=[{{act_str}}]]")
        idx[0] += 1
    elif name:
        print(f"{{indent}}- {{role}} = \"{{name}}\"")

    for child in acc:
        walk(child, depth+1, idx)

desktop = pyatspi.Registry.getDesktop(0)
for app in desktop:
    if app.get_process_id() == {pid}:
        for win in app:
            walk(win)
        break
"#, pid = pid);

    let out = Command::new("python3")
        .arg("-c")
        .arg(&script)
        .output()?;

    if !out.status.success() || out.stdout.is_empty() {
        anyhow::bail!("pyatspi not available or returned empty");
    }

    let raw_md = String::from_utf8_lossy(&out.stdout).into_owned();
    let nodes = parse_pyatspi_nodes(&raw_md);
    Ok((raw_md, nodes))
}

/// Parse pyatspi markdown output into AtspiNode list.
///
/// Recognizes lines like:
///   `  - [3] button "OK" value="1.0" [actions=[click,press,release]]`
fn parse_pyatspi_nodes(md: &str) -> Vec<AtspiNode> {
    let mut nodes = Vec::new();
    // We need indexed_nodes in order so element_index == position in vec.
    let mut indexed: Vec<(usize, AtspiNode)> = Vec::new();

    for line in md.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with('-') { continue; }
        let rest = trimmed.trim_start_matches('-').trim();

        // Check for indexed element: `[N] role "name" ...`
        if rest.starts_with('[') {
            if let Some(close) = rest.find(']') {
                let idx_str = &rest[1..close];
                if let Ok(idx) = idx_str.parse::<usize>() {
                    let after_idx = rest[close+1..].trim();

                    // Parse role (first word) and name (quoted string).
                    let (role, after_role) = split_first_word(after_idx);
                    let (name, after_name) = parse_quoted_string(after_role.trim());

                    // Parse optional value="..." field.
                    let value = parse_field(after_name, "value=");

                    // Parse [actions=[...]] field.
                    let actions = parse_actions(after_name);

                    let node = AtspiNode {
                        element_index: Some(idx),
                        role: role.to_owned(),
                        name: if name.is_empty() { None } else { Some(name.to_owned()) },
                        value: if value.is_empty() { None } else { Some(value.to_owned()) },
                        description: None,
                        actions,
                        element_key: idx as u64,
                    };
                    indexed.push((idx, node));
                }
            }
        }
    }

    // Sort by index and flatten.
    indexed.sort_by_key(|(i, _)| *i);
    nodes.extend(indexed.into_iter().map(|(_, n)| n));
    nodes
}

fn split_first_word(s: &str) -> (&str, &str) {
    let s = s.trim();
    if let Some(pos) = s.find(|c: char| c.is_whitespace()) {
        (&s[..pos], &s[pos..])
    } else {
        (s, "")
    }
}

/// Parse a `"quoted string"` from the start of `s`. Returns (content, rest).
fn parse_quoted_string(s: &str) -> (&str, &str) {
    let s = s.trim();
    if !s.starts_with('"') { return ("", s); }
    let inner = &s[1..];
    if let Some(end) = inner.find('"') {
        (&inner[..end], &inner[end+1..])
    } else {
        ("", s)
    }
}

/// Parse a field like `value="something"` from a string.
fn parse_field<'a>(s: &'a str, prefix: &str) -> &'a str {
    if let Some(pos) = s.find(prefix) {
        let after = &s[pos + prefix.len()..];
        let (content, _) = parse_quoted_string(after);
        content
    } else {
        ""
    }
}

/// Parse `[actions=[click,press,release]]` from a string.
fn parse_actions(s: &str) -> Vec<String> {
    // Find [actions=[...]]
    if let Some(start) = s.find("[actions=[") {
        let inner = &s[start + "[actions=[".len()..];
        if let Some(end) = inner.find("]]") {
            return inner[..end].split(',')
                .map(|a| a.trim().to_owned())
                .filter(|a| !a.is_empty())
                .collect();
        }
    }
    vec![]
}

/// Minimal X11 property-based tree (fallback when AT-SPI is unavailable).
fn walk_via_x11_properties(xid: u64, query: Option<&str>) -> AtspiTreeResult {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::*;
    use x11rb::rust_connection::RustConnection;

    let (conn, _) = match RustConnection::connect(None) {
        Ok(r) => r,
        Err(_) => return AtspiTreeResult { tree_markdown: String::new(), nodes: vec![] },
    };

    let window = xid as u32;

    // Read window title.
    let title = get_x11_title(&conn, window).unwrap_or_default();

    // Read WM_CLASS.
    let wm_class = get_x11_wm_class(&conn, window).unwrap_or_default();

    let mut md = String::new();
    let mut nodes = vec![];

    let root_node = AtspiNode {
        element_index: Some(0),
        role: "window".into(),
        name: if title.is_empty() { None } else { Some(title.clone()) },
        value: None,
        description: if wm_class.is_empty() { None } else { Some(wm_class.clone()) },
        actions: vec!["activate".into()],
        element_key: xid,
    };
    md.push_str(&format!("- [0] window \"{}\" [actions=[activate]]\n", title));
    nodes.push(root_node);

    let raw_md = md;
    let tree_markdown = if let Some(q) = query {
        filter_tree(&raw_md, q)
    } else {
        raw_md
    };

    AtspiTreeResult { tree_markdown, nodes }
}

fn get_x11_title(conn: &x11rb::rust_connection::RustConnection, window: u32) -> Option<String> {
    use x11rb::protocol::xproto::*;
    // Try _NET_WM_NAME first.
    let net_wm_name = conn.intern_atom(false, b"_NET_WM_NAME").ok()?.reply().ok()?.atom;
    let utf8_string = conn.intern_atom(false, b"UTF8_STRING").ok()?.reply().ok()?.atom;
    if let Ok(reply) = conn.get_property(false, window, net_wm_name, utf8_string, 0, 1024).ok()?.reply() {
        if !reply.value.is_empty() {
            return Some(String::from_utf8_lossy(&reply.value).into_owned());
        }
    }
    let reply = conn.get_property(false, window, AtomEnum::WM_NAME, AtomEnum::STRING, 0, 1024).ok()?.reply().ok()?;
    Some(String::from_utf8_lossy(&reply.value).into_owned())
}

fn get_x11_wm_class(conn: &x11rb::rust_connection::RustConnection, window: u32) -> Option<String> {
    use x11rb::protocol::xproto::*;
    let reply = conn.get_property(false, window, AtomEnum::WM_CLASS, AtomEnum::STRING, 0, 512).ok()?.reply().ok()?;
    let s = String::from_utf8_lossy(&reply.value);
    // WM_CLASS is two NUL-separated strings: instance_name\0class_name\0
    Some(s.trim_end_matches('\0').replace('\0', "."))
}

fn filter_tree(markdown: &str, query: &str) -> String {
    let needle = query.to_lowercase();
    let lines: Vec<&str> = markdown.lines().collect();
    let mut ancestors: Vec<&str> = Vec::new();
    let mut last_emitted: Vec<Option<&str>> = Vec::new();
    let mut output: Vec<&str> = Vec::new();

    for line in &lines {
        let depth = line.chars().take_while(|c| *c == ' ').count() / 2;
        while ancestors.len() <= depth { ancestors.push(""); last_emitted.push(None); }
        for d in (depth+1)..ancestors.len() { last_emitted[d] = None; }
        ancestors[depth] = line;
        if line.to_lowercase().contains(&needle) {
            for d in 0..depth {
                if ancestors[d].is_empty() { continue; }
                if last_emitted[d] == Some(ancestors[d]) { continue; }
                last_emitted[d] = Some(ancestors[d]);
                output.push(ancestors[d]);
            }
            last_emitted[depth] = Some(line);
            output.push(line);
        }
    }
    if output.is_empty() { return String::new(); }
    let mut r = output.join("\n"); r.push('\n'); r
}
