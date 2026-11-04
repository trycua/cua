// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Notifications: what the daemon's feed (`notifications_list`) becomes on
//! screen.
//!
//! The daemon keeps the feed while no app is open (persistent agents'
//! answers, `notify_user`, access requests, failures). An app polls it and
//! asks [`notifications_plan`] which entries to post as system
//! notifications: each entry at most once (by id), none of the backlog on
//! the first run, and never a flood (the newest few, then one summary). The
//! last-seen marker lives in the app settings file
//! ([`crate::settings::AppSettings::notifications_seen_ms`]), so a restart
//! does not post again.

use serde::{Deserialize, Serialize};

use crate::persistent::{LineView, ago};

/// Posted one by one at most; the rest become one summary.
pub const MAX_POSTS: usize = 3;

/// One feed entry (`notifications_list`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NotificationInput {
    pub id: String,
    pub at_ms: u64,
    pub agent: Option<String>,
    /// `turn_ended`, `message`, `approval`, `error`.
    pub kind: String,
    pub title: String,
    pub body: String,
    pub read: bool,
}

/// A system notification to post.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemNote {
    pub id: String,
    pub title: String,
    pub body: String,
}

/// What to post now, and the marker to save.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationsPlan {
    pub post: Vec<SystemNote>,
    /// The new `notifications_seen_ms` (unchanged when nothing is newer).
    pub seen_ms: u64,
}

fn first_line(s: &str, n: usize) -> String {
    let l = s
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    if l.chars().count() <= n {
        l.to_string()
    } else {
        let mut t: String = l.chars().take(n.saturating_sub(1)).collect();
        t.push('\u{2026}');
        t
    }
}

/// Which feed entries to post, given the last-seen marker (`0`: first
/// run, post nothing and mark everything seen).
pub fn notifications_plan(feed: &[NotificationInput], seen_ms: u64) -> NotificationsPlan {
    let newest = feed.iter().map(|n| n.at_ms).max().unwrap_or(0).max(seen_ms);
    if seen_ms == 0 {
        return NotificationsPlan {
            post: vec![],
            seen_ms: newest,
        };
    }
    let mut fresh: Vec<&NotificationInput> = feed
        .iter()
        .filter(|n| n.at_ms > seen_ms && !n.read)
        .collect();
    fresh.sort_by(|a, b| b.at_ms.cmp(&a.at_ms).then(a.id.cmp(&b.id)));
    fresh.dedup_by(|a, b| a.id == b.id);
    let mut post: Vec<SystemNote> = fresh
        .iter()
        .take(MAX_POSTS)
        .map(|n| SystemNote {
            id: n.id.clone(),
            title: n.title.clone(),
            body: first_line(&n.body, 200),
        })
        .collect();
    if fresh.len() > MAX_POSTS {
        let more = fresh.len() - MAX_POSTS;
        post.push(SystemNote {
            id: format!("more-{newest}"),
            title: "Cua".into(),
            body: format!("{more} more from your agents"),
        });
    }
    NotificationsPlan {
        post,
        seen_ms: newest,
    }
}

/// The notifications list as drawn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationsView {
    pub title: String,
    /// One line each: `title: body`, the time on the right; `on` is unread.
    pub rows: Vec<LineView>,
    pub empty_text: String,
    pub unread: u32,
    pub mark_all_label: Option<String>,
}

/// The list at `now_ms`, newest first.
pub fn notifications_view(feed: &[NotificationInput], now_ms: u64) -> NotificationsView {
    let mut rows: Vec<&NotificationInput> = feed.iter().collect();
    rows.sort_by_key(|n| std::cmp::Reverse(n.at_ms));
    let unread = feed.iter().filter(|n| !n.read).count() as u32;
    NotificationsView {
        title: "Notifications".into(),
        rows: rows
            .iter()
            .map(|n| {
                let body = first_line(&n.body, 80);
                LineView {
                    id: n.id.clone(),
                    text: if body.is_empty() {
                        n.title.clone()
                    } else {
                        format!("{}: {body}", n.title)
                    },
                    trailing: ago(n.at_ms, now_ms),
                    action_label: None,
                    secondary_label: None,
                    on: Some(!n.read),
                }
            })
            .collect(),
        empty_text: "No notifications.".into(),
        unread,
        mark_all_label: (unread > 0).then(|| "Mark all read".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(id: &str, at: u64) -> NotificationInput {
        NotificationInput {
            id: id.into(),
            at_ms: at,
            title: "ada".into(),
            body: "Your research is ready.\nDetails".into(),
            kind: "turn_ended".into(),
            ..Default::default()
        }
    }

    #[test]
    fn first_run_posts_nothing_then_only_new_once() {
        let feed = vec![n("a", 10), n("b", 20)];
        let p = notifications_plan(&feed, 0);
        assert!(p.post.is_empty());
        assert_eq!(p.seen_ms, 20);
        let feed = vec![n("a", 10), n("b", 20), n("c", 30)];
        let p = notifications_plan(&feed, 20);
        assert_eq!(p.post.len(), 1);
        assert_eq!(p.post[0].body, "Your research is ready.");
        assert_eq!(
            notifications_plan(&feed, p.seen_ms).post.len(),
            0,
            "never twice"
        );
        let many: Vec<_> = (0..6).map(|i| n(&format!("x{i}"), 100 + i)).collect();
        let p = notifications_plan(&many, 50);
        assert_eq!(p.post.len(), MAX_POSTS + 1);
        assert_eq!(p.post[3].body, "3 more from your agents");
        let v = notifications_view(&many, 200);
        assert_eq!(v.unread, 6);
        assert_eq!(v.rows[0].text, "ada: Your research is ready.");
    }
}
