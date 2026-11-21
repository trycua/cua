// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Generic staging `ext-foreign-toplevel-list-v1` window enumeration.
//!
//! Each window_id stands for one toplevel `identifier`, which the protocol
//! keeps unique and stable across clients. [`identifier_for`] maps it back,
//! so [`super::ext_screencopy::screenshot_toplevel`] can copy that exact
//! toplevel on a connection of its own.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use wayland_client::{
    event_created_child, protocol::wl_registry, Connection, Dispatch, Proxy, QueueHandle,
};
use wayland_protocols::ext::foreign_toplevel_list::v1::client::{
    ext_foreign_toplevel_handle_v1::{self as ext_handle, ExtForeignToplevelHandleV1},
    ext_foreign_toplevel_list_v1::{
        self as ext_list, ExtForeignToplevelListV1, EVT_TOPLEVEL_OPCODE,
    },
};

use crate::x11::WindowInfo;

/// Synthetic ext-toplevel IDs must survive the existing `window_id as u32`
/// element-token paths. Keep them in a high, nonzero u32 namespace and reserve
/// `u32::MAX` as an exhaustion sentinel rather than emitting it.
const EXT_ID_NAMESPACE_START: u32 = 0xF000_0000;
const EXT_ID_NAMESPACE_END: u32 = 0xFFFF_FFFE;

/// Globals a per-toplevel copy needs besides the list itself.
const TOPLEVEL_CAPTURE_GLOBALS: [&str; 2] = [
    "ext_foreign_toplevel_image_capture_source_manager_v1",
    "ext_image_copy_capture_manager_v1",
];

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ToplevelRecord {
    identifier: String,
    title: String,
    app_id: String,
}

#[derive(Default)]
struct PendingRecord {
    identifier: Option<String>,
    title: Option<String>,
    app_id: Option<String>,
}

#[derive(Default)]
struct State {
    manager: Option<ExtForeignToplevelListV1>,
    capture_globals: [bool; TOPLEVEL_CAPTURE_GLOBALS.len()],
    pending: HashMap<u32, PendingRecord>,
    toplevels: HashMap<u32, ToplevelRecord>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            if interface == ExtForeignToplevelListV1::interface().name {
                state.manager = Some(registry.bind::<ExtForeignToplevelListV1, _, _>(
                    name,
                    version.min(1),
                    qh,
                    (),
                ));
            } else if let Some(index) = TOPLEVEL_CAPTURE_GLOBALS
                .iter()
                .position(|global| *global == interface)
            {
                state.capture_globals[index] = true;
            }
        }
    }
}

impl Dispatch<ExtForeignToplevelListV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ExtForeignToplevelListV1,
        _: ext_list::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }

    event_created_child!(State, ExtForeignToplevelListV1, [
        EVT_TOPLEVEL_OPCODE => (ExtForeignToplevelHandleV1, ()),
    ]);
}

impl Dispatch<ExtForeignToplevelHandleV1, ()> for State {
    fn event(
        state: &mut Self,
        handle: &ExtForeignToplevelHandleV1,
        event: ext_handle::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let protocol_id = handle.id().protocol_id();
        match event {
            ext_handle::Event::Title { title } => {
                state.pending.entry(protocol_id).or_default().title = Some(title);
            }
            ext_handle::Event::AppId { app_id } => {
                state.pending.entry(protocol_id).or_default().app_id = Some(app_id);
            }
            ext_handle::Event::Identifier { identifier } => {
                state.pending.entry(protocol_id).or_default().identifier = Some(identifier);
            }
            ext_handle::Event::Done => {
                let pending = state.pending.remove(&protocol_id).unwrap_or_default();
                let record = state.toplevels.entry(protocol_id).or_default();
                if let Some(identifier) = pending.identifier {
                    record.identifier = identifier;
                }
                if let Some(title) = pending.title {
                    record.title = title;
                }
                if let Some(app_id) = pending.app_id {
                    record.app_id = app_id;
                }
            }
            ext_handle::Event::Closed => {
                state.pending.remove(&protocol_id);
                state.toplevels.remove(&protocol_id);
                handle.destroy();
            }
            _ => {}
        }
    }
}

struct IdRegistry {
    by_identifier: HashMap<String, u32>,
    by_id: HashMap<u32, String>,
    next: Option<u32>,
}

impl Default for IdRegistry {
    fn default() -> Self {
        Self {
            by_identifier: HashMap::new(),
            by_id: HashMap::new(),
            next: Some(EXT_ID_NAMESPACE_START),
        }
    }
}

impl IdRegistry {
    fn id_for(&mut self, identifier: &str) -> anyhow::Result<u64> {
        if let Some(id) = self.by_identifier.get(identifier) {
            if self.by_id.get(id).is_some_and(|known| known == identifier) {
                return Ok(u64::from(*id));
            }
            anyhow::bail!("ext toplevel id registry collision for identifier {identifier:?}");
        }

        let mut id = self
            .next
            .ok_or_else(|| anyhow::anyhow!("ext toplevel numeric id space exhausted"))?;
        while self.by_id.contains_key(&id) {
            id = id
                .checked_add(1)
                .filter(|candidate| *candidate <= EXT_ID_NAMESPACE_END)
                .ok_or_else(|| anyhow::anyhow!("ext toplevel numeric id space exhausted"))?;
        }
        self.next = (id < EXT_ID_NAMESPACE_END).then_some(id + 1);
        self.by_identifier.insert(identifier.to_owned(), id);
        self.by_id.insert(id, identifier.to_owned());
        Ok(u64::from(id))
    }

    fn identifier(&self, id: u64) -> Option<String> {
        self.by_id.get(&u32::try_from(id).ok()?).cloned()
    }
}

fn id_registry() -> &'static Mutex<IdRegistry> {
    static REGISTRY: OnceLock<Mutex<IdRegistry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(IdRegistry::default()))
}

fn stable_id(identifier: &str) -> anyhow::Result<u64> {
    id_registry()
        .lock()
        .map_err(|_| anyhow::anyhow!("ext toplevel id registry lock poisoned"))?
        .id_for(identifier)
}

/// The ext toplevel `identifier` a window_id from this module stands for.
pub fn identifier_for(id: u64) -> Option<String> {
    id_registry().lock().ok()?.identifier(id)
}

/// Enumerate ext toplevels and enrich them with AT-SPI pid and geometry data.
pub fn list_windows() -> anyhow::Result<Vec<WindowInfo>> {
    enumerate(false).map(|windows| windows.unwrap_or_default())
}

/// [`list_windows`], or `None` when the compositor cannot copy a toplevel
/// through `ext_foreign_toplevel_image_capture_source_manager_v1`.
pub fn list_capturable_windows() -> anyhow::Result<Option<Vec<WindowInfo>>> {
    enumerate(true)
}

fn enumerate(require_capture: bool) -> anyhow::Result<Option<Vec<WindowInfo>>> {
    let conn = Connection::connect_to_env()?;
    let mut queue = conn.new_event_queue::<State>();
    let qh = queue.handle();
    conn.display().get_registry(&qh, ());

    let mut state = State::default();
    queue.roundtrip(&mut state)?;
    if state.manager.is_none() {
        anyhow::bail!("compositor does not expose ext_foreign_toplevel_list_v1");
    }
    if require_capture && !state.capture_globals.iter().all(|advertised| *advertised) {
        return Ok(None);
    }
    for _ in 0..3 {
        queue.roundtrip(&mut state)?;
    }

    let mut native = Vec::with_capacity(state.toplevels.len());
    for record in state.toplevels.into_values() {
        if record.identifier.is_empty() {
            tracing::warn!("ignoring ext toplevel whose done batch had no identifier");
            continue;
        }
        native.push((stable_id(&record.identifier)?, record));
    }
    native.sort_unstable_by_key(|(id, _)| *id);
    Ok(Some(merge_atspi_records(
        native,
        crate::atspi::list_windows(None),
        require_capture,
    )))
}

/// Pair ext records with AT-SPI windows for pid and geometry. A capturable
/// listing pairs only unambiguous matches: its window_id yields that
/// toplevel's own pixels, so a pid guessed from a repeated title or app_id
/// would bind them to another application's accessibility tree.
fn merge_atspi_records(
    native: Vec<(u64, ToplevelRecord)>,
    atspi: Vec<WindowInfo>,
    unambiguous: bool,
) -> Vec<WindowInfo> {
    let mut claimed = vec![false; atspi.len()];
    let mut out = Vec::with_capacity(native.len() + atspi.len());
    let unique_title: Vec<bool> = native
        .iter()
        .map(|(_, record)| {
            native
                .iter()
                .filter(|(_, other)| other.title == record.title)
                .count()
                == 1
        })
        .collect();
    let unique_app_id: Vec<bool> = native
        .iter()
        .map(|(_, record)| {
            native
                .iter()
                .filter(|(_, other)| other.app_id == record.app_id)
                .count()
                == 1
        })
        .collect();

    for (position, (id, record)) in native.into_iter().enumerate() {
        let title_matches =
            |window: &WindowInfo| !record.title.is_empty() && window.title == record.title;
        let app_id_matches =
            |window: &WindowInfo| !record.app_id.is_empty() && window.app_name == record.app_id;
        let matching_atspi = if unambiguous {
            unique_title[position]
                .then(|| sole_match(&atspi, &claimed, title_matches))
                .flatten()
                .or_else(|| {
                    unique_app_id[position]
                        .then(|| sole_match(&atspi, &claimed, app_id_matches))
                        .flatten()
                })
        } else {
            atspi
                .iter()
                .enumerate()
                .find_map(|(index, window)| {
                    (!claimed[index] && title_matches(window)).then_some(index)
                })
                .or_else(|| unique_match(&atspi, &claimed, app_id_matches))
        };

        let mut window = WindowInfo {
            xid: id,
            pid: None,
            app_name: record.app_id,
            title: record.title,
            is_on_screen: true,
            z_index: None,
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        };
        if let Some(index) = matching_atspi {
            claimed[index] = true;
            let enrichment = &atspi[index];
            window.pid = enrichment.pid;
            window.is_on_screen = enrichment.is_on_screen;
            window.z_index = enrichment.z_index;
            window.x = enrichment.x;
            window.y = enrichment.y;
            window.width = enrichment.width;
            window.height = enrichment.height;
            if window.app_name.is_empty() {
                window.app_name = enrichment.app_name.clone();
            }
            if window.title.is_empty() {
                window.title = enrichment.title.clone();
            }
        }
        out.push(window);
    }

    out.extend(
        atspi
            .into_iter()
            .enumerate()
            .filter_map(|(index, window)| (!claimed[index]).then_some(window)),
    );
    out
}

fn unique_match(
    windows: &[WindowInfo],
    claimed: &[bool],
    predicate: impl Fn(&WindowInfo) -> bool,
) -> Option<usize> {
    let mut matches = windows
        .iter()
        .enumerate()
        .filter(|(index, window)| !claimed[*index] && predicate(window));
    let (index, _) = matches.next()?;
    matches.next().is_none().then_some(index)
}

/// The one window of all `windows`, claimed or not, that `predicate`
/// accepts, provided no earlier record claimed it.
fn sole_match(
    windows: &[WindowInfo],
    claimed: &[bool],
    predicate: impl Fn(&WindowInfo) -> bool,
) -> Option<usize> {
    let index = unique_match(windows, &vec![false; windows.len()], predicate)?;
    (!claimed[index]).then_some(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(identifier: &str, title: &str, app_id: &str) -> ToplevelRecord {
        ToplevelRecord {
            identifier: identifier.to_owned(),
            title: title.to_owned(),
            app_id: app_id.to_owned(),
        }
    }

    fn atspi(xid: u64, pid: u32, title: &str, app_name: &str, x: i32) -> WindowInfo {
        WindowInfo {
            xid,
            pid: Some(pid),
            app_name: app_name.to_owned(),
            title: title.to_owned(),
            is_on_screen: true,
            z_index: None,
            x,
            y: 20,
            width: 800,
            height: 600,
        }
    }

    #[test]
    fn id_registry_is_stable_and_bijective() {
        let mut registry = IdRegistry::default();
        let first = registry.id_for("opaque-a").unwrap();
        let second = registry.id_for("opaque-b").unwrap();
        assert_eq!(registry.id_for("opaque-a").unwrap(), first);
        assert_ne!(first, second);
        assert_eq!(
            registry
                .by_id
                .get(&u32::try_from(first).unwrap())
                .map(String::as_str),
            Some("opaque-a")
        );
        assert_eq!(
            registry
                .by_id
                .get(&u32::try_from(second).unwrap())
                .map(String::as_str),
            Some("opaque-b")
        );
    }

    #[test]
    fn id_registry_maps_issued_ids_back_to_their_identifier() {
        let mut registry = IdRegistry::default();
        let id = registry.id_for("opaque-a").unwrap();
        assert_eq!(registry.identifier(id).as_deref(), Some("opaque-a"));
        assert_eq!(registry.identifier(id + 1), None);
        assert_eq!(registry.identifier(u64::from(u32::MAX) + 1), None);
    }

    #[test]
    fn merge_recovers_metadata_and_preserves_unmatched_records() {
        let native = vec![
            (
                u64::from(EXT_ID_NAMESPACE_START),
                record("a", "Editor", "org.editor"),
            ),
            (
                u64::from(EXT_ID_NAMESPACE_START + 1),
                record("b", "Other", "org.viewer"),
            ),
            (
                u64::from(EXT_ID_NAMESPACE_START + 2),
                record("c", "Native only", "org.native"),
            ),
        ];
        let merged = merge_atspi_records(
            native,
            vec![
                atspi(11, 101, "Editor", "unrelated-name", 10),
                atspi(12, 202, "Different title", "org.viewer", 30),
                atspi(13, 303, "AT-SPI only", "org.extra", 50),
            ],
            false,
        );

        assert_eq!(merged.len(), 4);
        assert_eq!(
            (merged[0].xid, merged[0].pid, merged[0].x),
            (u64::from(EXT_ID_NAMESPACE_START), Some(101), 10)
        );
        assert_eq!(merged[1].pid, Some(202));
        assert_eq!(merged[2].pid, None);
        assert_eq!((merged[3].xid, merged[3].pid), (13, Some(303)));
    }

    #[test]
    fn merge_does_not_guess_ambiguous_app_ids() {
        let merged = merge_atspi_records(
            vec![(
                u64::from(EXT_ID_NAMESPACE_START),
                record("a", "", "org.same"),
            )],
            vec![
                atspi(11, 101, "One", "org.same", 10),
                atspi(12, 202, "Two", "org.same", 30),
            ],
            false,
        );
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0].pid, None);
        assert_eq!(merged[1].pid, Some(101));
        assert_eq!(merged[2].pid, Some(202));
    }

    #[test]
    fn merge_pairs_repeated_exact_titles_one_to_one() {
        let merged = merge_atspi_records(
            vec![
                (
                    u64::from(EXT_ID_NAMESPACE_START),
                    record("a", "Document", ""),
                ),
                (
                    u64::from(EXT_ID_NAMESPACE_START + 1),
                    record("b", "Document", ""),
                ),
            ],
            vec![
                atspi(11, 101, "Document", "first", 10),
                atspi(12, 202, "Document", "second", 30),
            ],
            false,
        );
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].pid, Some(101));
        assert_eq!(merged[1].pid, Some(202));
    }

    #[test]
    fn capturable_merge_refuses_repeated_titles_and_app_ids() {
        let merged = merge_atspi_records(
            vec![
                (
                    u64::from(EXT_ID_NAMESPACE_START),
                    record("a", "Document", "org.first"),
                ),
                (
                    u64::from(EXT_ID_NAMESPACE_START + 1),
                    record("b", "Document", "org.second"),
                ),
                (
                    u64::from(EXT_ID_NAMESPACE_START + 2),
                    record("c", "", "org.same"),
                ),
                (
                    u64::from(EXT_ID_NAMESPACE_START + 3),
                    record("d", "", "org.same"),
                ),
                (
                    u64::from(EXT_ID_NAMESPACE_START + 4),
                    record("e", "Unique", "org.unique"),
                ),
            ],
            vec![
                atspi(11, 101, "Document", "first", 10),
                atspi(13, 303, "Other", "org.same", 50),
                atspi(14, 404, "Unique", "org.unique", 70),
            ],
            true,
        );

        let pids: Vec<_> = merged[..5].iter().map(|window| window.pid).collect();
        assert_eq!(pids, [None, None, None, None, Some(404)]);
        assert_eq!(merged[4].x, 70);
        assert_eq!(merged.len(), 7);
    }

    #[test]
    fn capturable_merge_counts_windows_an_earlier_record_claimed() {
        let merged = merge_atspi_records(
            vec![
                (
                    u64::from(EXT_ID_NAMESPACE_START),
                    record("a", "Untitled", "org.editor"),
                ),
                (
                    u64::from(EXT_ID_NAMESPACE_START + 1),
                    record("b", "Shared", "org.other"),
                ),
                (
                    u64::from(EXT_ID_NAMESPACE_START + 2),
                    record("c", "Viewer A", "org.c"),
                ),
                (
                    u64::from(EXT_ID_NAMESPACE_START + 3),
                    record("d", "Unlisted", "org.viewer"),
                ),
            ],
            vec![
                atspi(11, 101, "Shared", "org.editor", 10),
                atspi(12, 202, "Shared", "org.unrelated", 30),
                atspi(13, 303, "Viewer A", "org.viewer", 50),
                atspi(14, 404, "Viewer B", "org.viewer", 70),
            ],
            true,
        );

        // `a` claims 11 by app_id, yet `b`'s title still matches two AT-SPI
        // windows; `c` claims 13 by title, yet `d`'s app_id still matches two.
        let pids: Vec<_> = merged[..4].iter().map(|window| window.pid).collect();
        assert_eq!(pids, [Some(101), None, Some(303), None]);
    }

    #[test]
    fn id_registry_skips_collisions_and_reports_exhaustion() {
        let mut registry = IdRegistry::default();
        registry
            .by_id
            .insert(EXT_ID_NAMESPACE_START, "already-used".to_owned());
        assert_eq!(
            registry.id_for("next").unwrap(),
            u64::from(EXT_ID_NAMESPACE_START + 1)
        );

        registry.next = Some(EXT_ID_NAMESPACE_END);
        assert_eq!(
            registry.id_for("last").unwrap(),
            u64::from(EXT_ID_NAMESPACE_END)
        );
        assert!(registry.id_for("exhausted").is_err());
    }
}
