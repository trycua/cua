//! Best-effort Sway/i3-compatible compositor metadata.
//!
//! The wlroots foreign-toplevel protocol exposes titles and app ids, but not
//! process ids or geometry. Sway's IPC tree supplies those missing fields and
//! a compositor-stable container id. Other compositors simply return no data.

use std::process::{Command, Stdio};

use serde::Deserialize;

#[derive(Clone, Debug, Default, Deserialize)]
struct Rect {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct Node {
    id: u64,
    #[serde(default)]
    name: String,
    #[serde(default)]
    app_id: String,
    pid: Option<u32>,
    #[serde(default)]
    rect: Rect,
    #[serde(default)]
    window_rect: Rect,
    #[serde(default)]
    deco_rect: Rect,
    #[serde(default)]
    focused: bool,
    #[serde(default = "default_visible")]
    visible: bool,
    #[serde(default)]
    fullscreen_mode: i32,
    #[serde(default)]
    nodes: Vec<Node>,
    #[serde(default)]
    floating_nodes: Vec<Node>,
}

fn default_visible() -> bool {
    true
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Window {
    pub id: u64,
    pub pid: u32,
    pub title: String,
    pub app_id: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub content_x: i32,
    pub content_y: i32,
    pub focused: bool,
    pub visible: bool,
    pub fullscreen: bool,
}

fn collect(node: &Node, windows: &mut Vec<Window>) {
    if let Some(pid) = node.pid {
        if !node.name.is_empty() || !node.app_id.is_empty() {
            // Sway normally reports the client surface origin in
            // `window_rect`. Some server-decorated Wayland clients instead
            // leave that origin at zero and expose the title-bar inset only
            // through `deco_rect`; use it as the content origin in that shape.
            let content_y = if node.window_rect.y != 0 {
                node.window_rect.y
            } else {
                node.deco_rect
                    .y
                    .saturating_add(node.deco_rect.height.max(0))
            };
            windows.push(Window {
                id: node.id,
                pid,
                title: node.name.clone(),
                app_id: node.app_id.clone(),
                x: node.rect.x,
                y: node.rect.y,
                width: node.rect.width.max(0) as u32,
                height: node.rect.height.max(0) as u32,
                content_x: node.window_rect.x,
                content_y,
                focused: node.focused,
                visible: node.visible,
                fullscreen: node.fullscreen_mode != 0,
            });
        }
    }
    for child in node.nodes.iter().chain(&node.floating_nodes) {
        collect(child, windows);
    }
}

fn parse_tree(bytes: &[u8]) -> Option<Vec<Window>> {
    let root: Node = serde_json::from_slice(bytes).ok()?;
    let mut windows = Vec::new();
    collect(&root, &mut windows);
    Some(windows)
}

pub fn list_windows() -> Option<Vec<Window>> {
    if std::env::var_os("SWAYSOCK").is_none() {
        return None;
    }
    let output = Command::new("swaymsg")
        .args(["-r", "-t", "get_tree"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_tree(&output.stdout)
}

pub fn window_for_id(id: u64) -> Option<Window> {
    list_windows()?.into_iter().find(|window| window.id == id)
}

pub fn window_for_pid(pid: u32) -> Option<Window> {
    list_windows()?
        .into_iter()
        .filter(|window| window.pid == pid && window.width > 0 && window.height > 0)
        .max_by_key(|window| {
            (
                window.focused,
                window.visible,
                u64::from(window.width) * u64::from(window.height),
            )
        })
}

pub fn window_for_title(title: &str) -> Option<Window> {
    list_windows()?
        .into_iter()
        .filter(|window| {
            window.width > 0
                && window.height > 0
                && (window.title == title
                    || (!window.title.is_empty() && title.starts_with(&window.title))
                    || (!title.is_empty() && window.title.starts_with(title)))
        })
        .max_by_key(|window| (window.focused, window.visible))
}

pub fn window_for_app_id(app_id: &str) -> Option<Window> {
    if app_id.is_empty() {
        return None;
    }
    list_windows()?
        .into_iter()
        .filter(|window| window.width > 0 && window.height > 0 && window.app_id == app_id)
        .max_by_key(|window| (window.focused, window.visible))
}

pub fn window_origin_for_pid(pid: u32) -> Option<(i32, i32)> {
    window_for_pid(pid).map(|window| (window.x, window.y))
}

/// Offset of the application content inside Sway's captured toplevel.
/// Server-side decorations are part of `rect`/window screenshots but not of
/// WebKitGTK's AT-SPI `CoordType::Window` descendants.
pub fn window_content_offset_for_pid(pid: u32) -> Option<(i32, i32)> {
    window_for_pid(pid).map(|window| (window.content_x, window.content_y))
}

pub fn window_origin_for_title(title: &str) -> Option<(i32, i32)> {
    let window = window_for_title(title)?;
    Some((window.x, window.y))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nested_and_floating_windows() {
        let tree = br#"{
          "id": 1,
          "nodes": [{
            "id": 2,
            "nodes": [{
              "id": 10,
              "name": "Editor",
              "app_id": "org.example.Editor",
              "pid": 123,
              "rect": {"x": 20, "y": 30, "width": 800, "height": 600},
              "window_rect": {"x": 0, "y": 47, "width": 800, "height": 553},
              "focused": true,
              "visible": true,
              "fullscreen_mode": 1
            }],
            "floating_nodes": [{
              "id": 11,
              "name": "Dialog",
              "pid": 124,
              "rect": {"x": 100, "y": 120, "width": 300, "height": 200}
            }]
          }]
        }"#;
        let windows = parse_tree(tree).expect("parse Sway tree");
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].pid, 123);
        assert_eq!((windows[0].x, windows[0].y), (20, 30));
        assert_eq!((windows[0].content_x, windows[0].content_y), (0, 47));
        assert!(windows[0].focused);
        assert!(windows[0].fullscreen);
        assert_eq!(windows[1].title, "Dialog");
    }

    #[test]
    fn decoration_height_fills_zero_window_content_origin() {
        let tree = br#"{
          "id": 1,
          "nodes": [{
            "id": 10,
            "name": "Tauri",
            "app_id": "cua-test-harness",
            "pid": 123,
            "rect": {"x": 0, "y": 0, "width": 940, "height": 780},
            "window_rect": {"x": 0, "y": 0, "width": 940, "height": 733},
            "deco_rect": {"x": 0, "y": 0, "width": 940, "height": 47}
          }]
        }"#;
        let windows = parse_tree(tree).expect("parse decorated Sway tree");
        assert_eq!((windows[0].content_x, windows[0].content_y), (0, 47));
    }
}
