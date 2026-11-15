//! Whether a Space can take work now ([`Spaces::reach`]), asked just before
//! work is handed to it.
//!
//! A job started on one of your computers goes into its Space's thread, to
//! an agent there or one started for it. The thread lives on the relay, so a
//! post to a computer that just went away still succeeds: the job would sit
//! queued for it, and the app would never move it to another computer. So
//! the apps ask first. The answer reads the relay (the one source of a
//! computer's presence, [`cua_spaces_contract::presence`]), then asks the
//! Space's cua-spacesd for a health check over the connection the work
//! would use: a computer that was switched off a moment ago is still listed
//! online for a while, and only the check finds it gone.
//!
//! The apps call it as the app method `spaces.reach` (`CallSpaceTool`, like
//! `agent_keys.*`): it is not a contract tool, so no MCP server lists it.

use crate::Spaces;
use crate::id::SpaceId;
use serde::Serialize;
use std::time::Duration;

/// The longest [`Spaces::reach`] takes: a connect, a health check (at most
/// [`crate::spaces::LIVE_CHECK`]) and, when the cached connection is dead,
/// one dial through the relay. A live computer answers in one round trip.
pub const REACH_WITHIN: Duration = Duration::from_secs(8);

/// What [`Spaces::reach`] found.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Reach {
    /// The Space asked about, as given.
    pub space: String,
    /// It answered just now: work handed to it starts.
    pub reachable: bool,
    /// The relay lists its computer offline.
    pub listed_offline: bool,
    /// Why it can't take work, when it can't.
    pub reason: Option<String>,
}

/// The app methods this module serves (`SpaceService.CallSpaceTool` names).
pub const APP_METHODS: &[&str] = &["spaces.reach"];

/// Whether `name` is one of [`APP_METHODS`].
pub fn is_app_method(name: &str) -> bool {
    APP_METHODS.contains(&name)
}

/// The relay computer that carries Space `space`: the relay machine itself,
/// or the host a routed Space rides on.
fn carrier(space: &str) -> Option<String> {
    match SpaceId::parse(space).ok()? {
        SpaceId::Relay { machine_id } => Some(machine_id),
        SpaceId::Direct { authority } => {
            crate::id::routed_host(&authority).map(|(machine, _)| machine.to_string())
        }
        _ => None,
    }
}

impl Spaces {
    /// Whether Space `space` can take work now. Reads the relay's directory
    /// (at most a few seconds old), then connects and asks the Space's
    /// cua-spacesd for a health check; a cached connection that does not
    /// answer is dropped and the Space dialled again. Never takes longer
    /// than [`REACH_WITHIN`].
    ///
    /// A computer the relay lists offline is reachable only when its
    /// connection still answers (the LAN mesh may carry it); its reason
    /// then starts with [`cua_spaces_contract::presence::LISTED_OFFLINE`].
    pub async fn reach(&self, space: &str) -> Reach {
        let machine = carrier(space);
        if let Some(machine) = &machine {
            // A stale directory falls back to the last answer inside; an
            // error here leaves the connect below to say what is wrong.
            let _ = self.relay_directory(Some(machine.as_str())).await;
        }
        let attempt = async {
            let s = self.space(space).await?;
            self.checked(s).await
        };
        let reason = match tokio::time::timeout(REACH_WITHIN, attempt).await {
            Ok(Ok(_)) => None,
            Ok(Err(e)) => Some(e.to_string()),
            Err(_) => Some(format!(
                "{space} did not answer in {} s",
                REACH_WITHIN.as_secs()
            )),
        };
        let listed_offline = self.carrier_listed_offline(space);
        let reason = match (reason, listed_offline) {
            (Some(r), true) => Some(crate::spaces::listed_offline_reason(&r)),
            (r, _) => r,
        };
        Reach {
            space: space.to_string(),
            reachable: reason.is_none(),
            listed_offline,
            reason,
        }
    }
}

/// Runs one of [`APP_METHODS`] (`spaces.reach {"space": <id>}`); `None` for
/// any other name.
#[cfg(feature = "mcp")]
pub async fn app_call(
    spaces: &Spaces,
    name: &str,
    args: serde_json::Value,
) -> Option<crate::mcp::ToolOutcome> {
    if name != "spaces.reach" {
        return None;
    }
    let Some(space) = args.get("space").and_then(serde_json::Value::as_str) else {
        return Some(crate::mcp::ToolOutcome::error_message(
            "invalid_argument",
            "spaces.reach needs a space",
        ));
    };
    Some(crate::mcp::ToolOutcome::json(&spaces.reach(space).await))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_carrier_is_the_relay_machine_or_the_routed_host() {
        assert_eq!(
            carrier("relay:0123abcd4567ef89").as_deref(),
            Some("0123abcd4567ef89")
        );
        assert_eq!(carrier("local:dev"), None);
        assert!(is_app_method("spaces.reach"));
        assert!(!is_app_method("list_spaces"));
    }
}
