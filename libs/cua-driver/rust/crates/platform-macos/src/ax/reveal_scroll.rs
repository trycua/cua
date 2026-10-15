//! Background scroll through accessibility, for surfaces that drop
//! background wheel events and keystrokes (Electron/Chromium on macOS).
//!
//! There is no AX "scroll by N pixels" for web content, but every element
//! supports `AXScrollToVisible`, which scrolls its container just enough to
//! show it. Scrolling down by a distance is therefore: find the scroll
//! container under the point, find the element whose bottom edge lies about
//! that distance below the visible area, and reveal it. Up, left and right
//! mirror this. No event is posted and nothing is fronted.

use core_foundation::base::{CFRelease, CFRetain, CFTypeRef};

use super::bindings::{
    copy_children, copy_element_attr, copy_string_attr, element_at_screen_position,
    element_screen_rect, perform_action, AXUIElementRef,
};

/// Roles that scroll their content.
const CONTAINER_ROLES: &[&str] = &["AXScrollArea", "AXWebArea"];
/// Ancestors searched from the hit element for a scroll container.
const MAX_ASCENT: usize = 40;
/// Descendants visited when looking for the element to reveal.
const MAX_NODES: usize = 4_000;
const MAX_DEPTH: usize = 80;

/// What a reveal scroll did.
#[derive(Debug, Clone, PartialEq)]
pub struct RevealOutcome {
    /// Role of the scroll container used.
    pub container_role: String,
    /// How far the content moved, in points, in the requested direction.
    pub moved: f64,
}

/// `[x, y, w, h]` in screen points.
type Rect = [f64; 4];

fn intersect(a: Rect, b: Rect) -> Option<Rect> {
    let left = a[0].max(b[0]);
    let top = a[1].max(b[1]);
    let right = (a[0] + a[2]).min(b[0] + b[2]);
    let bottom = (a[1] + a[3]).min(b[1] + b[3]);
    (right > left && bottom > top).then_some([left, top, right - left, bottom - top])
}

/// Which candidate to reveal so the content moves about `distance` points in
/// `direction`: for "down", the element whose bottom edge is closest to
/// `distance` below the viewport's bottom (and lies below it); "up", "left"
/// and "right" mirror this. Elements larger than the viewport along the
/// scroll axis are containers and are skipped. `None` when nothing lies
/// beyond that edge.
pub fn pick_reveal_target(
    viewport: Rect,
    direction: &str,
    distance: f64,
    candidates: &[Rect],
) -> Option<usize> {
    let [vx, vy, vw, vh] = viewport;
    let mut best: Option<(usize, f64)> = None;
    for (index, rect) in candidates.iter().enumerate() {
        let [x, y, w, h] = *rect;
        let (edge, goal, beyond, fits) = match direction {
            "down" => (y + h, vy + vh + distance, y + h > vy + vh + 1.0, h <= vh),
            "up" => (y, vy - distance, y < vy - 1.0, h <= vh),
            "right" => (x + w, vx + vw + distance, x + w > vx + vw + 1.0, w <= vw),
            "left" => (x, vx - distance, x < vx - 1.0, w <= vw),
            _ => return None,
        };
        if !beyond || !fits {
            continue;
        }
        let gap = (edge - goal).abs();
        if best.is_none_or(|(_, best_gap)| gap < best_gap) {
            best = Some((index, gap));
        }
    }
    best.map(|(index, _)| index)
}

/// A clamped row chosen for a reveal.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClampedPick {
    /// Index into the nodes.
    pub index: usize,
    /// How many rows past the edge it is (1 = the first).
    pub rows: usize,
    /// Row pitch measured on the visible leaves.
    pub pitch: f64,
}

/// Chromium reports an element scrolled out of view as a 1 pt sliver
/// clamped to the viewport edge, not at its real position, and its
/// AXScrollToVisible centers the element. So when nothing lies geometrically
/// beyond the edge, the off-screen content is the leaves clamped to that
/// edge, in document order, and revealing the k-th of them moves the content
/// by about half the viewport plus (k - 0.5) rows. Pick k for `distance`,
/// counting rows at the pitch of the visible leaves. `nodes` are
/// (frame, is_leaf) in document order. `None` when nothing is clamped there.
pub fn pick_clamped(
    viewport: Rect,
    direction: &str,
    distance: f64,
    nodes: &[(Rect, bool)],
) -> Option<ClampedPick> {
    let [vx, vy, vw, vh] = viewport;
    let vertical = matches!(direction, "up" | "down");
    let sliver = |r: &Rect| if vertical { r[3] <= 1.5 } else { r[2] <= 1.5 };
    let at_edge = |r: &Rect| match direction {
        "down" => r[1] >= vy + vh - 2.5,
        "up" => r[1] <= vy + 1.5,
        "right" => r[0] >= vx + vw - 2.5,
        "left" => r[0] <= vx + 1.5,
        _ => false,
    };
    let mut clamped: Vec<usize> = nodes
        .iter()
        .enumerate()
        .filter(|(_, (rect, leaf))| *leaf && sliver(rect) && at_edge(rect))
        .map(|(index, _)| index)
        .collect();
    if clamped.is_empty() {
        return None;
    }
    if matches!(direction, "up" | "left") {
        clamped.reverse();
    }
    // Row pitch: the median gap between the visible leaves' starts.
    let mut starts: Vec<f64> = nodes
        .iter()
        .filter(|(rect, leaf)| {
            *leaf
                && !sliver(rect)
                && rect[1] >= vy
                && rect[1] + rect[3] <= vy + vh
                && rect[0] >= vx
                && rect[0] + rect[2] <= vx + vw
        })
        .map(|(rect, _)| if vertical { rect[1] } else { rect[0] })
        .collect();
    starts.sort_by(f64::total_cmp);
    starts.dedup_by(|a, b| (*a - *b).abs() < 2.0);
    let mut gaps: Vec<f64> = starts.windows(2).map(|pair| pair[1] - pair[0]).collect();
    gaps.sort_by(f64::total_cmp);
    let pitch = gaps.get(gaps.len() / 2).copied().unwrap_or(40.0).max(8.0);
    let half = if vertical { vh } else { vw } / 2.0;
    let rows = ((distance - half) / pitch + 0.5).round().max(1.0) as usize;
    let rows = rows.min(clamped.len());
    Some(ClampedPick {
        index: clamped[rows - 1],
        rows,
        pitch,
    })
}

/// Scroll the container under screen point `point` by about `distance`
/// points in `direction`, by revealing the element just past its edge.
/// `window` clips the container's visible area. `None` when there is no
/// scroll container under the point or the reveal moved nothing; the caller
/// then reports that background scroll is unavailable.
///
/// Chromium exposes an `overflow: auto` element as a plain group, not an
/// AXScrollArea, so the nearest scroll-role ancestor of a point inside an
/// inner scroller is the page's AXWebArea. The groups between the hit
/// element and that ancestor that clip content past their edge are tried
/// first, innermost out; the scroll-role container is the last resort. The
/// first one that moves wins.
///
/// # Safety
///
/// Performs AX calls on `pid`'s elements; call from a blocking context.
pub unsafe fn reveal_scroll(
    pid: i32,
    point: (f64, f64),
    window: Option<Rect>,
    direction: &str,
    distance: f64,
) -> Option<RevealOutcome> {
    let hit = element_at_screen_position(pid, point.0, point.1)?;
    let candidates = scroll_candidates(hit, direction);
    CFRelease(hit as CFTypeRef);
    let mut candidates = candidates.into_iter();
    while let Some((container, role)) = candidates.next() {
        let outcome = reveal_in(container, &role, window, direction, distance);
        CFRelease(container as CFTypeRef);
        match outcome {
            Some(outcome) => {
                for (rest, _) in candidates {
                    CFRelease(rest as CFTypeRef);
                }
                return Some(outcome);
            }
            None => {}
        }
    }
    None
}

/// The elements at or above `element` to try, innermost first and retained:
/// each one that clips content past its edge in `direction`, then the
/// nearest scroll-role container. Empty without a scroll-role container.
unsafe fn scroll_candidates(
    element: AXUIElementRef,
    direction: &str,
) -> Vec<(AXUIElementRef, String)> {
    let mut candidates = Vec::new();
    CFRetain(element as CFTypeRef);
    let mut current = element;
    for _ in 0..MAX_ASCENT {
        let role = copy_string_attr(current, "AXRole");
        match role.as_deref() {
            Some(role) if CONTAINER_ROLES.contains(&role) => {
                candidates.push((current, role.to_owned()));
                return candidates;
            }
            Some("AXWindow") | Some("AXApplication") | None => break,
            Some(role) => {
                if clips_past_edge(current, direction) {
                    CFRetain(current as CFTypeRef);
                    candidates.push((current, role.to_owned()));
                }
            }
        }
        let Some(parent) = copy_element_attr(current, "AXParent") else {
            CFRelease(current as CFTypeRef);
            current = std::ptr::null_mut();
            break;
        };
        CFRelease(current as CFTypeRef);
        current = parent;
    }
    if !current.is_null() {
        CFRelease(current as CFTypeRef);
    }
    for (candidate, _) in candidates {
        CFRelease(candidate as CFTypeRef);
    }
    Vec::new()
}

/// Whether `element` holds content past its edge in `direction`: a direct
/// child lies beyond it, or is a sliver clamped to it.
unsafe fn clips_past_edge(element: AXUIElementRef, direction: &str) -> bool {
    let Some(frame) = element_screen_rect(element).filter(|frame| real_size(*frame)) else {
        return false;
    };
    let mut clips = false;
    for child in copy_children(element) {
        if !clips {
            clips =
                element_screen_rect(child).is_some_and(|rect| past_edge(frame, direction, rect));
        }
        CFRelease(child as CFTypeRef);
    }
    clips
}

/// Whether `rect`, a child of the element with `frame`, lies past that
/// frame's edge in `direction` or is a sliver clamped to that edge (how
/// Chromium reports content scrolled out of an overflow container).
pub fn past_edge(frame: Rect, direction: &str, rect: Rect) -> bool {
    let [fx, fy, fw, fh] = frame;
    let [x, y, w, h] = rect;
    match direction {
        "down" => y + h > fy + fh + 1.0 || (h <= 1.5 && y >= fy + fh - 2.5),
        "up" => y < fy - 1.0 || (h <= 1.5 && y <= fy + 1.5),
        "right" => x + w > fx + fw + 1.0 || (w <= 1.5 && x >= fx + fw - 2.5),
        "left" => x < fx - 1.0 || (w <= 1.5 && x <= fx + 1.5),
        _ => false,
    }
}

unsafe fn reveal_in(
    container: AXUIElementRef,
    role: &str,
    window: Option<Rect>,
    direction: &str,
    distance: f64,
) -> Option<RevealOutcome> {
    let frame = element_screen_rect(container)?;
    let viewport = match window {
        Some(window) => intersect(frame, window)?,
        None => frame,
    };
    // Visit descendants in document order, keeping each element with a
    // frame. Subtrees that lie wholly on the near side of the viewport
    // cannot hold the target.
    let mut elements: Vec<AXUIElementRef> = Vec::new();
    let mut nodes: Vec<(Rect, bool)> = Vec::new();
    let mut stack: Vec<(AXUIElementRef, usize)> = copy_children(container)
        .into_iter()
        .rev()
        .map(|child| (child, 1))
        .collect();
    let mut visited = 0usize;
    while let Some((element, depth)) = stack.pop() {
        visited += 1;
        let rect = element_screen_rect(element);
        let behind = rect.is_some_and(|[x, y, w, h]| match direction {
            "down" => y + h < viewport[1],
            "up" => y > viewport[1] + viewport[3],
            "right" => x + w < viewport[0],
            "left" => x > viewport[0] + viewport[2],
            _ => false,
        });
        let mut leaf = true;
        if !behind && depth < MAX_DEPTH && visited < MAX_NODES {
            let children = copy_children(element);
            leaf = children.is_empty();
            for child in children.into_iter().rev() {
                stack.push((child, depth + 1));
            }
        }
        match rect {
            Some(rect) if !behind => {
                elements.push(element);
                nodes.push((rect, leaf));
            }
            _ => CFRelease(element as CFTypeRef),
        }
        if visited >= MAX_NODES {
            break;
        }
    }
    for (element, _) in stack {
        CFRelease(element as CFTypeRef);
    }

    // Real frames past the edge first; Chromium's clamped slivers otherwise.
    let real: Vec<Rect> = nodes
        .iter()
        .map(|(rect, _)| *rect)
        .map(|rect| {
            if rect[2] <= 1.5 || rect[3] <= 1.5 {
                [0.0, 0.0, 0.0, 0.0]
            } else {
                rect
            }
        })
        .collect();
    let outcome = if let Some(index) = pick_reveal_target(viewport, direction, distance, &real) {
        // A real frame: measure the move on the element itself.
        let before = nodes[index].0;
        perform_action(elements[index], "AXScrollToVisible");
        std::thread::sleep(std::time::Duration::from_millis(150));
        let after = element_screen_rect(elements[index]).unwrap_or(before);
        let moved = match direction {
            "down" => before[1] - after[1],
            "up" => after[1] - before[1],
            "right" => before[0] - after[0],
            _ => after[0] - before[0],
        };
        (moved > 0.5).then(|| RevealOutcome {
            container_role: role.to_owned(),
            moved,
        })
    } else if let Some(pick) = pick_clamped(viewport, direction, distance, &nodes) {
        // A clamped sliver: it scrolled when it now shows at a real size
        // inside the viewport or a visible reference moved (or left). Its old
        // position is estimated from the row count, so the distance is
        // approximate.
        let reference = nodes
            .iter()
            .position(|(rect, leaf)| *leaf && real_size(*rect) && contains(viewport, *rect));
        let reference_before = reference.map(|at| nodes[at].0);
        perform_action(elements[pick.index], "AXScrollToVisible");
        std::thread::sleep(std::time::Duration::from_millis(150));
        let after = element_screen_rect(elements[pick.index]);
        let reference_after = reference.and_then(|at| element_screen_rect(elements[at]));
        let shown =
            after.is_some_and(|rect| real_size(rect) && intersect(rect, viewport).is_some());
        if content_moved(reference_before, reference_after) != Some(true) && !shown {
            return finish(elements, None);
        }
        after.filter(|rect| real_size(*rect)).map(|after| {
            let past = (pick.rows as f64 - 1.0) * pick.pitch;
            let [vx, vy, vw, vh] = viewport;
            let moved = match direction {
                "down" => vy + vh + past - after[1],
                "up" => after[1] + after[3] - (vy - past),
                "right" => vx + vw + past - after[0],
                _ => after[0] + after[2] - (vx - past),
            };
            RevealOutcome {
                container_role: role.to_owned(),
                moved: moved.max(1.0),
            }
        })
    } else {
        None
    };
    finish(elements, outcome)
}

/// Release the visited elements and return `outcome`.
unsafe fn finish(
    elements: Vec<AXUIElementRef>,
    outcome: Option<RevealOutcome>,
) -> Option<RevealOutcome> {
    for element in elements {
        CFRelease(element as CFTypeRef);
    }
    outcome
}

fn real_size(rect: Rect) -> bool {
    rect[2] > 1.5 && rect[3] > 1.5
}

fn contains(outer: Rect, inner: Rect) -> bool {
    inner[0] >= outer[0]
        && inner[1] >= outer[1]
        && inner[0] + inner[2] <= outer[0] + outer[2]
        && inner[1] + inner[3] <= outer[1] + outer[3]
}

/// Whether a reveal moved the content, judged on an element that was fully
/// visible before: it moved, or it left (now a clamped sliver or gone).
/// `None` without such an element.
pub fn content_moved(before: Option<Rect>, after: Option<Rect>) -> Option<bool> {
    let before = before?;
    Some(match after {
        None => true,
        Some(after) if !real_size(after) => true,
        Some(after) => (after[0] - before[0]).abs() > 0.5 || (after[1] - before[1]).abs() > 0.5,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIEWPORT: Rect = [0.0, 100.0, 800.0, 500.0];

    fn row(y: f64) -> Rect {
        [10.0, y, 600.0, 40.0]
    }

    #[test]
    fn down_reveals_the_row_closest_to_the_distance_below_the_edge() {
        // Rows every 50 pt from y=100; the viewport ends at y=600.
        let rows: Vec<Rect> = (0..30).map(|i| row(100.0 + 50.0 * i as f64)).collect();
        let picked = pick_reveal_target(VIEWPORT, "down", 360.0, &rows).unwrap();
        // Goal: a bottom edge at 960. Row 17 spans 950-990, row 16 900-940.
        assert_eq!(rows[picked], row(900.0));
    }

    #[test]
    fn up_left_and_right_mirror_down() {
        let rows: Vec<Rect> = (-10..10).map(|i| row(100.0 + 50.0 * i as f64)).collect();
        let picked = pick_reveal_target(VIEWPORT, "up", 120.0, &rows).unwrap();
        assert_eq!(
            rows[picked],
            row(0.0),
            "top edge closest to 100 - 120 = -20"
        );

        let columns: Vec<Rect> = (0..20)
            .map(|i| [100.0 * i as f64, 200.0, 90.0, 40.0])
            .collect();
        let right = pick_reveal_target(VIEWPORT, "right", 120.0, &columns).unwrap();
        assert_eq!(columns[right][0], 800.0);
        assert_eq!(pick_reveal_target(VIEWPORT, "left", 120.0, &columns), None);
    }

    #[test]
    fn without_candidates_beyond_the_viewport_nothing_is_picked() {
        let visible = vec![row(120.0), row(300.0), row(550.0)];
        assert_eq!(pick_reveal_target(VIEWPORT, "down", 120.0, &visible), None);
        // A page-sized wrapper reaching past the edge is not a target.
        let wrapper = vec![[0.0, 100.0, 800.0, 3000.0]];
        assert_eq!(pick_reveal_target(VIEWPORT, "down", 120.0, &wrapper), None);
        assert_eq!(
            pick_reveal_target(VIEWPORT, "sideways", 120.0, &[row(900.0)]),
            None
        );
    }

    #[test]
    fn clamped_slivers_count_as_the_content_past_the_edge() {
        // What Chromium reported live: rows past the viewport bottom (y=890)
        // as 1 pt slivers at y=889; visible rows every 45 pt.
        let mut nodes: Vec<(Rect, bool)> = (0..13)
            .map(|i| ([518.0, 301.0 + 45.0 * i as f64, 600.0, 41.0], true))
            .collect();
        nodes.push(([518.0, 300.0, 700.0, 590.0], false)); // the list itself
        let first_clamped = nodes.len();
        for _ in 0..100 {
            nodes.push(([518.0, 889.0, 600.0, 1.0], true));
        }
        let viewport = [500.0, 290.0, 800.0, 600.0];
        // Revealing centers the row: 360 pt is half the 600 pt viewport plus
        // about 1.5 rows of 45 pt, so the 2nd row past the edge.
        let pick = pick_clamped(viewport, "down", 360.0, &nodes).unwrap();
        assert_eq!(
            (pick.index, pick.rows, pick.pitch),
            (first_clamped + 1, 2, 45.0)
        );
        // A page down (600 pt) is about 7 rows on.
        assert_eq!(
            pick_clamped(viewport, "down", 600.0, &nodes).unwrap().rows,
            7
        );
        // A distance past the content picks the last row.
        let far = pick_clamped(viewport, "down", 100_000.0, &nodes).unwrap();
        assert_eq!(far.index, first_clamped + 99);
        // Nothing clamped at the top edge: at the start already.
        assert_eq!(pick_clamped(viewport, "up", 360.0, &nodes), None);
        // Up counts from the nearest sliver above, the last in document order;
        // a short distance still reveals at least the nearest row.
        let mut above: Vec<(Rect, bool)> =
            (0..5).map(|_| ([518.0, 290.0, 600.0, 1.0], true)).collect();
        above.extend((0..13).map(|i| ([518.0, 301.0 + 45.0 * i as f64, 600.0, 41.0], true)));
        assert_eq!(pick_clamped(viewport, "up", 90.0, &above).unwrap().index, 4);
        assert_eq!(
            pick_clamped(viewport, "up", 480.0, &above).unwrap().index,
            0
        );
    }

    #[test]
    fn a_reveal_that_moves_nothing_visible_is_not_confirmed_movement() {
        let row = [518.0, 301.0, 600.0, 41.0];
        // Seen live at the top of the page: the reveal changed nothing.
        assert_eq!(content_moved(Some(row), Some(row)), Some(false));
        assert_eq!(
            content_moved(Some(row), Some([518.0, 260.0, 600.0, 41.0])),
            Some(true)
        );
        // Scrolled out of view: clamped to a sliver, or gone.
        assert_eq!(
            content_moved(Some(row), Some([518.0, 222.0, 600.0, 1.0])),
            Some(true)
        );
        assert_eq!(content_moved(Some(row), None), Some(true));
        assert_eq!(content_moved(None, Some(row)), None);
    }

    #[test]
    fn intersect_clips_to_the_window() {
        assert_eq!(
            intersect([0.0, 0.0, 1000.0, 3000.0], [10.0, 50.0, 800.0, 600.0]),
            Some([10.0, 50.0, 800.0, 600.0])
        );
        assert_eq!(
            intersect([0.0, 0.0, 10.0, 10.0], [20.0, 20.0, 5.0, 5.0]),
            None
        );
    }
    #[test]
    fn a_child_past_or_clamped_to_the_edge_marks_an_inner_scroller() {
        let frame = [0.0, 100.0, 300.0, 128.0];
        // A row below the visible area, and Chromium's 1 pt sliver clamped
        // to the bottom edge for a row scrolled out of view.
        assert!(past_edge(frame, "down", [0.0, 240.0, 300.0, 16.0]));
        assert!(past_edge(frame, "down", [0.0, 227.0, 300.0, 1.0]));
        // A visible row inside the frame is not.
        assert!(!past_edge(frame, "down", [0.0, 110.0, 300.0, 16.0]));
        // At the top nothing is above; a row above marks it for "up".
        assert!(!past_edge(frame, "up", [0.0, 110.0, 300.0, 16.0]));
        assert!(past_edge(frame, "up", [0.0, 60.0, 300.0, 16.0]));
        assert!(past_edge(frame, "right", [250.0, 110.0, 100.0, 16.0]));
        assert!(!past_edge(frame, "left", [0.0, 110.0, 300.0, 16.0]));
    }
}
