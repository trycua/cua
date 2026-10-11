//! Point ownership for element clicks.
//!
//! A snapshot frame is where the toolkit *said* an element is; a real pointer
//! press lands on whatever the toolkit *draws* there. The two disagree when a
//! view that is not on screen still reports its rows with frames: Qt's
//! `QAccessibleTableCell::state()` never derives `invisible` from its view, so
//! the QListView of a QFileDialog in Detail mode (a QStackedWidget page that is
//! not current) lists every row over the visible QTreeView. Pressing the
//! list-row centre hit the tree's "Name" column header and flipped the sort
//! order instead of selecting the file (VLC "Convert / Save", round 7).
//!
//! Before a press, ask the toolkit's own `GetAccessibleAtPoint` who owns the
//! resolved point. The observed object (or an ancestor of the deepest hit)
//! owning it means the press is honest. Otherwise look for the same-named
//! selectable item of the table-like container the point really belongs to
//! (the visible tree row) and press that; failing that the caller falls back
//! to the element's AT-SPI action or refuses with the owner named.

use super::*;

/// The accessible that owns a screen point, as the toolkit answers it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PointOwner {
    pub object_ref: ObjectRef,
    pub role: String,
    pub name: String,
}

impl PointOwner {
    /// `tree item "Desktop"` / `column header` (unnamed).
    pub fn describe(&self) -> String {
        if self.name.is_empty() {
            format!("{} (unnamed)", self.role)
        } else {
            format!("{} \"{}\"", self.role, self.name)
        }
    }
}

/// What a hit-test of an element's press point found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PointOwnership {
    /// The element itself, or one of its descendants, owns the point.
    Owned,
    /// Another object owns the point, but a same-named selectable item of a
    /// table-like container on the owner's chain has a point of its own.
    Redirect {
        owner: PointOwner,
        target: PointOwner,
        /// Screen centre of `target`.
        screen_point: (i32, i32),
    },
    /// Another object owns the point and no equivalent item was found.
    Mismatch { owner: PointOwner },
    /// The toolkit could not be asked (Wayland, no application, no
    /// `GetAccessibleAtPoint` answer, timeout): the press proceeds unchecked.
    Unknown,
}

/// Rows of a table-like container the name lookup may redirect to.
fn is_row_container_role(role_lower: &str) -> bool {
    matches!(role_lower, "tree" | "tree table" | "table" | "list")
}

/// Children a redirect lookup reads from one container at most.
const REDIRECT_CHILDREN_CAP: usize = 512;

async fn describe_node(node: &HitNode<'_>) -> PointOwner {
    let role = call(node.acc.get_role_name())
        .await
        .and_then(Result::ok)
        .unwrap_or_default();
    let name = call(node.acc.name())
        .await
        .and_then(Result::ok)
        .unwrap_or_default();
    PointOwner {
        object_ref: ObjectRef {
            bus: node.oref.name.clone(),
            path: node.oref.path.clone(),
        },
        role,
        name,
    }
}

/// The same-named selectable item among the children of the table-like
/// containers on `chain` (deepest first), with its trusted screen centre.
async fn same_named_item_on_chain(
    conn: &AccessibilityConnection,
    chain: &[HitNode<'_>],
    name: &str,
    display: Option<(u32, u32)>,
) -> Option<(PointOwner, (i32, i32))> {
    for node in chain.iter().rev().skip(1) {
        let Some(Ok(role)) = call(node.acc.get_role_name()).await else {
            continue;
        };
        if !is_row_container_role(&role.trim().to_ascii_lowercase()) {
            continue;
        }
        let Some(Ok(children)) = call(raw_children(conn.connection(), &node.oref)).await else {
            continue;
        };
        for child in children.iter().take(REDIRECT_CHILDREN_CAP) {
            let Some(Ok(acc)) = call(accessible_for(conn, child)).await else {
                continue;
            };
            let Some(Ok(child_name)) = call(acc.name()).await else {
                continue;
            };
            if child_name != name {
                continue;
            }
            let Some(Ok(child_role)) = call(acc.get_role_name()).await else {
                continue;
            };
            if !is_selectable_item_role(&child_role) {
                continue;
            }
            let Some(Ok(proxies)) = call(acc.proxies()).await else {
                continue;
            };
            let Some(Ok(component)) = call(proxies.component()).await else {
                continue;
            };
            let Some(Ok(raw)) = call(component.get_extents(CoordType::Screen)).await else {
                continue;
            };
            if !screen_extents_trusted(raw, display) || raw.2 <= 0 || raw.3 <= 0 {
                continue;
            }
            let centre = (raw.0 + raw.2 / 2, raw.1 + raw.3 / 2);
            return Some((
                PointOwner {
                    object_ref: ObjectRef {
                        bus: child.name.clone(),
                        path: child.path.clone(),
                    },
                    role: child_role,
                    name: child_name,
                },
                centre,
            ));
        }
    }
    None
}

/// Who owns screen point `(sx, sy)` of window `xid` in `pid`'s application,
/// relative to the snapshot object `observed` a click is about to press.
/// Blocking; bounded by [`INPUT_QUERY_BUDGET`]. Never errors: an unanswered
/// question is [`PointOwnership::Unknown`].
pub fn point_ownership(
    pid: u32,
    xid: u64,
    observed: &ObjectRef,
    sx: i32,
    sy: i32,
) -> PointOwnership {
    if crate::wayland::is_wayland() {
        return PointOwnership::Unknown;
    }
    let Some(client_origin) = x11_window_origin(xid) else {
        return PointOwnership::Unknown;
    };
    let (win_x, win_y) = (sx - client_origin.0, sy - client_origin.1);
    let display = x11_display_size();
    let observed_raw = raw_ref(observed);
    let result: Result<PointOwnership> = bounded_for(
        INPUT_QUERY_BUDGET,
        async {
            let conn = shared_connection().await?;
            let Some(app) = app_for_pid(conn, pid).await? else {
                return Ok(PointOwnership::Unknown);
            };
            let seeds: Vec<RawObjectRef> = match call(app.get_children()).await {
                Some(Ok(children)) => children
                    .into_iter()
                    .filter_map(|child| RawObjectRef::from_atspi(&child))
                    .collect(),
                _ => return Ok(PointOwnership::Unknown),
            };
            let scoped = resolve_window_frame(conn, pid, xid, &seeds).await;
            let (hit_x, hit_y) = match scoped {
                Some(ordinal) => {
                    let frame_origin = decoration_frame_origin(conn, &app, &seeds[ordinal]).await;
                    at_point_toolkit_coords((win_x, win_y), Some(client_origin), frame_origin)
                }
                None => (win_x, win_y),
            };
            let ordered: Vec<RawObjectRef> = match scoped {
                Some(ordinal) => seeds.get(ordinal).cloned().into_iter().collect(),
                None => seeds,
            };
            for seed in ordered {
                let chain = descend_at_point(conn, seed, hit_x, hit_y).await;
                if chain.len() <= 1 {
                    continue;
                }
                if chain.iter().any(|node| node.oref == observed_raw) {
                    return Ok(PointOwnership::Owned);
                }
                let deepest = chain.last().expect("chain has more than one node");
                // The toolkit stopped at one of the target's own containers
                // (a web document answers with its `panel` / `filler`): it
                // cannot resolve the point any deeper, which is not evidence
                // that a different control owns it.
                if is_ancestor_of(conn, &deepest.oref, &observed_raw).await {
                    return Ok(PointOwnership::Unknown);
                }
                let owner = describe_node(deepest).await;
                let observed_name = match call(accessible_for(conn, &observed_raw)).await {
                    Some(Ok(acc)) => call(acc.name())
                        .await
                        .and_then(Result::ok)
                        .unwrap_or_default(),
                    _ => String::new(),
                };
                if !observed_name.is_empty() {
                    if let Some((target, screen_point)) =
                        same_named_item_on_chain(conn, &chain, &observed_name, display).await
                    {
                        if target.object_ref != observed.clone() {
                            return Ok(PointOwnership::Redirect {
                                owner,
                                target,
                                screen_point,
                            });
                        }
                    }
                }
                return Ok(PointOwnership::Mismatch { owner });
            }
            Ok(PointOwnership::Unknown)
        },
        || Ok(PointOwnership::Unknown),
    );
    result.unwrap_or(PointOwnership::Unknown)
}

/// Whether `ancestor` is on `node`'s parent chain (bounded).
async fn is_ancestor_of(
    conn: &AccessibilityConnection,
    ancestor: &RawObjectRef,
    node: &RawObjectRef,
) -> bool {
    let same = |a: &RawObjectRef, b: &RawObjectRef| a.name == b.name && a.path == b.path;
    let mut current = node.clone();
    for _ in 0..32 {
        let Some(Ok(acc)) = call(accessible_for(conn, &current)).await else {
            return false;
        };
        let Some(Ok(parent)) = call(acc.parent()).await else {
            return false;
        };
        let Some(parent) = RawObjectRef::from_atspi(&parent) else {
            return false;
        };
        if same(&parent, ancestor) {
            return true;
        }
        if same(&parent, &current) {
            return false;
        }
        current = parent;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_description_names_role_and_name() {
        let owner = PointOwner {
            object_ref: ObjectRef {
                bus: ":1.9".into(),
                path: "/a".into(),
            },
            role: "column header".into(),
            name: String::new(),
        };
        assert_eq!(owner.describe(), "column header (unnamed)");
        let named = PointOwner {
            name: "Desktop".into(),
            role: "tree item".into(),
            ..owner
        };
        assert_eq!(named.describe(), "tree item \"Desktop\"");
    }

    #[test]
    fn row_containers_are_the_table_like_roles() {
        assert!(is_row_container_role("tree"));
        assert!(is_row_container_role("tree table"));
        assert!(is_row_container_role("list"));
        assert!(!is_row_container_role("panel"));
        assert!(!is_row_container_role("menu"));
    }

    #[test]
    fn no_display_is_unknown_not_an_error() {
        let observed = ObjectRef {
            bus: ":1.9".into(),
            path: "/org/a11y/atspi/accessible/1".into(),
        };
        let display = crate::test_env::unreachable_x11_display();
        let ownership = point_ownership(1, 0x1234, &observed, 10, 10);
        drop(display);
        assert_eq!(ownership, PointOwnership::Unknown);
    }
}
