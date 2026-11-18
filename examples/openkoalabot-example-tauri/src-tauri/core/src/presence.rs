// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Presence: who else is looking at (or driving) the Space, and where their
//! cursors are. Events fold into the SDK's `cua_spaces::presence::Roster`;
//! the shell shows [`Presence::avatars`] and draws the others' cursors.

use crate::{Error, Result};
use cua_spaces::Space;
use cua_spaces::presence::{
    Cursor, Identity, Participant, PresenceEvent, PresenceSession, PresenceView, Roster, now_ms,
};
use std::time::Duration;

/// An avatar in the presence bar.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Avatar {
    pub participant_id: String,
    /// The id it joined with; a Bot's is the Bot's id.
    pub principal_id: String,
    pub display_name: String,
    pub color: String,
    pub agent: bool,
    pub cursor: Option<(f64, f64)>,
    pub me: bool,
    /// The guest's cursor shape at `cursor` (the shared art's names).
    pub shape: String,
    /// Opacity of the cursor: fades out after 5 s without movement.
    pub alpha: f64,
}

fn avatar(p: &Participant, cursor: Option<&Cursor>, me: bool) -> Avatar {
    Avatar {
        participant_id: p.participant_id.clone(),
        principal_id: p.principal_id.clone(),
        display_name: p.display_name.clone(),
        color: p.color.clone(),
        agent: p.kind == "agent",
        cursor: cursor.filter(|c| c.visible).map(|c| (c.x, c.y)),
        me,
        shape: cursor.map(|c| c.shape.as_str()).unwrap_or("arrow").into(),
        alpha: 1.0,
    }
}

/// The avatars for `roster`: the caller first (`me`), each with their last
/// cursor. The roster itself is the SDK's (`cua_spaces::presence::Roster`).
pub fn avatars(roster: &Roster) -> Vec<Avatar> {
    roster
        .entries
        .iter()
        .map(|(p, c)| avatar(p, c.as_ref(), p.participant_id == roster.me))
        .collect()
}

/// [`avatars`], with each cursor where `view` draws it at `now` (local Unix
/// ms): interpolated, faded when idle, absent once faded out, and without the
/// participants the view dropped (a heartbeat no longer lists them, or their
/// agent run ended).
pub fn avatars_in(roster: &Roster, view: &mut PresenceView, now: f64) -> Vec<Avatar> {
    let drawn = view.drawables(now, None);
    let mut out = avatars(roster);
    out.retain(|a| a.me || view.participant(&a.participant_id).is_some());
    for a in &mut out {
        match drawn.iter().find(|d| d.participant_id == a.participant_id) {
            Some(d) => {
                a.cursor = Some((d.x, d.y));
                a.shape = d.shape.as_str().into();
                a.alpha = d.alpha;
            }
            None => a.cursor = None,
        }
    }
    out
}

/// A joined presence session plus its roster and the SDK's `PresenceView`
/// (interpolation, idle fade, heartbeat staleness).
pub struct Presence {
    session: PresenceSession,
    roster: Roster,
    view: PresenceView,
}

impl Presence {
    /// Joins as `display_name` (an agent when `agent`).
    pub async fn join(
        space: &Space,
        id: &str,
        display_name: &str,
        agent: bool,
        timeout: Duration,
    ) -> Result<Self> {
        Self::join_with_color(space, id, display_name, agent, None, timeout).await
    }

    /// [`Presence::join`], requesting `color` instead of the default.
    pub async fn join_with_color(
        space: &Space,
        id: &str,
        display_name: &str,
        agent: bool,
        color: Option<&str>,
        timeout: Duration,
    ) -> Result<Self> {
        // An agent joins with the SDK's agent identity, which requests its
        // stable presence color: the color of its avatar.
        let identity = if agent {
            Identity::agent(id, display_name)
        } else {
            Identity {
                id: id.to_string(),
                display_name: display_name.to_string(),
                ..Default::default()
            }
        };
        let mut identity = identity;
        if let Some(c) = color {
            identity.color = c.to_string();
        }
        let session = space.join_presence(identity, timeout).await?;
        let roster = Roster::from_session(&session);
        let view = session.view();
        Ok(Self {
            session,
            roster,
            view,
        })
    }

    /// My participant id.
    pub fn me(&self) -> &Participant {
        self.session.me()
    }

    /// Everyone, me first.
    pub fn avatars(&self) -> Vec<Avatar> {
        avatars(&self.roster)
    }

    /// Everyone, me first, with each cursor where the SDK's `PresenceView`
    /// draws it now: interpolated, faded when idle (`alpha`), and absent once
    /// faded out, gone from a heartbeat, or its agent run ended.
    pub fn avatars_now(&mut self) -> Vec<Avatar> {
        avatars_in(&self.roster, &mut self.view, now_ms())
    }

    /// The SDK roster (who is here, their last cursors).
    pub fn roster(&self) -> &Roster {
        &self.roster
    }

    /// Publishes my cursor (normalized 0..1).
    pub async fn move_cursor(&self, x: f64, y: f64) -> Result<()> {
        Ok(self.session.update_cursor(&Cursor::at(x, y)).await?)
    }

    /// Reads events for up to `timeout` / `max_events`, folding them in;
    /// returns them. Ending quietly on timeout is normal.
    pub async fn pump(
        &mut self,
        timeout: Duration,
        max_events: usize,
    ) -> Result<Vec<PresenceEvent>> {
        let deadline = tokio::time::Instant::now() + timeout;
        let mut out = Vec::new();
        for _ in 0..max_events {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            if left.is_zero() {
                break;
            }
            match self.session.next_event(left).await {
                Ok(Some(e)) => {
                    self.roster.apply(&e);
                    self.view.apply(&e, now_ms());
                    out.push(e);
                }
                Ok(None) => break,
                Err(cua_spaces::Error::Timeout(_)) => break,
                Err(e) => return Err(e.into()),
            }
        }
        Ok(out)
    }

    /// Waits for the first event matching `pred`, folding every event it
    /// reads into the roster: at most `max_events` events or `timeout`.
    pub async fn wait_for(
        &mut self,
        timeout: Duration,
        max_events: usize,
        mut pred: impl FnMut(&PresenceEvent) -> bool,
    ) -> Result<PresenceEvent> {
        let deadline = tokio::time::Instant::now() + timeout;
        for _ in 0..max_events {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            match self.session.next_event(left).await {
                Ok(Some(e)) => {
                    self.roster.apply(&e);
                    self.view.apply(&e, now_ms());
                    if pred(&e) {
                        return Ok(e);
                    }
                }
                Ok(None) => return Err(Error::Invalid("presence stream ended".into())),
                Err(cua_spaces::Error::Timeout(m)) => return Err(Error::Timeout(m)),
                Err(e) => return Err(e.into()),
            }
        }
        Err(Error::Timeout(format!(
            "no matching presence event in {max_events} events"
        )))
    }

    /// Leaves.
    pub async fn leave(self) -> Result<()> {
        Ok(self.session.leave().await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(id: &str, kind: &str) -> Participant {
        Participant {
            participant_id: id.into(),
            principal_id: format!("u-{id}"),
            display_name: id.to_uppercase(),
            color: "#123456".into(),
            kind: kind.into(),
        }
    }

    #[test]
    fn avatars_follow_the_sdk_roster_with_me_first() {
        let mut r = Roster {
            me: "me".into(),
            entries: vec![(p("me", "human"), None)],
        };
        r.apply(&PresenceEvent::Joined {
            participant: p("a", "agent"),
        });
        r.apply(&PresenceEvent::CursorMoved {
            participant_id: "a".into(),
            cursor: Cursor::at(0.25, 0.75),
        });
        let v = avatars(&r);
        assert_eq!(v.len(), 2);
        assert!(v[0].me && !v[0].agent);
        assert!(v[1].agent && !v[1].me);
        assert_eq!(v[1].cursor, Some((0.25, 0.75)));
        r.apply(&PresenceEvent::Left {
            participant_id: "a".into(),
            reason: String::new(),
        });
        assert_eq!(avatars(&r).len(), 1);
    }

    #[test]
    fn cursors_come_from_the_presence_view_fade_when_idle_and_go_on_run_end_or_heartbeat() {
        use cua_spaces::presence::CursorShape;
        let t0 = 1_000_000.0;
        let mut r = Roster {
            me: "me".into(),
            entries: vec![(p("me", "human"), None)],
        };
        let mut v = PresenceView::new("me", 100.0);
        v.upsert(p("me", "human"));
        fn both(r: &mut Roster, v: &mut PresenceView, e: PresenceEvent, t: f64) {
            r.apply(&e);
            v.apply(&e, t);
        }
        for id in ["a", "b"] {
            both(
                &mut r,
                &mut v,
                PresenceEvent::Joined {
                    participant: p(id, "agent"),
                },
                t0,
            );
            let mut c = Cursor::at(0.25, 0.75);
            c.shape = CursorShape::Text;
            c.shape_source = cua_spaces::presence::ShapeSource::HitTest;
            c.received_ms = t0;
            both(
                &mut r,
                &mut v,
                PresenceEvent::CursorMoved {
                    participant_id: id.into(),
                    cursor: c,
                },
                t0,
            );
        }
        let now = avatars_in(&r, &mut v, t0 + 200.0);
        assert_eq!(now.len(), 3);
        assert_eq!(now[1].cursor, Some((0.25, 0.75)));
        assert_eq!(now[1].shape, "text");
        assert_eq!(now[1].alpha, 1.0);
        // Idle 5.15 s: half faded.
        let idle = avatars_in(&r, &mut v, t0 + 5_150.0);
        assert!((idle[1].alpha - 0.5).abs() < 1e-6);
        // Agent a's run ends.
        both(
            &mut r,
            &mut v,
            PresenceEvent::Left {
                participant_id: "a".into(),
                reason: "run_ended".into(),
            },
            t0 + 5_200.0,
        );
        let after = avatars_in(&r, &mut v, t0 + 5_250.0);
        assert!(!after.iter().any(|a| a.participant_id == "a"));
        // A heartbeat without b drops b too.
        both(
            &mut r,
            &mut v,
            PresenceEvent::Heartbeat {
                participant_ids: vec!["me".into()],
            },
            t0 + 5_300.0,
        );
        let alone = avatars_in(&r, &mut v, t0 + 5_350.0);
        assert_eq!(alone.len(), 1);
        assert!(alone[0].me);
    }
}
