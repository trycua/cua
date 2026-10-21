//! Sway metadata and focus over the native i3-ipc socket.
//!
//! The wlroots foreign-toplevel protocol exposes titles and app ids, but not
//! process ids or geometry. Sway's IPC tree supplies those missing fields and
//! a compositor-stable container id. Each request opens a connection to
//! `SWAYSOCK`, checks that its peer is the process serving `WAYLAND_DISPLAY`,
//! and finishes by its deadline. Other compositors, or a socket that fails
//! that check, return no data.

use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use super::compositor_peer::{peer_pid, wayland_socket_with_timeout};

const MAGIC: &[u8; 6] = b"i3-ipc";
const RUN_COMMAND: u32 = 0;
const GET_TREE: u32 = 4;
const QUERY_CAP: Duration = Duration::from_secs(1);
const MAX_REPLY_BYTES: usize = 16 * 1024 * 1024;

fn remaining(deadline: Instant) -> Result<Duration> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        bail!("Sway IPC timed out");
    }
    Ok(left)
}

fn frame(kind: u32, payload: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(14 + payload.len());
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&(payload.len() as u32).to_ne_bytes());
    bytes.extend_from_slice(&kind.to_ne_bytes());
    bytes.extend_from_slice(payload);
    bytes
}

fn read_reply(stream: &mut impl Read, kind: u32) -> Result<Vec<u8>> {
    let mut header = [0u8; 14];
    stream
        .read_exact(&mut header)
        .context("Sway IPC reply unavailable")?;
    if &header[..6] != MAGIC {
        bail!("Sway IPC reply has no i3-ipc header");
    }
    let length = u32::from_ne_bytes(header[6..10].try_into()?) as usize;
    if u32::from_ne_bytes(header[10..14].try_into()?) != kind {
        bail!("Sway IPC replied to a different request");
    }
    if length > MAX_REPLY_BYTES {
        bail!("Sway IPC reply exceeds size limit");
    }
    let mut payload = vec![0u8; length];
    stream
        .read_exact(&mut payload)
        .context("Sway IPC reply truncated")?;
    Ok(payload)
}

/// Send one `kind` request over a newly attested `SWAYSOCK` connection and
/// read its reply by `deadline`.
fn request(kind: u32, payload: &[u8], deadline: Instant) -> Result<Vec<u8>> {
    let path = std::env::var_os("SWAYSOCK").context("missing SWAYSOCK")?;
    let socket = socket2::Socket::new(socket2::Domain::UNIX, socket2::Type::STREAM, None)?;
    socket.connect_timeout(&socket2::SockAddr::unix(path)?, remaining(deadline)?)?;
    let mut stream: UnixStream = socket.into();
    let wayland = wayland_socket_with_timeout(remaining(deadline)?)?;
    if peer_pid(stream.as_raw_fd())? != peer_pid(wayland.as_raw_fd())? {
        bail!("SWAYSOCK and WAYLAND_DISPLAY name different compositor processes");
    }
    stream.set_write_timeout(Some(remaining(deadline)?))?;
    stream.write_all(&frame(kind, payload))?;
    stream.set_read_timeout(Some(remaining(deadline)?))?;
    read_reply(&mut stream, kind)
}

#[derive(Deserialize)]
struct Outcome {
    success: bool,
    #[serde(default)]
    error: Option<String>,
}

fn command_outcome(command: &str, reply: &[u8]) -> Result<()> {
    let outcomes: Vec<Outcome> =
        serde_json::from_slice(reply).context("invalid Sway command reply")?;
    if outcomes.is_empty() {
        bail!("Sway ran no command for `{command}`");
    }
    if let Some(failed) = outcomes.iter().find(|outcome| !outcome.success) {
        bail!(
            "Sway refused `{command}`: {}",
            failed.error.as_deref().unwrap_or("no reason given")
        );
    }
    Ok(())
}

fn tree(deadline: Instant) -> Result<Vec<Window>> {
    parse_tree(&request(GET_TREE, b"", deadline)?).context("invalid Sway IPC tree")
}

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
            let inferred_top_inset = if node.window_rect.height > 0 {
                node.rect
                    .height
                    .saturating_sub(node.window_rect.height)
                    .max(0)
            } else {
                0
            };
            let content_y = if node.window_rect.y != 0 {
                node.window_rect.y
            } else {
                node.deco_rect
                    .y
                    .saturating_add(node.deco_rect.height.max(0))
                    .max(inferred_top_inset)
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
    std::env::var_os("SWAYSOCK")?;
    tree(crate::interaction::operation_deadline(QUERY_CAP)).ok()
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

/// The focused container's PID and id by `deadline`, `None` when no window
/// has focus.
pub fn focused(deadline: Instant) -> Result<Option<(u32, u64)>> {
    Ok(tree(deadline)?
        .into_iter()
        .find(|window| window.focused)
        .map(|window| (window.pid, window.id)))
}

/// Focus exact container `id` of `pid` and read the focus back by
/// `deadline`.
pub fn focus(pid: u32, id: u64, deadline: Instant) -> Result<()> {
    let command = format!("[con_id={id}] focus");
    request(RUN_COMMAND, command.as_bytes(), deadline)
        .and_then(|reply| command_outcome(&command, &reply))
        .with_context(|| format!("Sway refused to focus exact container {id}"))?;
    let confirmed = crate::interaction::confirm(deadline, |deadline| {
        let observed = match focused(deadline) {
            Ok(focused) => crate::interaction::facts::Fact::Known(focused),
            Err(error) => crate::interaction::facts::Fact::Unknown(error.to_string()),
        };
        Ok::<_, ()>(crate::interaction::policy::focused(&(pid, id), &observed))
    })
    .unwrap_or(false);
    if !confirmed {
        bail!("Sway did not confirm focus on exact container {id}");
    }
    Ok(())
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
    fn frames_round_trip_and_reject_foreign_headers() {
        let framed = frame(GET_TREE, b"{}");
        assert_eq!(&framed[..6], MAGIC);
        assert_eq!(read_reply(&mut &framed[..], GET_TREE).unwrap(), b"{}");
        assert!(read_reply(&mut &framed[..], RUN_COMMAND).is_err());
        assert!(read_reply(&mut &framed[..framed.len() - 1], GET_TREE).is_err());
        let mut foreign = framed.clone();
        foreign[0] = b'x';
        assert!(read_reply(&mut &foreign[..], GET_TREE).is_err());
        let mut oversized = frame(GET_TREE, b"");
        oversized[6..10].copy_from_slice(&(MAX_REPLY_BYTES as u32 + 1).to_ne_bytes());
        assert!(read_reply(&mut &oversized[..], GET_TREE).is_err());
    }

    #[test]
    fn a_command_succeeds_only_when_every_outcome_succeeds() {
        assert!(command_outcome("focus", br#"[{"success":true}]"#).is_ok());
        let refused = command_outcome(
            "focus",
            br#"[{"success":true},{"success":false,"error":"No matching node"}]"#,
        )
        .unwrap_err();
        assert!(refused.to_string().contains("No matching node"));
        assert!(command_outcome("focus", b"[]").is_err());
        assert!(command_outcome("focus", b"not json").is_err());
    }

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

    #[test]
    fn outer_client_height_delta_fills_missing_decoration_metadata() {
        let tree = br#"{
          "id": 1,
          "nodes": [{
            "id": 10,
            "name": "Tauri",
            "app_id": "cua-test-harness",
            "pid": 123,
            "rect": {"x": 0, "y": 0, "width": 940, "height": 780},
            "window_rect": {"x": 0, "y": 0, "width": 940, "height": 733},
            "deco_rect": {"x": 0, "y": 0, "width": 0, "height": 0}
          }]
        }"#;
        let windows = parse_tree(tree).expect("parse Sway tree with implicit decoration");
        assert_eq!((windows[0].content_x, windows[0].content_y), (0, 47));
    }
}
