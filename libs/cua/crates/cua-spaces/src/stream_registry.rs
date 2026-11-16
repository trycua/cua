//! Cross-Space window streams: a window of one Space shown in another.
//!
//! An agent that makes a child Space and opens an app there shows the
//! window in its own Space with `stream_space_window(space=<child>,
//! into=<parent>)`. That registers a [`StreamEntry`] with the daemon (this
//! registry lives in the [`Spaces`] runtime the daemon hosts). The parent's
//! Stream section is its own windows plus its entries: [`Spaces::stream_windows`]
//! lists them as window rows whose [`WindowTarget::origin`] names the Space
//! the picture comes from, and their handle ([`entry_window_id`]) opens through
//! [`Spaces::open_stream_entry`], so a shell that only knows windows needs no
//! second code path.
//!
//! Opening decides the route once, at open time ([`decide`]):
//!
//! - **Direct.** The viewer owns the origin Space or holds a grant on it
//!   ([`Access`]): the ticket is minted by the origin's own spacesd, scoped
//!   to that Space and that window, and the client connects straight to it.
//!   Nothing is relayed through the parent and nothing is encoded twice. The
//!   viewer's presence is published to the origin's channel as well as the
//!   parent's ([`crate::presence::MergedRoster`] folds the two into one avatar
//!   and one cursor per viewer), and input reaches the origin window with the
//!   semantics of a stream of the parent.
//! - **Proxied.** The viewer cannot reach the origin: the ticket is minted by
//!   the runtime that holds the origin (this one) and the client attaches
//!   through that runtime's media bridge ([`StreamTicket::needs_gateway_headers`]
//!   is set), the one hop the parent's host always offered.
//!
//! An entry ends with its window, with its origin Space (stopped, suspended
//! or deleted) and with its parent (deleted): [`Spaces::stream_windows`]
//! drops it at the next listing at the latest, and the power and delete
//! operations drop it at once.

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::stream::{StreamOptions, StreamTarget, StreamTicket, WindowTarget};
use crate::{Space, Spaces};

/// Prefix of a registry entry's window handle in its parent's window list.
pub const ENTRY_PREFIX: &str = "xs:";

/// The window handle an entry has in its parent's window list.
pub fn entry_window_id(entry_id: &str) -> String {
    format!("{ENTRY_PREFIX}{entry_id}")
}

/// The entry id of a window handle from [`entry_window_id`], `None` for an
/// ordinary window of the Space.
pub fn entry_of_window_id(window_id: &str) -> Option<&str> {
    window_id
        .strip_prefix(ENTRY_PREFIX)
        .filter(|s| !s.is_empty())
}

/// What the viewer may do with the origin Space.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "role", rename_all = "kebab-case")]
pub enum Access {
    /// The viewer's account owns it (or it is a Space on this computer).
    Owner,
    /// A grant: `editor` or `viewer`.
    Grant(String),
    /// Neither: only the parent's host can reach it.
    None,
}

impl Access {
    /// Whether the viewer reaches the origin on their own.
    pub fn reaches_origin(&self) -> bool {
        !matches!(self, Access::None)
    }
}

/// How a stream reaches the viewer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Route {
    /// Straight from the origin Space.
    Direct,
    /// Through the runtime that holds the origin (the parent's host).
    Proxied,
}

/// The route for a viewer: direct whenever they reach the origin.
pub fn decide(access: &Access) -> Route {
    if access.reaches_origin() {
        Route::Direct
    } else {
        Route::Proxied
    }
}

/// A window of one Space shown in another.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamEntry {
    /// Stable id (`xs:` + this is the window handle in the parent's list).
    pub id: String,
    /// The Space that shows it (canonical id).
    pub parent: String,
    /// The Space the window lives in (canonical id).
    pub origin_space: String,
    /// The origin's name, for the row's label.
    pub origin_name: String,
    /// The origin's OS (`macos`, `linux`, `windows`, or empty).
    pub os: String,
    /// The window's handle in the origin.
    pub window_id: String,
    /// Its app.
    pub app_name: String,
    /// Its app id.
    #[serde(default)]
    pub app_id: String,
    /// Its title.
    pub title: String,
    /// Its owning process.
    #[serde(default)]
    pub pid: u32,
    /// Always `window` today (the field leaves room for more).
    pub kind: String,
    /// When it was registered (Unix seconds).
    pub created_at_unix: u64,
}

/// Where a window row's picture comes from; absent on a Space's own windows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamOrigin {
    /// The origin Space (canonical id).
    pub space_id: String,
    /// Its name.
    pub name: String,
    /// Its OS.
    #[serde(default)]
    pub os: String,
    /// The window's handle in the origin.
    pub window_id: String,
}

impl StreamEntry {
    /// The entry as a row of its parent's window list.
    pub fn as_window(&self) -> WindowTarget {
        WindowTarget {
            window_id: entry_window_id(&self.id),
            epoch: 0,
            title: self.title.clone(),
            app_name: self.app_name.clone(),
            app_id: self.app_id.clone(),
            pid: self.pid,
            bounds: [0.0, 0.0, 1280.0, 800.0],
            on_screen: true,
            focused: false,
            available: true,
            limitation: String::new(),
            origin: Some(StreamOrigin {
                space_id: self.origin_space.clone(),
                name: self.origin_name.clone(),
                os: self.os.clone(),
                window_id: self.window_id.clone(),
            }),
        }
    }
}

/// The daemon's entries.
#[derive(Default)]
pub struct StreamRegistry {
    entries: Mutex<Vec<StreamEntry>>,
}

impl StreamRegistry {
    /// Adds an entry, or returns the one already showing this window in this
    /// parent.
    pub fn register(&self, entry: StreamEntry) -> StreamEntry {
        let mut all = self.entries.lock().expect("stream registry");
        if let Some(e) = all.iter_mut().find(|e| {
            e.parent == entry.parent
                && e.origin_space == entry.origin_space
                && e.window_id == entry.window_id
        }) {
            // The same window again: refresh what may have changed.
            e.title = entry.title;
            e.app_name = entry.app_name;
            e.origin_name = entry.origin_name;
            return e.clone();
        }
        all.push(entry.clone());
        entry
    }

    /// The entries one parent shows.
    pub fn of_parent(&self, parent: &str) -> Vec<StreamEntry> {
        self.entries
            .lock()
            .expect("stream registry")
            .iter()
            .filter(|e| e.parent == parent)
            .cloned()
            .collect()
    }

    /// One entry.
    pub fn get(&self, id: &str) -> Option<StreamEntry> {
        self.entries
            .lock()
            .expect("stream registry")
            .iter()
            .find(|e| e.id == id)
            .cloned()
    }

    /// Every entry.
    pub fn all(&self) -> Vec<StreamEntry> {
        self.entries.lock().expect("stream registry").clone()
    }

    /// Drops one entry; whether it was there.
    pub fn remove(&self, id: &str) -> bool {
        let mut all = self.entries.lock().expect("stream registry");
        let before = all.len();
        all.retain(|e| e.id != id);
        before != all.len()
    }

    /// Drops every entry that shows a window of `space` or is shown by it
    /// (the Space stopped or was deleted); how many.
    pub fn drop_space(&self, space: &str) -> usize {
        let mut all = self.entries.lock().expect("stream registry");
        let before = all.len();
        all.retain(|e| e.parent != space && e.origin_space != space);
        before - all.len()
    }

    /// Drops the entries of `origin` whose window is not in `live` (the
    /// handles the origin lists now); how many.
    pub fn retain_windows(&self, origin: &str, live: &[String]) -> usize {
        let mut all = self.entries.lock().expect("stream registry");
        let before = all.len();
        all.retain(|e| e.origin_space != origin || live.contains(&e.window_id));
        before - all.len()
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// An opened entry: the ticket and how it reaches the viewer.
#[derive(Clone, Debug, Serialize)]
pub struct EntryStream {
    /// The entry.
    pub entry: StreamEntry,
    /// How it reaches the viewer.
    pub route: Route,
    /// What the viewer holds on the origin.
    pub access: Access,
    /// The Space whose spacesd minted the ticket: the origin for a direct
    /// stream.
    pub minted_by: String,
    /// The ticket, scoped to that Space and the entry's window; `None` when
    /// the caller only asked for the decision ([`Spaces::resolve_stream_entry`]).
    pub ticket: Option<StreamTicket>,
}

impl Spaces {
    /// What the viewer (this runtime's account) may do with `origin`.
    pub async fn viewer_access(&self, origin: &str) -> Access {
        match self.find(origin).await {
            Ok(info) => match info.sharing.as_ref().map(|s| s.role.as_str()) {
                None | Some("") | Some("owner") => Access::Owner,
                Some(role) => Access::Grant(role.to_string()),
            },
            Err(_) => Access::None,
        }
    }

    /// Shows `origin`'s window in `parent` (`stream_space_window` with
    /// `into`).
    pub async fn register_stream(
        &self,
        parent: &str,
        origin: &Space,
        window: &WindowTarget,
        title: Option<String>,
    ) -> Result<StreamEntry> {
        let parent_id = self.resolve_now(parent).await?.to_string();
        let origin_id = origin.id().to_string();
        if parent_id == origin_id {
            return Err(Error::invalid(
                "into is the Space the window lives in: it is already in that Space's Stream section",
            ));
        }
        let info = self.find(&origin_id).await.ok();
        let entry = StreamEntry {
            id: format!(
                "{:016x}",
                rand_id(&parent_id, &origin_id, &window.window_id)
            ),
            parent: parent_id,
            origin_name: info
                .as_ref()
                .map(|i| i.shown_name().to_string())
                .unwrap_or_else(|| origin.name().to_string()),
            os: info.map(|i| i.os).unwrap_or_default(),
            origin_space: origin_id,
            window_id: window.window_id.clone(),
            app_name: window.app_name.clone(),
            app_id: window.app_id.clone(),
            title: title.unwrap_or_else(|| window.title.clone()),
            pid: window.pid,
            kind: "window".into(),
            created_at_unix: now_unix(),
        };
        Ok(self.inner.streams.register(entry))
    }

    /// The window rows a Space shows from other Spaces. Entries whose window
    /// closed or whose origin is gone are dropped first.
    pub async fn stream_windows(&self, parent: &str) -> Vec<WindowTarget> {
        let Ok(parent_id) = self.resolve_now(parent).await.map(|i| i.to_string()) else {
            return Vec::new();
        };
        let entries = self.inner.streams.of_parent(&parent_id);
        let mut origins: Vec<String> = Vec::new();
        for e in &entries {
            if !origins.contains(&e.origin_space) {
                origins.push(e.origin_space.clone());
            }
        }
        for origin in origins {
            self.prune_origin(&origin).await;
        }
        self.inner
            .streams
            .of_parent(&parent_id)
            .iter()
            .map(StreamEntry::as_window)
            .collect()
    }

    /// Drops the entries of `origin` that no longer hold: the Space is gone
    /// or off, or the window closed.
    async fn prune_origin(&self, origin: &str) {
        let info = match self.find(origin).await {
            Ok(i) => i,
            Err(Error::NotFound(_)) => {
                self.inner.streams.drop_space(origin);
                return;
            }
            // Unknown: keep what is registered rather than flicker.
            Err(_) => return,
        };
        if matches!(info.power_state.as_str(), "stopped" | "suspended") {
            self.inner.streams.drop_space(origin);
            return;
        }
        let Ok(space) = self.space(origin).await else {
            return;
        };
        if let Ok(windows) = space.windows(None).await {
            let live: Vec<String> = windows.into_iter().map(|w| w.window_id).collect();
            self.inner.streams.retain_windows(origin, &live);
        }
    }

    /// A Space was stopped, suspended or deleted: its entries, and the ones
    /// it shows, end now.
    pub(crate) fn drop_stream_entries(&self, space: &str) {
        let id = self
            .resolve(space)
            .map(|i| i.to_string())
            .unwrap_or_else(|_| space.to_string());
        self.inner.streams.drop_space(&id);
    }

    /// The route a registry entry streams by for this viewer, without
    /// opening anything.
    pub async fn resolve_stream_entry(&self, entry_id: &str) -> Result<EntryStream> {
        let entry = self
            .inner
            .streams
            .get(entry_of_window_id(entry_id).unwrap_or(entry_id))
            .ok_or_else(|| Error::NotFound(format!("stream entry {entry_id}")))?;
        let access = self.viewer_access(&entry.origin_space).await;
        Ok(EntryStream {
            route: decide(&access),
            minted_by: entry.origin_space.clone(),
            entry,
            access,
            ticket: None,
        })
    }

    /// Opens the stream of a registry entry for the viewer; see the module
    /// for the route.
    pub async fn open_stream_entry(
        &self,
        entry_id: &str,
        options: StreamOptions,
    ) -> Result<EntryStream> {
        let EntryStream {
            entry,
            access,
            route,
            ..
        } = self.resolve_stream_entry(entry_id).await?;
        let origin = self.space(&entry.origin_space).await?;
        let ticket = origin
            .open_stream(StreamTarget::Window(entry.window_id.clone()), options)
            .await
            .inspect_err(|_| {
                // A window that cannot be opened is gone.
                self.inner.streams.remove(&entry.id);
            })?;
        let ticket = match route {
            Route::Direct => ticket,
            Route::Proxied => StreamTicket {
                needs_gateway_headers: true,
                ..ticket
            },
        };
        Ok(EntryStream {
            minted_by: entry.origin_space.clone(),
            entry,
            route,
            access,
            ticket: Some(ticket),
        })
    }
}

fn rand_id(a: &str, b: &str, c: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    a.hash(&mut h);
    b.hash(&mut h);
    c.hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(parent: &str, origin: &str, window: &str) -> StreamEntry {
        StreamEntry {
            id: format!("{parent}-{origin}-{window}"),
            parent: parent.into(),
            origin_space: origin.into(),
            origin_name: "mac-child".into(),
            os: "macos".into(),
            window_id: window.into(),
            app_name: "Safari".into(),
            app_id: "com.apple.Safari".into(),
            title: "Apple".into(),
            pid: 42,
            kind: "window".into(),
            created_at_unix: 1,
        }
    }

    #[test]
    fn the_owner_and_a_grant_stream_directly_and_no_access_is_proxied() {
        assert_eq!(decide(&Access::Owner), Route::Direct);
        assert_eq!(decide(&Access::Grant("viewer".into())), Route::Direct);
        assert_eq!(decide(&Access::Grant("editor".into())), Route::Direct);
        assert_eq!(decide(&Access::None), Route::Proxied);
    }

    #[test]
    fn a_grant_revoked_mid_stream_decides_proxied_at_the_next_open() {
        let mut access = Access::Grant("viewer".into());
        assert_eq!(decide(&access), Route::Direct);
        access = Access::None;
        assert_eq!(decide(&access), Route::Proxied);
    }

    #[test]
    fn registering_the_same_window_twice_keeps_one_entry() {
        let r = StreamRegistry::default();
        let a = r.register(entry("p", "o", "w1"));
        let mut again = entry("p", "o", "w1");
        again.title = "Apple (2)".into();
        again.id = "other".into();
        let b = r.register(again);
        assert_eq!(a.id, b.id);
        assert_eq!(b.title, "Apple (2)");
        assert_eq!(r.of_parent("p").len(), 1);
    }

    #[test]
    fn an_entry_ends_with_its_window() {
        let r = StreamRegistry::default();
        r.register(entry("p", "o", "w1"));
        r.register(entry("p", "o", "w2"));
        assert_eq!(r.retain_windows("o", &["w2".into()]), 1);
        let left = r.of_parent("p");
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].window_id, "w2");
        // Another origin's windows are untouched.
        r.register(entry("p", "o2", "x"));
        assert_eq!(r.retain_windows("o", &[]), 1);
        assert_eq!(r.of_parent("p").len(), 1);
    }

    #[test]
    fn an_entry_ends_with_its_origin_or_its_parent() {
        let r = StreamRegistry::default();
        r.register(entry("p", "o", "w1"));
        r.register(entry("p2", "o", "w1"));
        r.register(entry("p", "o3", "w1"));
        // The origin stops: both parents lose it.
        assert_eq!(r.drop_space("o"), 2);
        assert_eq!(r.all().len(), 1);
        // The parent is deleted.
        assert_eq!(r.drop_space("p"), 1);
        assert!(r.all().is_empty());
    }

    #[test]
    fn a_row_carries_its_origin_and_its_handle_opens_the_entry() {
        let e = entry("p", "o", "w1");
        let row = e.as_window();
        assert_eq!(entry_of_window_id(&row.window_id), Some(e.id.as_str()));
        assert_eq!(entry_of_window_id("1234"), None);
        assert_eq!(entry_of_window_id("xs:"), None);
        let origin = row.origin.clone().expect("origin");
        assert_eq!(origin.space_id, "o");
        assert_eq!(origin.name, "mac-child");
        assert_eq!(origin.os, "macos");
        assert!(row.is_streamable());
    }

    #[test]
    fn an_old_row_without_an_origin_still_parses_and_stays_off_the_wire() {
        let old = r#"{"window_id":"7","epoch":1,"title":"t","app_name":"a","app_id":"","pid":1,
            "bounds":[0,0,100,100],"on_screen":true,"focused":false,"available":true}"#;
        let w: WindowTarget = serde_json::from_str(old).expect("old row parses");
        assert!(w.origin.is_none());
        let out = serde_json::to_value(&w).unwrap();
        assert!(out.get("origin").is_none());
        let with = serde_json::to_value(entry("p", "o", "w").as_window()).unwrap();
        assert_eq!(with["origin"]["space_id"], "o");
    }
}
