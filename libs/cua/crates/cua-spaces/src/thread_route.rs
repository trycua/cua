//! Where a Space's thread lives, and calls to it from other devices.
//!
//! A Space's thread and its agent members belong to the daemon that holds
//! the Space: for a Space a machine provides (`relay:<machine>` with a
//! host), that machine's cua daemon. This device's thread and agent member
//! tools ([`routed`]) given such a Space call the owner instead of
//! answering here: through the relay, the host's cua-spacesd
//! (`HostThreadService`), which forwards the call to its daemon with the
//! caller the relay verified (this device's signed-in account). The owner
//! decides what that account may do; nothing here vouches for anyone.
//!
//! Every path into the tools goes through here (the daemon's
//! `CallSpaceTool`, the MCP servers, the SDK's embedded runtime), so the
//! CLI, the SDK and the app reach a teammate's thread the same way.

use std::time::Duration;

use serde_json::{Value, json};

use crate::mcp::ToolOutcome;
use crate::{Error, Result, SpaceId, Spaces};
use cua_proto::env::v1 as pb;

/// The feature a host's cua-spacesd reports while it serves the threads of
/// the Spaces it provides.
pub const HOST_THREAD_FEATURE: &str = "host_thread";

/// The thread and agent member tools that run on a Space's owner.
pub const TOOLS: &[&str] = &[
    "thread_read",
    "thread_post",
    "thread_inbox",
    "thread_claim",
    "thread_reply",
    "agent_members",
    "agent_send",
];

/// The run tools that run on the owner when given a member's `name` (with a
/// `run_id` they reach the Space's own agent service, wherever it is).
pub const MEMBER_TOOLS: &[&str] = &[
    "agent_message",
    "agent_status",
    "agent_events",
    "agent_interrupt",
    "agent_stop",
];

/// Asks the relay for a Space thread grant: the Space (its relay machine
/// id) a person reaches the thread of on the machine that provides it,
/// when the Space is shared with them and that machine is not.
pub const THREAD_SPACE_HEADER: &str = "x-cua-thread-space";

/// The longest `thread_read` long poll (`follow_ms`).
pub const MAX_FOLLOW_MS: u64 = 25_000;

/// One relayed wait of a long poll at most: a longer `follow_ms` is several
/// calls, so no request through the relay stays open long.
pub const FOLLOW_CHUNK_MS: u64 = 15_000;

/// How long a relayed call may take besides its long poll.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);

/// Whether `tool` with `args` is a thread or agent member call that runs
/// where the Space's thread lives.
pub fn routed(tool: &str, args: &Value) -> bool {
    if TOOLS.contains(&tool) {
        return true;
    }
    MEMBER_TOOLS.contains(&tool)
        && args
            .get("name")
            .and_then(Value::as_str)
            .is_some_and(|n| !n.trim().is_empty())
}

/// What an older owner answers, in words.
pub fn owner_too_old(host: &str) -> String {
    format!(
        "{host} runs an older Cua that has no Space thread for other devices; update Cua on \
         the owner's machine ({host}) to read and post in this Space's thread from here"
    )
}

/// Where a Space's thread lives.
#[derive(Clone, Debug)]
pub enum ThreadHome {
    /// Here. `Some(id)`: under this id here (a Space this machine provides,
    /// named by its relay machine).
    Here(Option<String>),
    /// On the daemon of the machine `host` (which provides the Space), as
    /// `space` there. `granted`: that machine is not shared with this
    /// account, only the Space is; the relay grants its thread alone.
    Owner {
        host: Box<crate::relay::RelayMachine>,
        space: String,
        granted: bool,
    },
}

impl Spaces {
    /// Where the thread of `space` (an id or name) lives.
    pub async fn thread_home(&self, space: &str) -> Result<ThreadHome> {
        let machine = match self.resolve(space) {
            Ok(SpaceId::Relay { machine_id }) => machine_id,
            Ok(_) => return Ok(ThreadHome::Here(None)),
            Err(_) => match space.trim().strip_prefix("relay:") {
                Some(m) if !m.is_empty() => m.to_string(),
                _ => return Ok(ThreadHome::Here(None)),
            },
        };
        // A Space this machine provides: its thread is here, under its id
        // here.
        let host_dir = cua_host::Host::new(self.home_dir()).paths().dir.clone();
        if let Ok(all) = cua_host::provided::load_provided(&host_dir)
            && let Some(p) = all.iter().find(|p| p.relay_machine == machine)
        {
            return Ok(ThreadHome::Here(Some(p.local_space.clone())));
        }
        if self.relay().is_err() {
            return Ok(ThreadHome::Here(None));
        }
        let Some(row) = self.relay_row(&machine).await else {
            // Neither listed for this account nor a Space added here: not
            // one it may reach (a share that was revoked).
            let id = format!("relay:{machine}");
            if self.registry().get(&id).ok().flatten().is_none() {
                return Err(Error::NotFound(format!(
                    "{id} (not one of your Spaces, nor shared with you)"
                )));
            }
            return Ok(ThreadHome::Here(None));
        };
        let Some(host) = row.host.clone().filter(|h| !h.is_empty()) else {
            return Ok(ThreadHome::Here(None));
        };
        let space = format!("relay:{machine}");
        let Some(host_row) = self.relay_row(&host).await else {
            // The Space is shared with this account, its host is not: the
            // relay grants that Space's thread alone.
            let name = if row.name.is_empty() {
                space.clone()
            } else {
                row.name.clone()
            };
            return Ok(ThreadHome::Owner {
                host: Box::new(crate::relay::RelayMachine {
                    id: host,
                    name: format!("the computer that provides {name}"),
                    online: true,
                    sharing: true,
                    ..Default::default()
                }),
                space,
                granted: true,
            });
        };
        Ok(ThreadHome::Owner {
            host: Box::new(host_row),
            space,
            granted: false,
        })
    }

    /// Runs thread or agent member tool `tool` on the daemon of `host`
    /// (which provides `space`), as this device's signed-in account. A
    /// `thread_read` long poll is several relayed waits of at most
    /// [`FOLLOW_CHUNK_MS`]. Tool errors (a refusal) come back as the
    /// owner's results.
    pub async fn call_thread_owner(
        &self,
        host: &crate::relay::RelayMachine,
        space: &str,
        tool: &str,
        granted: bool,
        mut args: Value,
    ) -> Result<ToolOutcome> {
        let label = if host.name.is_empty() {
            host.id.clone()
        } else {
            host.name.clone()
        };
        let client = if granted {
            self.thread_grant_client(&host.id, space, &label).await?
        } else {
            self.thread_host_space(host, &label)
                .await?
                .spacesd()?
                .clone()
        };
        let client = &client;
        let follow = (tool == "thread_read")
            .then(|| args.get("follow_ms").and_then(Value::as_u64))
            .flatten()
            .filter(|ms| *ms > 0 && args.get("after").is_some_and(|a| !a.is_null()))
            .map(|ms| ms.min(MAX_FOLLOW_MS));
        let Some(total) = follow else {
            return call_once(client, space, tool, &args, CALL_TIMEOUT, &label).await;
        };
        let deadline = tokio::time::Instant::now() + Duration::from_millis(total);
        loop {
            let left = deadline
                .saturating_duration_since(tokio::time::Instant::now())
                .as_millis() as u64;
            let chunk = left.min(FOLLOW_CHUNK_MS);
            args["follow_ms"] = json!(chunk);
            let out = call_once(
                client,
                space,
                tool,
                &args,
                CALL_TIMEOUT + Duration::from_millis(chunk),
                &label,
            )
            .await?;
            if out.is_error || chunk >= left || has_entries(&out) {
                return Ok(out);
            }
        }
    }

    /// A connection to machine `host` (named `label`) through the relay
    /// for the thread of `space` alone, a Space it provides that is shared
    /// with this account ([`THREAD_SPACE_HEADER`]): the relay grants that
    /// thread and nothing else of the machine.
    async fn thread_grant_client(
        &self,
        host: &str,
        space: &str,
        label: &str,
    ) -> Result<cua_spacesd_client::SpacesdClient> {
        let relay = self.relay()?;
        let url = cua_host::RelayClient::new(&relay.url)?.machine_url(host);
        let options = cua_spacesd_client::ConnectOptions::parse(&url)?
            .gateway(std::sync::Arc::new(crate::relay::RelayBearer {
                account: relay,
            }))
            .header(
                THREAD_SPACE_HEADER,
                space.strip_prefix("relay:").unwrap_or(space),
            );
        let client = cua_spacesd_client::SpacesdClient::connect(options)
            .await
            .map_err(|e| grant_refused(e, space, label))?;
        let caps = match client.capabilities().await {
            Ok(caps) => caps,
            Err(e) => return Err(grant_refused(e, space, label)),
        };
        if !caps
            .features
            .iter()
            .any(|f| f.name == HOST_THREAD_FEATURE && f.supported)
        {
            return Err(Error::HostTooOld(owner_too_old(label)));
        }
        Ok(client)
    }

    /// The Space of machine `host` (named `label`) to call its
    /// `HostThreadService`: online, sharing, and new enough. A view-only
    /// share of it is enough (it reads).
    async fn thread_host_space(
        &self,
        host: &crate::relay::RelayMachine,
        label: &str,
    ) -> Result<crate::Space> {
        if !host.online {
            return Err(Error::host(
                "space thread",
                format!(
                    "{label} is offline, and this Space's thread lives there; try again when it \
                     is back"
                ),
            ));
        }
        if !host.sharing {
            return Err(crate::host_spaces::stopped_sharing(label));
        }
        let id = format!("relay:{}", host.id);
        let serves = |s: &crate::Space| {
            s.capabilities()
                .features
                .iter()
                .any(|f| f.name == HOST_THREAD_FEATURE && f.supported)
        };
        let s = self.space(&id).await?;
        if serves(&s) {
            return Ok(s);
        }
        // Capabilities may be cached from before the host was updated:
        // check once more.
        self.forget_connection(&id).await?;
        let s = self.space(&id).await?;
        if serves(&s) {
            return Ok(s);
        }
        let provides = s
            .capabilities()
            .features
            .iter()
            .any(|f| f.name == crate::host_spaces::HOST_SPACES_FEATURE && f.supported);
        let reports = s
            .capabilities()
            .features
            .iter()
            .any(|f| f.name == HOST_THREAD_FEATURE);
        if !provides && reports {
            return Err(Error::host(
                "space thread",
                format!(
                    "{label} no longer provides Spaces, so it serves no Space thread (on it: \
                     `cua host config --provide-spaces on`)"
                ),
            ));
        }
        Err(Error::HostTooOld(owner_too_old(label)))
    }
}

impl crate::host_spaces::HostSpacesServer {
    /// `HostThreadService.CallThreadTool`: thread or agent member tool
    /// `tool` on `space` (a Space this machine provides, by its relay
    /// machine or its id here), run as `caller` (the caller cua-spacesd
    /// verified). Only [`routed`] tools; another account reaches only the
    /// Spaces this machine provides. What `caller` may do there is the
    /// thread's own policy (roles and each agent's rights).
    pub async fn call_thread_tool(
        &self,
        caller: &crate::host_spaces::HostCaller,
        space: &str,
        tool: &str,
        arguments_json: &str,
    ) -> Result<ToolOutcome> {
        let tool = cua_spaces_contract::canonical(tool);
        let mut args: Value = if arguments_json.trim().is_empty() {
            json!({})
        } else {
            // Never quote the arguments: they carry messages.
            serde_json::from_str(arguments_json)
                .map_err(|_| Error::invalid("arguments_json is not JSON"))?
        };
        if !args.is_object() {
            return Err(Error::invalid("arguments_json is not a JSON object"));
        }
        if !routed(tool, &args) {
            return Err(Error::Relay(cua_host::Error::PermissionDenied(format!(
                "{tool} can't be called from another device; only a Space's thread and agent \
                 member tools can"
            ))));
        }
        let id = space.trim();
        let machine = id.strip_prefix("relay:").unwrap_or(id);
        // A Space shared with the caller (not this machine): that Space's
        // thread only.
        if let Some(granted) = caller.space.as_deref()
            && machine != granted.strip_prefix("relay:").unwrap_or(granted)
        {
            return Err(Error::Relay(cua_host::Error::PermissionDenied(format!(
                "{id}: a Space shared with you reaches only its own thread here (relay:{granted})"
            ))));
        }
        let host_dir = cua_host::Host::new(self.spaces().home_dir())
            .paths()
            .dir
            .clone();
        let provided = cua_host::provided::load_provided(&host_dir)?;
        let here = match provided.iter().find(|p| {
            !machine.is_empty()
                && (p.relay_machine == machine || (caller.space.is_none() && p.local_space == id))
        }) {
            Some(p) => p.local_space.clone(),
            // The owner, from another of their devices: any of their Spaces.
            None if caller.is_owner() && !id.is_empty() => id.to_string(),
            None => {
                return Err(Error::NotFound(format!(
                    "{id} (a Space this machine provides)"
                )));
            }
        };
        args["space"] = json!(here);
        let as_caller = crate::extension::Caller::from_host(caller);
        Ok(as_caller
            .scope(crate::mcp::McpServer::new(self.spaces().clone()).call(tool, args))
            .await)
    }
}

/// `ErrorInfo.metadata["kind"]` of a refused Space thread grant, from the
/// relay or the machine (`cua_relay::server::THREAD_GRANT_REFUSED`,
/// `cua_spacesd_server::auth::THREAD_GRANT_REFUSED`).
pub const THREAD_GRANT_REFUSED: &str = "thread_grant_refused";

/// What a refused thread grant means, for `space` on `host` (named
/// `label`). A relay or machine that knows thread grants types its refusal
/// ([`THREAD_GRANT_REFUSED`]): its message stands. Older ones are told by
/// their text (CUA-2300 removes this): a machine whose Cua predates thread
/// grants refuses the relay's assertion (`not allowed on this computer`),
/// a relay that predates them answers that the machine is not shared.
fn grant_refused(e: cua_spacesd_client::Error, space: &str, label: &str) -> Error {
    use cua_spacesd_client::Error as E;
    if let E::PermissionDenied(d) = &e
        && d.metadata.get("kind").map(String::as_str) == Some(THREAD_GRANT_REFUSED)
    {
        return Error::Relay(cua_host::Error::PermissionDenied(d.message.clone()));
    }
    let text = e.to_string();
    if text.contains("relay assertion refused") && text.contains("is not allowed on this computer")
    {
        return Error::HostTooOld(owner_too_old(label));
    }
    if text.contains("this computer is not shared with your account") {
        return Error::Relay(cua_host::Error::PermissionDenied(format!(
            "{space}'s thread lives on {label}, which is not shared with you, and the relay \
             does not grant a Space's thread alone; ask the owner to share that computer with \
             you (Can view is enough to read the thread)"
        )));
    }
    crate::host_spaces::from_host(e)
}

/// One relayed call.
async fn call_once(
    client: &cua_spacesd_client::SpacesdClient,
    space: &str,
    tool: &str,
    args: &Value,
    timeout: Duration,
    label: &str,
) -> Result<ToolOutcome> {
    let mut req = tonic::Request::new(pb::CallThreadToolRequest {
        space: space.to_string(),
        name: tool.to_string(),
        arguments_json: args.to_string(),
    });
    req.set_timeout(timeout);
    let r = match client.host_thread().call_thread_tool(req).await {
        Ok(r) => r.into_inner(),
        Err(status) => {
            let e = cua_spacesd_client::Error::from(status);
            if cua_spacesd_client::old_host::is_unknown_rpc(&e) {
                return Err(Error::HostTooOld(owner_too_old(label)));
            }
            return Err(crate::host_spaces::from_host(e));
        }
    };
    let parse = |s: &str| -> Result<Option<Value>> {
        if s.trim().is_empty() {
            return Ok(None);
        }
        serde_json::from_str(s)
            .map(Some)
            .map_err(|_| Error::host("space thread", format!("{label} answered a bad result")))
    };
    let content = match parse(&r.content_json)? {
        Some(Value::Array(parts)) => parts,
        Some(_) => {
            return Err(Error::host(
                "space thread",
                format!("{label} answered a bad result"),
            ));
        }
        None => vec![],
    };
    Ok(ToolOutcome {
        content,
        structured: parse(&r.structured_json)?,
        is_error: r.is_error,
        meta: parse(&r.meta_json)?,
    })
}

/// Whether a `thread_read` result holds entries.
fn has_entries(out: &ToolOutcome) -> bool {
    let v = out.structured.clone().or_else(|| {
        out.first_text()
            .and_then(|t| serde_json::from_str::<Value>(t).ok())
    });
    v.and_then(|v| {
        v.get("entries")
            .and_then(Value::as_array)
            .map(|e| !e.is_empty())
    })
    .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_the_thread_tools_and_member_calls_by_name() {
        for t in TOOLS {
            assert!(routed(t, &json!({"space": "relay:m"})), "{t}");
        }
        assert!(routed("agent_stop", &json!({"space": "s", "name": "ada"})));
        assert!(!routed("agent_stop", &json!({"space": "s", "run_id": "r"})));
        assert!(!routed("agent_stop", &json!({"space": "s", "name": " "})));
        assert!(!routed("space_bash", &json!({"space": "s"})));
        assert!(!routed("thread_import", &json!({"space": "s"})));
    }

    #[cfg(feature = "spaces-agents")]
    #[test]
    fn member_tools_are_the_run_tools_that_take_a_name() {
        assert_eq!(MEMBER_TOOLS, crate::agents::MEMBER_RUN_TOOLS);
    }

    /// The relay and the machine type a refused thread grant with this.
    #[test]
    fn the_thread_grant_refusal_kind_is_pinned() {
        assert_eq!(THREAD_GRANT_REFUSED, "thread_grant_refused");
    }

    /// The relay reads this header (cua-relay's `THREAD_SPACE_HEADER`,
    /// pinned to the same name in cua-spacesd-server's tests).
    #[test]
    fn the_thread_space_header_is_pinned() {
        assert_eq!(THREAD_SPACE_HEADER, "x-cua-thread-space");
    }

    #[test]
    fn a_read_with_no_entries_waits_on() {
        let empty = ToolOutcome::json(&json!({"entries": [], "cursor": 4}));
        assert!(!has_entries(&empty));
        let some = ToolOutcome::json(&json!({"entries": [{"seq": 5}], "cursor": 5}));
        assert!(has_entries(&some));
        assert!(has_entries(&ToolOutcome::text("not json")));
    }
}
