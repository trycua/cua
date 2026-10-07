//! AT-SPI hit-test for presence cursor shapes: the chain of accessibles from
//! a top-level frame down to the deepest one under a point, with role and
//! the states the shape table needs. Read-only: no action, focus or pointer
//! change.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::Result;
use atspi::proxy::accessible::AccessibleProxy;
use atspi::proxy::proxy_ext::ProxyExt;
use atspi::{CoordType, State};

use super::{accessible_for, app_for_pid, call, runtime, shared_connection, RawObjectRef};
use crate::pointer_shape_map::RoleSample;

/// Budget for resolving a pid's application accessible (cached afterwards).
const APP_BUDGET: Duration = Duration::from_millis(400);
/// How long an app's cached frame list is trusted (new windows appear
/// within this).
const APP_TTL: Duration = Duration::from_secs(3);
/// Deepest descent; real trees are far shallower.
const MAX_DEPTH: usize = 40;

/// Which coordinate space the point is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitCoords {
    /// Global screen pixels (X11 toolkits report these correctly).
    Screen,
    /// Relative to the top-level window (Wayland, where screen extents are
    /// unknown to clients).
    Window,
}

/// One node of a hit chain.
#[derive(Debug, Clone)]
pub struct HitNode {
    pub sample: RoleSample,
    /// Extents in the requested coordinate space, when the node has them.
    pub extents: Option<(i32, i32, i32, i32)>,
}

/// pid -> the app's top-level frame references and when they were read.
type FrameCache = Mutex<HashMap<u32, (Vec<RawObjectRef>, Instant)>>;

fn app_cache() -> &'static FrameCache {
    static CACHE: std::sync::OnceLock<FrameCache> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn raw_of(acc: &AccessibleProxy<'_>) -> RawObjectRef {
    let inner = acc.inner();
    RawObjectRef {
        name: inner.destination().to_string(),
        path: inner.path().to_string(),
    }
}

async fn sample(acc: &AccessibleProxy<'_>, coords: CoordType) -> Option<HitNode> {
    let role = call(acc.get_role_name()).await?.ok()?;
    let states = call(acc.get_state()).await.and_then(|r| r.ok());
    let editable = states.is_some_and(|s| s.contains(State::Editable));
    let busy = states.is_some_and(|s| s.contains(State::Busy));
    let extents = match call(acc.proxies()).await {
        Some(Ok(p)) => match call(p.component()).await {
            Some(Ok(c)) => call(c.get_extents(coords)).await.and_then(|r| r.ok()),
            _ => None,
        },
        _ => None,
    };
    Some(HitNode {
        sample: RoleSample {
            role,
            editable,
            busy,
        },
        extents,
    })
}

fn inside(extents: Option<(i32, i32, i32, i32)>, x: i32, y: i32) -> bool {
    extents.is_some_and(|(ex, ey, w, h)| {
        w > 0 && h > 0 && x >= ex && y >= ey && x < ex + w && y < ey + h
    })
}

/// Bounded scan of `parent`'s children for the one under `(x, y)`: the
/// smallest showing child containing the point, else (in a page tab list)
/// the selected page, whose body the tab's own extents do not cover.
async fn child_under(
    conn: &atspi::connection::AccessibilityConnection,
    parent: &AccessibleProxy<'_>,
    x: i32,
    y: i32,
    coords: CoordType,
    parent_role: &str,
) -> Option<RawObjectRef> {
    const MAX_CHILDREN: usize = 64;
    let children = call(parent.get_children()).await?.ok()?;
    let mut best: Option<(i64, RawObjectRef)> = None;
    let mut selected = None;
    for child in children.iter().take(MAX_CHILDREN) {
        let Some(raw) = RawObjectRef::from_atspi(child) else {
            continue;
        };
        let Ok(acc) = accessible_for(conn, &raw).await else {
            continue;
        };
        let Some(states) = call(acc.get_state()).await.and_then(|r| r.ok()) else {
            continue;
        };
        if !states.contains(State::Showing) {
            continue;
        }
        if parent_role == "page tab list" && states.contains(State::Selected) {
            selected = Some(raw.clone());
        }
        let Some(node) = sample(&acc, coords).await else {
            continue;
        };
        if inside(node.extents, x, y) {
            let area = node
                .extents
                .map(|(_, _, w, h)| i64::from(w) * i64::from(h))
                .unwrap_or(i64::MAX);
            if best.as_ref().is_none_or(|(a, _)| area < *a) {
                best = Some((area, raw));
            }
        }
    }
    best.map(|(_, raw)| raw).or(selected)
}

/// The accessible chain under `(x, y)` in `pid`'s application, frame first.
///
/// `title` narrows the frame when several of the app's frames could contain
/// the point (always the case in window coordinates). `Ok(None)` when the
/// app has no accessible, no frame contains the point, or `budget` ran out.
pub fn chain_at_point(
    pid: u32,
    x: i32,
    y: i32,
    coords: HitCoords,
    title: Option<&str>,
    budget: Duration,
) -> Result<Option<Vec<HitNode>>> {
    let coord_type = match coords {
        HitCoords::Screen => CoordType::Screen,
        HitCoords::Window => CoordType::Window,
    };
    runtime().block_on(async move {
        let conn = shared_connection().await?;
        // Cache the app's top-level frame references (their bus names are
        // real peer names; the application proxy's own destination may be a
        // peer-to-peer placeholder that cannot be re-addressed).
        let cached = app_cache()
            .lock()
            .unwrap()
            .get(&pid)
            .filter(|(_, at)| at.elapsed() < APP_TTL)
            .map(|(frames, _)| frames.clone());
        let frames = match cached {
            Some(frames) => frames,
            None => {
                let app = match tokio::time::timeout(APP_BUDGET, app_for_pid(conn, pid)).await {
                    Ok(Ok(Some(app))) => app,
                    Ok(Err(e)) => return Err(e),
                    _ => return Ok(None),
                };
                let Some(Ok(children)) = call(app.get_children()).await else {
                    return Ok(None);
                };
                let frames: Vec<RawObjectRef> = children
                    .iter()
                    .filter_map(RawObjectRef::from_atspi)
                    .take(32)
                    .collect();
                app_cache()
                    .lock()
                    .unwrap()
                    .insert(pid, (frames.clone(), Instant::now()));
                frames
            }
        };
        let walk = async {
            // Pick the frame: showing, containing the point, the title match
            // or the active one first.
            let mut best: Option<(u8, AccessibleProxy<'_>, HitNode)> = None;
            for raw in &frames {
                let Ok(acc) = accessible_for(conn, raw).await else {
                    continue;
                };
                let Some(states) = call(acc.get_state()).await.and_then(|r| r.ok()) else {
                    continue;
                };
                if !states.contains(State::Showing) {
                    continue;
                }
                let Some(node) = sample(&acc, coord_type).await else {
                    continue;
                };
                if coords == HitCoords::Screen && !inside(node.extents, x, y) {
                    if std::env::var_os("CUA_ATSPI_DEBUG").is_some() {
                        eprintln!("[cua-atspi] frame {:?} misses ({x},{y})", node.extents);
                    }
                    continue;
                }
                let name = call(acc.name())
                    .await
                    .and_then(|r| r.ok())
                    .unwrap_or_default();
                if std::env::var_os("CUA_ATSPI_DEBUG").is_some() {
                    eprintln!(
                        "[cua-atspi] hit frame {name:?} {:?} at ({x},{y})",
                        node.extents
                    );
                }
                let score = if title.is_some_and(|t| !t.is_empty() && t == name) {
                    3
                } else if states.contains(State::Active) {
                    2
                } else {
                    1
                };
                if best.as_ref().is_none_or(|(s, _, _)| score > *s) {
                    best = Some((score, acc, node));
                }
            }
            let (_, mut current, frame) = best?;
            let mut chain = vec![frame];
            let mut seen = raw_of(&current).path;
            for _ in 0..MAX_DEPTH {
                let Some(Ok(proxies)) = call(current.proxies()).await else {
                    break;
                };
                let Some(Ok(component)) = call(proxies.component()).await else {
                    break;
                };
                let at_point = call(component.get_accessible_at_point(x, y, coord_type))
                    .await
                    .and_then(|r| r.ok())
                    .filter(|c| !c.is_null())
                    .and_then(|c| RawObjectRef::from_atspi(&c))
                    .filter(|raw| raw.path != seen);
                // Toolkits answer GetAccessibleAtPoint only for children whose
                // own extents they track: a GtkNotebook's page tabs report the
                // tab label, so the page body (a terminal, an editor) is never
                // reached. Fall back to scanning the children.
                let raw = match at_point {
                    Some(raw) => raw,
                    None => {
                        let role = chain.last().map(|n| n.sample.role.as_str()).unwrap_or("");
                        match child_under(conn, &current, x, y, coord_type, role).await {
                            Some(raw) if raw.path != seen => raw,
                            _ => break,
                        }
                    }
                };
                seen = raw.path.clone();
                let Ok(next) = accessible_for(conn, &raw).await else {
                    break;
                };
                let Some(node) = sample(&next, coord_type).await else {
                    break;
                };
                chain.push(node);
                current = next;
            }
            Some(chain)
        };
        Ok(tokio::time::timeout(budget, walk).await.ok().flatten())
    })
}
