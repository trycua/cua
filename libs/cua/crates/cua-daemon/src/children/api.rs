//! The API a Space reaches on its spawn channel: `SpaceService` and
//! `DaemonService` of the cua daemon, so `cua`, the SDKs and the agent's
//! tools work in a Space unchanged, but answered for this Space's own
//! subtree and nothing else.
//!
//! - Every call names Spaces inside the caller's subtree. A sibling, an
//!   ancestor, another tree or a guessed id is "not found", word for word
//!   what a missing Space gets.
//! - A create or fork makes a child of the caller: one level deeper (up to
//!   its tree's limit), on this computer, with the caller's egress rules,
//!   and a lifetime no longer than the caller's (4 hours by default).
//! - Everything else (other computers, clouds, volumes, shares, the
//!   daemon's own token and settings) is refused.
//!
//! The caller is the Space whose channel the request came in on (the host
//! opened it), and the capability it presents must be that Space's live
//! one ([`super::capability`]).

use std::sync::Arc;
use std::time::Duration;

use cua_proto::daemon::v1::{
    self as pb, daemon_service_server::DaemonService, space_service_server::SpaceService,
};
use serde_json::Value;
use tonic::{Request, Response, Status};

use super::capability::Claims;
use super::tree;
use crate::server::{Shared, Svc};
use cua_sandbox_core::placement::On;

type R<T> = std::result::Result<Response<T>, Status>;

/// The label that mirrors a child's parent. The record's tree fields
/// (`SpaceMeta.parent`, `root`, `depth`) are what counts on this computer.
pub const LABEL_PARENT: &str = cua_spaces::tree::PARENT_LABEL;
/// The root of the tree, for a child placed on another computer or a cloud
/// (whose registry does not hold its parent).
pub const LABEL_ROOT: &str = "cua.root";
/// The depth, for a child placed elsewhere (0 for a Space a person made).
pub const LABEL_DEPTH: &str = "cua.depth";

/// A child's lifetime when its parent has none.
pub const DEFAULT_CHILD_LIFETIME: Duration = Duration::from_secs(4 * 3600);

/// The one answer for a Space outside the caller's subtree.
pub fn not_found(query: &str) -> Status {
    Status::not_found(format!("no Space {query:?} among this Space's children"))
}

fn refused(what: &str) -> Status {
    Status::permission_denied(format!(
        "a Space can only {what}; ask the owner to do this outside the Space"
    ))
}

fn deny<T>() -> R<T> {
    Err(refused("create, list and manage its own children"))
}

/// `(power, power_state)` as an agent in a Space sees them. `power` is how the Space turns off (`stop` or `suspend`);
/// the guest tools and the `cua spaces ls` an image shipped before 0.14 print it as the Space's state, so a running
/// child read "stop" and agents said "the Space shows as stopped" about a Space they were using. On this channel
/// `power` therefore carries the state itself (`running`, `stopped`, `suspended`); `power_state` is unchanged.
pub(crate) fn state_for_agents((power, state): (String, String)) -> (String, String) {
    if state.is_empty() {
        (power, state)
    } else {
        (state.clone(), state)
    }
}

/// The scoped services of one Space's channel.
#[derive(Clone)]
pub struct Scoped {
    pub(crate) shared: Arc<Shared>,
    /// The Space the channel belongs to.
    pub(crate) space: String,
}

/// The verified capability of a request (put there by the channel's auth).
fn claims<T>(req: &Request<T>) -> Result<Claims, Status> {
    req.extensions()
        .get::<Claims>()
        .cloned()
        .ok_or_else(|| Status::unauthenticated(super::capability::Refusal::Malformed.message()))
}

impl Scoped {
    fn owner(&self) -> Svc {
        Svc(self.shared.clone())
    }

    fn spaces(&self) -> &cua_spaces::Spaces {
        self.shared.runtime.spaces()
    }

    /// One line of the hash-chained audit log of what Spaces ask of their
    /// children API (`<home>/children-audit.jsonl`): who, what, which Space.
    /// A log that cannot be written never blocks the request.
    fn audit(&self, action: &str, target: &str, detail: &str) {
        let log =
            cua_spaces::audit::AuditLog::new(self.spaces().home_dir().join("children-audit.jsonl"));
        if let Err(e) = log.append(&format!("space:{}", self.space), action, target, detail) {
            tracing::warn!(error = %e, "could not write the children audit log");
        }
    }

    /// Parent links of every registered Space.
    pub(crate) fn links(&self) -> tree::Links {
        links_of(self.spaces())
    }

    /// The registered records below the caller.
    fn subtree_records(&self) -> Result<Vec<pb::Space>, Status> {
        let links = self.links();
        let below: std::collections::BTreeSet<String> =
            tree::subtree(&links, &self.space).into_iter().collect();
        let mut records: Vec<pb::Space> = self
            .spaces()
            .registry()
            .list()
            .map_err(|e| Status::internal(e.to_string()))?
            .into_iter()
            .filter(|r| below.contains(&r.id))
            .collect();
        self.fill(&mut records);
        Ok(records)
    }

    /// Power, stop reason and the rest of a record's live fields.
    fn fill(&self, records: &mut [pb::Space]) {
        for r in records.iter_mut() {
            (r.power, r.power_state) = state_for_agents(self.spaces().power_of(&r.id));
            r.stop_reason = self.spaces().stop_reason_of(&r.id);
        }
        self.spaces().fill_records(records);
        crate::spaces_svc::imp::mark_children(self.spaces(), records);
    }

    /// What an agent home reaches beyond its tree (none for any other Space).
    fn home_scope(&self) -> super::home::HomeScope {
        // What the home was made with, plus what the person approved since.
        self.own_meta()
            .map(|m| super::home::HomeScope::from_labels(&m.labels))
            .unwrap_or_default()
            .with_grants(super::home::read_grants(
                self.spaces().home_dir(),
                &self.space,
            ))
    }

    /// The Spaces outside the caller's subtree that an agent home may see:
    /// the ones the person @mentioned, or all once the person approved
    /// listing all. Read only: they are never resolved by the calls that
    /// change or connect to a Space.
    fn granted_records(&self, seen: &[pb::Space]) -> Result<Vec<pb::Space>, Status> {
        let scope = self.home_scope();
        if !scope.is_home {
            return Ok(Vec::new());
        }
        let mut records: Vec<pb::Space> = self
            .spaces()
            .registry()
            .list()
            .map_err(|e| Status::internal(e.to_string()))?
            .into_iter()
            .filter(|r| {
                !seen.iter().any(|s| s.id == r.id)
                    && !super::home::is_warming_home(&r.labels)
                    && scope.may_list(&r.id, &r.name)
            })
            .collect();
        self.fill(&mut records);
        Ok(records)
    }

    /// The Space `query` names among the caller's descendants: by id, then
    /// by name. Nothing outside the subtree is ever matched.
    pub(crate) fn resolve(&self, query: &str) -> Result<pb::Space, Status> {
        let q = query.trim();
        if q.is_empty() {
            return Err(Status::invalid_argument("name a Space"));
        }
        let records = self.subtree_records()?;
        if let Some(r) = records.iter().find(|r| r.id == q) {
            return Ok(r.clone());
        }
        let named: Vec<&pb::Space> = records
            .iter()
            .filter(|r| r.name == q || (!r.display_name.is_empty() && r.display_name == q))
            .collect();
        match named.as_slice() {
            [one] => Ok((*one).clone()),
            [] => Err(not_found(q)),
            many => Err(Status::failed_precondition(format!(
                "{q:?} names {} children: use an id ({})",
                many.len(),
                many.iter()
                    .map(|r| r.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))),
        }
    }

    /// One request to the account's pool, from a Space of the tree under
    /// `root`: `cua run`, `cua jobs` and `cua computers` inside the Space.
    /// What the Space submits is recorded under it ([`super::runs`]); it sees,
    /// reads and cancels only its own tree's runs; the pool's other methods
    /// are refused.
    async fn scoped_pool_request(
        &self,
        root: &str,
        max_depth: u32,
        mut r: pb::PoolRequestRequest,
    ) -> R<pb::PoolRequestResponse> {
        use serde_json::json;
        let home = self.spaces().home_dir().to_path_buf();
        let mut params: Value = if r.params_json.trim().is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&r.params_json)
                .map_err(|e| Status::invalid_argument(format!("params_json: {e}")))?
        };
        let run_of = |p: &Value| p["run"].as_str().unwrap_or_default().to_string();
        let own = |run: &str| super::runs::Ledger::load(&home).visible(run, root);
        let method = r.method.clone();
        use super::runs::PoolOp;
        match super::runs::pool_op(method.as_str()) {
            // The computers and the queue: read only.
            PoolOp::Read => {}
            PoolOp::Submit => {
                let mut run = run_of(&params);
                if run.trim().is_empty() {
                    run = format!("p-{:08x}", rand::random::<u32>());
                    params["run"] = json!(run);
                }
                // The job's Space is a child of this one wherever it runs
                // ([`cua_cluster::model::ENV_PARENT_SPACE`]).
                let (_, depth) = position(self.spaces(), &self.space);
                if depth.saturating_add(1) > max_depth {
                    return Err(Status::resource_exhausted(format!(
                        "this Space is {depth} level(s) below its tree's root and the tree allows                          {max_depth}: a run is a child Space, so it cannot start one"
                    )));
                }
                if let Err(why) = super::runs::confine_submit(
                    &mut params,
                    &run,
                    &self.space,
                    root,
                    depth,
                    &self.own_network(),
                ) {
                    self.audit("refuse", &run, &why);
                    return Err(refused(&why));
                }
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let (space, root) = (self.space.clone(), root.to_string());
                let (jname, jgpu) = super::runs::job_facts(&params);
                let ok = super::runs::Ledger::update(&home, |l| {
                    let ok = l.claim(&run, &space, &root, now);
                    if ok {
                        l.describe(&run, &jname, jgpu);
                    }
                    ok
                });
                if !ok {
                    return Err(not_found(&run));
                }
                self.audit("pool-submit", &run, "a run submitted from a Space");
            }
            PoolOp::List => {}
            PoolOp::OwnRun => {
                let run = run_of(&params);
                if !own(&run) {
                    return Err(not_found(&run));
                }
            }
            PoolOp::OwnJob => {
                // A job id names a run only by asking for the job first.
                let probe = self
                    .owner()
                    .pool_request(Request::new(pb::PoolRequestRequest {
                        target: r.target.clone(),
                        method: "job".into(),
                        params_json: params.to_string(),
                    }))
                    .await?
                    .into_inner();
                let body: Value = serde_json::from_str(&probe.body_json).unwrap_or(Value::Null);
                let run = body["run"]
                    .as_str()
                    .or_else(|| body["run_id"].as_str())
                    .unwrap_or_default();
                if probe.status == 200 && !own(run) {
                    return Err(not_found(&params["id"].to_string()));
                }
            }
            PoolOp::Refused => return deny(),
        }
        r.params_json = params.to_string();
        let out = self.owner().pool_request(Request::new(r)).await?;
        if method != "runs" {
            return Ok(out);
        }
        // The list holds this tree's runs only.
        let mut resp = out.into_inner();
        if resp.status == 200 {
            let mut body: Value = serde_json::from_str(&resp.body_json).unwrap_or(Value::Null);
            if let Some(list) = body["runs"].as_array_mut() {
                let ledger = super::runs::Ledger::load(&home);
                list.retain(|v| ledger.visible(v["run"].as_str().unwrap_or_default(), root));
            }
            resp.body_json = body.to_string();
        }
        Ok(Response::new(resp))
    }

    /// The caller's own record (its egress, lifetime).
    fn own_meta(&self) -> Option<cua_spaces::registry::SpaceMeta> {
        self.spaces().registry().meta(&self.space)
    }

    /// The request a create of a child becomes: refused when it asks for
    /// more than a child may have, otherwise placed under the caller.
    pub(crate) fn child_request(
        &self,
        claims: &Claims,
        mut r: pb::CreateSpaceRequest,
    ) -> Result<pb::CreateSpaceRequest, Status> {
        let depth = claims.depth.saturating_add(1);
        if depth > claims.max_depth {
            return Err(Status::resource_exhausted(format!(
                "this Space is {} level(s) below its tree's root and the tree allows {}: \
                 it cannot make children",
                claims.depth, claims.max_depth
            )));
        }
        // Whether it may make children of its own is the tree's depth's
        // call ([`Scoped::adopt`]), never the request's.
        r.children = None;
        // Its place in the tree is this Space's, and its network stays
        // within this Space's: neither is the request's to say.
        if r.widen {
            return Err(refused(
                "make children whose network is no more open than its own",
            ));
        }
        if !r.parent.trim().is_empty() && r.parent != self.space {
            return Err(Status::invalid_argument(
                "a child's parent is the Space that makes it; leave parent empty",
            ));
        }
        // Where: this computer (the default), the least-loaded of the
        // owner's computers and linked clouds (`auto`), one of the owner's
        // machines (`host:<m>`), or a linked cloud. This daemon creates it
        // with the owner's own sign-in and cloud links; the Space asking
        // never holds them.
        let location = r.location.trim().to_owned();
        let on = if location.is_empty() {
            On::Local
        } else {
            On::parse_or_host(&location).map_err(|e| Status::invalid_argument(e.message))?
        };
        match &on {
            On::Local | On::Auto | On::Host(_) | On::Provider(_) => {}
            On::Cloud | On::Direct(_) | On::Relay(_) => {
                return Err(Status::failed_precondition(format!(
                    "a child goes on this computer, `auto`, one of your computers \
                     (`host:<name>`) or a linked cloud; on={location:?} is not one"
                )));
            }
        }
        // An agent home puts children on the person's computers only where
        // the person said so (an approval, or an @mention).
        if let On::Host(machine) = &on {
            let scope = self.home_scope();
            if scope.is_home && !scope.may_use_machine(machine) {
                return Err(refused(&format!(
                    "use the computer {machine:?} once the person lets this agent use it \
                     (ask them, or have them @mention it); for now it can make children here or with `auto`"
                )));
            }
        }
        r.location = on.to_string();
        let local = matches!(on, On::Local);
        if r.reuse {
            return Err(Status::invalid_argument(
                "a child is always a new Space (reuse is not available here)",
            ));
        }
        if r.durable {
            return Err(Status::invalid_argument(
                "a child lives as long as its parent allows (durable is not available here)",
            ));
        }
        if !r.volumes.is_empty() {
            return Err(refused(
                "mount volumes in children that it was given itself",
            ));
        }
        if local && (!r.instance_type.is_empty() || !r.region.is_empty()) {
            return Err(Status::failed_precondition(
                "a machine type or region needs on=auto or a linked cloud",
            ));
        }
        if let Some(k) = r.labels.keys().find(|k| k.starts_with("cua.")) {
            return Err(Status::invalid_argument(format!(
                "label {k:?} is reserved (labels starting with \"cua.\" are set by Cua)"
            )));
        }
        // Egress: the parent's rules, never wider.
        let own = self.own_meta().map(|m| m.egress).unwrap_or_default();
        let asked = r.egress.take().map(|e| cua_spaces::egress::EgressPolicy {
            allow: e.allow,
            deny: e.deny,
        });
        let egress = match asked {
            None => own,
            Some(a) if a.is_empty() => own,
            Some(a) if own.is_empty() || a == own => a,
            Some(_) => {
                return Err(Status::permission_denied(
                    "a child keeps its parent's egress rules; leave egress empty",
                ));
            }
        };
        if !egress.is_empty() {
            r.egress = Some(pb::EgressPolicy {
                allow: egress.allow,
                deny: egress.deny,
            });
        }
        if r.name.trim().is_empty() {
            r.name = self.next_child_name("c");
        }
        if local {
            // This computer records it in the caller's tree: the record's
            // parent, root and depth, its network no more open than the
            // caller's, the tree's own network.
            r.parent = self.space.clone();
            self.allow_depth(claims);
        } else {
            // Another computer or a cloud does not hold the caller's record:
            // the network goes with the request, cut down to the caller's,
            // and labels carry the tree.
            let asked = match r.network_settings.as_ref() {
                Some(n) => cua_spaces::network::from_pb(n)
                    .map_err(|e| Status::invalid_argument(e.to_string()))?,
                None => cua_spaces::network::NetworkSettings::internet(),
            };
            let own = self.own_network();
            r.network_settings = Some(cua_spaces::network::to_pb(&asked.intersect(&own)));
            r.labels.insert(LABEL_PARENT.into(), self.space.clone());
            r.labels.insert(LABEL_ROOT.into(), claims.root.clone());
            r.labels.insert(LABEL_DEPTH.into(), depth.to_string());
        }
        Ok(r)
    }

    /// The caller's network settings (or what it would move to).
    fn own_network(&self) -> cua_spaces::network::NetworkSettings {
        match self.own_meta() {
            Some(m) => m
                .network
                .clone()
                .unwrap_or_else(|| cua_spaces::network::migrate_meta(&m, false)),
            None => cua_spaces::network::NetworkSettings::internet(),
        }
    }

    /// The tree's depth limit is at least what the owner granted the
    /// caller's capability, so the record's own limit does not refuse a
    /// child the capability allows.
    fn allow_depth(&self, claims: &Claims) {
        let want = claims.max_depth.min(cua_spaces::tree::MAX_DEPTH);
        let limit =
            cua_spaces::tree::depth_limit(self.spaces().registry().meta(&claims.root).as_ref());
        if limit < want {
            let _ = self
                .spaces()
                .registry()
                .update_meta(&claims.root, |m| m.depth_limit = Some(want));
        }
    }

    /// `<parent name>.<prefix><n>`, the first one not taken.
    fn next_child_name(&self, prefix: &str) -> String {
        let base = self
            .space
            .split_once(':')
            .map(|(_, n)| n)
            .unwrap_or(&self.space)
            .to_owned();
        let taken: std::collections::BTreeSet<String> = self
            .spaces()
            .registry()
            .list()
            .unwrap_or_default()
            .into_iter()
            .map(|r| r.name)
            .collect();
        (1..)
            .map(|n| format!("{base}.{prefix}{n}"))
            .find(|n| !taken.contains(n))
            .expect("an unused name")
    }

    /// After a child exists: its lifetime (no longer than the caller's) and,
    /// when the tree allows another level, its own children turned on.
    pub(crate) fn adopt(&self, claims: &Claims, child: &str) {
        let spaces = self.spaces();
        let remaining = spaces.lifetime(&self.space).map(|l| {
            l.expires_at()
                .duration_since(std::time::SystemTime::now())
                .unwrap_or_default()
        });
        let life = remaining
            .map(|r| r.min(DEFAULT_CHILD_LIFETIME))
            .unwrap_or(DEFAULT_CHILD_LIFETIME)
            .max(Duration::from_secs(60));
        if let Err(e) =
            spaces.set_lifetime(child, life, Some(cua_sandbox_core::LocalExpiry::Delete))
        {
            tracing::warn!(child, error = %e, "could not set a child's lifetime");
        }
        if claims.depth.saturating_add(1) < claims.max_depth
            && let Err(e) = spaces.set_children(child, true)
        {
            tracing::warn!(child, error = %e, "could not turn on a child's own children");
        }
    }

    /// Deletes `id` and everything below it, deepest first.
    async fn delete_tree(&self, id: &str) -> Result<String, Status> {
        let links = self.links();
        let mut order = tree::subtree(&links, id);
        order.push(id.to_owned());
        let mut last = String::new();
        for victim in order {
            let r = self
                .owner()
                .delete_space(Request::new(pb::DeleteSpaceRequest {
                    space: victim.clone(),
                    keep_children: false,
                }))
                .await;
            if let Some(a) = self.shared.spawn_authority() {
                a.revoke(&victim);
            }
            let _ = self.spaces().set_children(&victim, false);
            match r {
                Ok(m) => {
                    self.audit("delete", &victim, "");
                    last = m.into_inner().message
                }
                Err(s) if s.code() == tonic::Code::NotFound => {}
                Err(s) => return Err(s),
            }
        }
        Ok(last)
    }

    /// The tools a Space may call on its children, and the argument that
    /// names the child.
    fn tool_target(name: &str) -> Option<&'static str> {
        Some(match name {
            "stop_space" | "start_space" | "space_bash" | "space_write" | "upload" | "download"
            | "list_tools" | "call_tool" | "list_space_windows" => "space",
            // What the person sees of the Space (its own too): a window or the
            // whole desktop on their screen.
            "stream_space_window" | "show_space_pip" | "hide_space_pip" | "open_space_viewer" => {
                "space"
            }
            _ => return None,
        })
    }

    /// Whether the tool shows a Space on the person's desktop: it may name the
    /// caller itself, not only its children.
    fn shows_on_desktop(name: &str) -> bool {
        matches!(
            name,
            "stream_space_window"
                | "show_space_pip"
                | "hide_space_pip"
                | "open_space_viewer"
                | "list_space_windows"
        )
    }

    /// Rewrites a tool call's Space argument to the child's id, refusing
    /// tools that are not a child's and Spaces outside the subtree.
    pub(crate) fn scoped_tool_args(&self, name: &str, args: &mut Value) -> Result<(), Status> {
        let Some(key) = Self::tool_target(name) else {
            return Err(refused("use its children's own tools"));
        };
        let own_name = self
            .space
            .split_once(':')
            .map(|(_, n)| n.to_owned())
            .unwrap_or_default();
        let q = match args.get(key).and_then(Value::as_str) {
            Some(q) => q.to_owned(),
            // The window or desktop of the Space asking, when it names none.
            None if Self::shows_on_desktop(name) => self.space.clone(),
            None => {
                return Err(Status::invalid_argument(format!("{name} needs `{key}`")));
            }
        };
        if Self::shows_on_desktop(name)
            && (q == self.space || q == "self" || (!own_name.is_empty() && q == own_name))
        {
            args[key] = Value::String(self.space.clone());
            return Ok(());
        }
        let child = self.resolve(&q)?;
        args[key] = Value::String(child.id);
        // A window it shows goes into the Stream section of the Space that
        // asks, never another Space's or the person's desktop.
        pin_stream_target(name, args, &self.space);
        Ok(())
    }
}

/// A Space shows a child's window only in its own Stream section: whatever
/// `into` the agent passed (another Space, `desktop`) is replaced.
fn pin_stream_target(name: &str, args: &mut Value, caller: &str) {
    if name == "stream_space_window" {
        args["into"] = Value::String(caller.to_string());
    }
}

/// What a create of a child asks of its tree's budget (a child on this
/// machine costs nothing per hour).
/// A create refused for a reason the agent can do nothing about, in words it can pass on to the
/// person: what is missing, and that retrying from inside the Space will not help. The host's own
/// wording talks to someone at a terminal (`cua cache prune`, a build script); an agent that
/// repeats it sends the person to commands they do not need.
/// What the thread shows of one create an agent asked for: recorded in
/// [`cua_spaces::child_creates`] as it goes, so every client draws the same row.
struct Tracked {
    home: std::path::PathBuf,
    /// This create's own record (two creates under one name never share one).
    id: String,
    /// Whether it ended in a result or an error. One that is dropped before either (its stream was
    /// cut) is failed, not left on "creating".
    done: std::sync::atomic::AtomicBool,
    os: &'static str,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

impl Tracked {
    fn start(home: &std::path::Path, parent: &str, child: &pb::CreateSpaceRequest) -> Tracked {
        let os = cua_spaces::child_creates::os_of_image(&child.image);
        Tracked {
            home: home.to_path_buf(),
            id: cua_spaces::child_creates::start(
                home,
                parent,
                &child.name,
                &child.name,
                os,
                now_ms(),
            ),
            done: std::sync::atomic::AtomicBool::new(false),
            os,
        }
    }

    fn set(&self, f: impl FnOnce(&mut cua_spaces::child_creates::Entry)) {
        cua_spaces::child_creates::update_id(&self.home, &self.id, f);
    }

    fn progress(&self, p: &pb::CreateSpaceProgress) {
        let words = cua_spaces::child_creates::phase_words(&p.phase, self.os);
        self.set(|e| {
            if !words.is_empty() {
                e.phase = words;
            }
            e.code = p.phase.clone();
            e.percent = p.fraction.map(|f| (f.clamp(0.0, 1.0) * 100.0) as u8);
            if !p.space.is_empty() {
                e.space = p.space.clone();
            }
        });
    }

    fn ready(&self, id: &str) {
        self.done.store(true, std::sync::atomic::Ordering::Relaxed);
        self.set(|e| {
            e.state = cua_spaces::child_creates::State::Ready;
            e.space = id.to_string();
            e.phase = "Ready".into();
            e.percent = None;
            e.ended_ms = Some(now_ms());
        });
    }

    fn failed(&self, why: &str) {
        self.done.store(true, std::sync::atomic::Ordering::Relaxed);
        let reason = cua_spaces::child_creates::person_reason(why);
        // A create the person cancelled is not a failure to show on a row.
        let cancelled = why.to_lowercase().contains("cancel");
        self.set(|e| {
            e.hidden = cancelled;
            e.state = cua_spaces::child_creates::State::Failed;
            e.error = reason;
            e.percent = None;
            e.ended_ms = Some(now_ms());
        });
    }
}

impl Drop for Tracked {
    fn drop(&mut self) {
        if !self.done.load(std::sync::atomic::Ordering::Relaxed) {
            // The stream was dropped (the agent or its connection went away) before the create ended: it
            // is cancelled with it, so its row says so instead of staying on "creating".
            self.failed("The create was interrupted before it finished");
        }
    }
}

pub(crate) fn for_the_agent(status: Status) -> Status {
    match agent_words(status.message()) {
        Some(m) => Status::new(status.code(), m),
        None => status,
    }
}

fn agent_words(msg: &str) -> Option<String> {
    let l = msg.to_lowercase();
    if l.contains("probe port") && l.contains("not published") {
        // The agent repeats what it is told to the person: no port numbers or internals.
        return Some(
            "The Space's network setup did not come up, so it was not made. Say that plainly; do not quote ports or internals. Offer to try a Linux Space instead, or to try this one again later."
                .into(),
        );
    }
    if l.contains("stopped while booting") {
        return Some(
            "The Space stopped while starting up, so it was not made. Say that plainly and offer to try again or to use a Linux Space instead."
                .into(),
        );
    }
    if l.contains("not enough disk space") {
        let between = |a: &str, b: &str| -> Option<String> {
            let from = msg.find(a)? + a.len();
            let rest = &msg[from..];
            Some(rest[..rest.find(b)?].trim().to_string())
        };
        let needed = between("needs about ", " and").unwrap_or_else(|| "more than is free".into());
        let free = between("but only ", " is available").unwrap_or_else(|| "too little".into());
        return Some(format!(
            "There is not enough free disk on the person's computer to make this computer: it needs about {needed} and only {free} is free. \
             Nothing in this Space can free it. Tell the person to free disk space on their computer (delete computers they no longer use in Cua Spaces, \
             or empty the Trash) and then try again. A Linux computer needs far less, if that can do the job."
        ));
    }
    if l.contains("cua-netgw") && (l.contains("not installed") || l.contains("ships next to")) {
        return Some(
            "This installation of Cua Spaces is missing its network gateway, which a child computer needs so its network stays inside its parent's limits. \
             That is a problem with the install, not with the request: tell the person to update Cua Spaces. Trying again will not help."
                .into(),
        );
    }
    None
}

fn ask_of(r: &pb::CreateSpaceRequest) -> super::budget::Ask {
    super::budget::Ask {
        vcpus: if r.cpus == 0 { 2 } else { r.cpus },
        memory_mb: if r.memory_mb == 0 { 4096 } else { r.memory_mb },
        usd_per_hour: 0.0,
    }
}

/// What a child placed off this computer costs per hour: what the elastic
/// pool would add to place it (0 when it fits on a machine already running).
/// A child the pool can't place is refused with the pool's reason.
async fn priced(
    spaces: &cua_spaces::Spaces,
    r: &pb::CreateSpaceRequest,
    mut ask: super::budget::Ask,
) -> Result<super::budget::Ask, Status> {
    if r.location == "local" {
        return Ok(ask);
    }
    let req = cua_spaces::elastic::ElasticRequest {
        image: r.image.clone(),
        kind: r.kind.clone(),
        cpus: Some(ask.vcpus),
        memory_mb: Some(ask.memory_mb),
        count: 1,
        ..Default::default()
    };
    match spaces.elastic_plan(&[req]).await {
        Ok(p) => {
            if let Some(u) = p.plan.unplaced.first() {
                return Err(Status::resource_exhausted(format!(
                    "no machine can take this child: {}",
                    u.reason
                )));
            }
            ask.usd_per_hour = p.plan.added_usd_per_hour.max(0.0);
            Ok(ask)
        }
        // Without a pool view (no linked cloud), it goes on a machine of
        // yours: free here; the create itself says if none can take it.
        Err(_) => Ok(ask),
    }
}

/// A tree's budget and use, as the API reports it.
pub(crate) fn tree_budget(shared: &Shared, root: &str) -> pb::TreeBudget {
    let Some(c) = shared.children.as_deref() else {
        return pb::TreeBudget {
            root: root.to_owned(),
            ..Default::default()
        };
    };
    let set = c.budget.budget(root);
    let l = set.effective();
    let used = super::tree_usage(shared, root);
    let reserved = c.budget.reserved(root);
    pb::TreeBudget {
        root: root.to_owned(),
        set: Some(pb::TreeBudgetLimits {
            max_spaces: set.max_spaces,
            max_vcpus: set.max_vcpus,
            max_memory_mb: set.max_memory_mb,
            usd_per_hour: set.usd_per_hour,
            usd_total: set.usd_total,
        }),
        max_spaces: l.max_spaces,
        max_vcpus: l.max_vcpus,
        max_memory_mb: l.max_memory_mb,
        usd_per_hour: l.usd_per_hour,
        usd_total: l.usd_total,
        spaces: used.spaces + reserved.spaces,
        vcpus: used.vcpus + reserved.vcpus,
        memory_mb: used.memory_mb + reserved.memory_mb,
        usd_per_hour_used: used.usd_per_hour + reserved.usd_per_hour,
        usd_spent: used.usd_spent,
    }
}

/// Parent links from the registry's records.
pub(crate) fn links_of(spaces: &cua_spaces::Spaces) -> tree::Links {
    let meta = spaces.registry().all_meta();
    let ids: Vec<String> = spaces
        .registry()
        .list()
        .unwrap_or_default()
        .into_iter()
        .map(|r| r.id)
        .collect();
    tree::Links(
        ids.into_iter()
            .map(|id| {
                let parent = meta.get(&id).and_then(parent_of);
                (id, parent)
            })
            .collect(),
    )
}

/// A record's parent: its tree field, else the label a child placed from
/// another computer carries.
pub(crate) fn parent_of(m: &cua_spaces::registry::SpaceMeta) -> Option<String> {
    m.parent
        .clone()
        .or_else(|| m.labels.get(LABEL_PARENT).cloned())
}

/// The tree position of `id`: (root, depth). A Space a person made is its
/// own root at depth 0.
pub(crate) fn position(spaces: &cua_spaces::Spaces, id: &str) -> (String, u32) {
    let meta = spaces.registry().meta(id);
    if let Some(m) = meta.as_ref().filter(|m| m.parent.is_some()) {
        let root = m
            .root
            .clone()
            .or_else(|| m.parent.clone())
            .unwrap_or_default();
        return (root, m.depth);
    }
    let label = |k: &str| meta.as_ref().and_then(|m| m.labels.get(k).cloned());
    match (
        label(LABEL_ROOT),
        label(LABEL_DEPTH).and_then(|d| d.parse().ok()),
    ) {
        (Some(root), Some(depth)) if label(LABEL_PARENT).is_some() => (root, depth),
        _ => (id.to_owned(), 0),
    }
}

#[tonic::async_trait]
impl SpaceService for Scoped {
    async fn add_space(&self, _: Request<pb::AddSpaceRequest>) -> R<pb::AddSpaceResponse> {
        deny()
    }

    async fn list_spaces(&self, _: Request<pb::ListSpacesRequest>) -> R<pb::ListSpacesResponse> {
        let mut spaces = self.subtree_records()?;
        let granted = self.granted_records(&spaces)?;
        spaces.extend(granted);
        Ok(Response::new(pb::ListSpacesResponse {
            spaces,
            ..Default::default()
        }))
    }

    async fn remove_space(&self, _: Request<pb::RemoveSpaceRequest>) -> R<pb::RemoveSpaceResponse> {
        deny()
    }

    async fn list_volumes(&self, _: Request<pb::ListVolumesRequest>) -> R<pb::ListVolumesResponse> {
        deny()
    }

    async fn create_volume(
        &self,
        _: Request<pb::CreateVolumeRequest>,
    ) -> R<pb::CreateVolumeResponse> {
        deny()
    }

    async fn delete_volume(
        &self,
        _: Request<pb::DeleteVolumeRequest>,
    ) -> R<pb::DeleteVolumeResponse> {
        deny()
    }

    async fn resolve_space(
        &self,
        req: Request<pb::ResolveSpaceRequest>,
    ) -> R<pb::ResolveSpaceResponse> {
        let space = self.resolve(&req.into_inner().space)?;
        Ok(Response::new(pb::ResolveSpaceResponse {
            space: Some(space),
        }))
    }

    async fn provision_local_space(
        &self,
        _: Request<pb::ProvisionLocalSpaceRequest>,
    ) -> R<pb::ProvisionLocalSpaceResponse> {
        deny()
    }

    async fn release_space(
        &self,
        _: Request<pb::ReleaseSpaceRequest>,
    ) -> R<pb::ReleaseSpaceResponse> {
        deny()
    }

    async fn create_space(
        &self,
        req: Request<pb::CreateSpaceRequest>,
    ) -> R<pb::CreateSpaceResponse> {
        let claims = claims(&req)?;
        let child = self.child_request(&claims, req.into_inner())?;
        let ask = priced(self.spaces(), &child, ask_of(&child)).await?;
        let name = child.name.clone();
        let _held = super::admit(&self.shared, &claims.root, ask).inspect_err(|e| {
            self.audit("refuse", &name, e.message());
        })?;
        let tracked = Tracked::start(self.spaces().home_dir(), &self.space, &child);
        let mut resp = match self.owner().create_space(Request::new(child)).await {
            Ok(r) => r,
            Err(e) => {
                tracked.failed(e.message());
                return Err(for_the_agent(e));
            }
        };
        if let Some(s) = resp.get_mut().space.as_mut() {
            // The result says how the Space is now (running or not), read now, not left to the registry's last word.
            (s.power, s.power_state) = state_for_agents(self.spaces().power_of(&s.id));
        }
        if let Some(s) = &resp.get_ref().space {
            tracked.ready(&s.id);
            self.audit("spawn", &s.id, &name);
            self.adopt(&claims, &s.id);
            super::record_cost(&self.shared, &claims.root, &s.id, ask);
        }
        Ok(resp)
    }

    type CreateSpaceStreamStream = <Svc as SpaceService>::CreateSpaceStreamStream;

    async fn create_space_stream(
        &self,
        req: Request<pb::CreateSpaceStreamRequest>,
    ) -> R<Self::CreateSpaceStreamStream> {
        use tokio_stream::StreamExt as _;
        let claims = claims(&req)?;
        let inner = req.into_inner();
        let child = self.child_request(&claims, inner.create.unwrap_or_default())?;
        let ask = priced(self.spaces(), &child, ask_of(&child)).await?;
        let held = super::admit(&self.shared, &claims.root, ask)?;
        let tracked = std::sync::Arc::new(Tracked::start(
            self.spaces().home_dir(),
            &self.space,
            &child,
        ));
        let stream = match self
            .owner()
            .create_space_stream(Request::new(pb::CreateSpaceStreamRequest {
                create: Some(child),
            }))
            .await
        {
            Ok(r) => r.into_inner(),
            Err(e) => {
                tracked.failed(e.message());
                return Err(for_the_agent(e));
            }
        };
        let me = self.clone();
        // The reservation lasts as long as the create's stream.
        let adopted = stream.map(move |item| {
            let _ = &held;
            match &item {
                Ok(pb::CreateSpaceStreamResponse {
                    event: Some(pb::create_space_stream_response::Event::Progress(p)),
                }) => tracked.progress(p),
                Err(e) => tracked.failed(e.message()),
                _ => {}
            }
            let mut item = item.map_err(for_the_agent);
            if let Ok(pb::CreateSpaceStreamResponse {
                event: Some(pb::create_space_stream_response::Event::Result(r)),
            }) = &mut item
                && let Some(s) = r.space.as_mut()
            {
                // The result says how the Space is now, read now.
                (s.power, s.power_state) = state_for_agents(me.spaces().power_of(&s.id));
            }
            if let Ok(pb::CreateSpaceStreamResponse {
                event: Some(pb::create_space_stream_response::Event::Result(r)),
            }) = &item
                && let Some(s) = &r.space
            {
                tracked.ready(&s.id);
                me.adopt(&claims, &s.id);
                super::record_cost(&me.shared, &claims.root, &s.id, ask);
            }
            item
        });
        Ok(Response::new(Box::pin(adopted)))
    }

    async fn cancel_create_space(
        &self,
        req: Request<pb::CancelCreateSpaceRequest>,
    ) -> R<pb::CancelCreateSpaceResponse> {
        let space = self.resolve(&req.into_inner().space)?;
        self.owner()
            .cancel_create_space(Request::new(pb::CancelCreateSpaceRequest {
                space: space.id,
            }))
            .await
    }

    async fn get_gpu_support(
        &self,
        _: Request<pb::GetGpuSupportRequest>,
    ) -> R<pb::GetGpuSupportResponse> {
        deny()
    }

    async fn delete_space(
        &self,
        req: Request<pb::DeleteSpaceRequest>,
    ) -> R<pb::DeleteSpaceResponse> {
        let space = self.resolve(&req.into_inner().space)?;
        let message = self.delete_tree(&space.id).await?;
        Ok(Response::new(pb::DeleteSpaceResponse { message }))
    }

    async fn delete_spaces(
        &self,
        req: Request<pb::DeleteSpacesRequest>,
    ) -> R<pb::DeleteSpacesResponse> {
        let mut results = Vec::new();
        for q in req.into_inner().spaces {
            let r = match self.resolve(&q) {
                Ok(s) => self.delete_tree(&s.id).await,
                Err(e) => Err(e),
            };
            results.push(match r {
                Ok(message) => pb::SpaceBulkResult {
                    space: q,
                    ok: true,
                    message,
                    ..Default::default()
                },
                Err(e) => pb::SpaceBulkResult {
                    space: q,
                    ok: false,
                    error: e.message().to_owned(),
                    ..Default::default()
                },
            });
        }
        Ok(Response::new(pb::DeleteSpacesResponse { results }))
    }

    async fn leave_spaces(&self, _: Request<pb::LeaveSpacesRequest>) -> R<pb::LeaveSpacesResponse> {
        deny()
    }

    async fn rename_space(
        &self,
        req: Request<pb::RenameSpaceRequest>,
    ) -> R<pb::RenameSpaceResponse> {
        let r = req.into_inner();
        let space = self.resolve(&r.space)?;
        self.owner()
            .rename_space(Request::new(pb::RenameSpaceRequest {
                space: space.id,
                name: r.name,
            }))
            .await
    }

    async fn fork_space(&self, req: Request<pb::ForkSpaceRequest>) -> R<pb::ForkSpaceResponse> {
        let claims = claims(&req)?;
        let r = req.into_inner();
        if claims.depth.saturating_add(1) > claims.max_depth {
            return Err(Status::resource_exhausted(format!(
                "this Space is {} level(s) below its tree's root and the tree allows {}: \
                 it cannot make children",
                claims.depth, claims.max_depth
            )));
        }
        // A Space may fork itself or any of its descendants.
        let source = if r.source.trim().is_empty() || r.source == self.space {
            self.space.clone()
        } else {
            self.resolve(&r.source)?.id
        };
        let name = if r.name.trim().is_empty() {
            self.next_child_name("f")
        } else {
            r.name
        };
        let (vcpus, memory_mb) = super::local_size(&self.shared, &source);
        let _held = super::admit(
            &self.shared,
            &claims.root,
            super::budget::Ask {
                vcpus,
                memory_mb,
                usd_per_hour: 0.0,
            },
        )?;
        let name_of_fork = name.clone();
        let resp = self
            .owner()
            .fork_space(Request::new(pb::ForkSpaceRequest { source, name }))
            .await?;
        if let Some(s) = &resp.get_ref().space {
            // A fork starts with its source's labels: it is the caller's
            // child whatever it was forked from.
            let depth = claims.depth.saturating_add(1);
            let _ = self.spaces().registry().update_meta(&s.id, |m| {
                m.parent = Some(self.space.clone());
                m.root = Some(claims.root.clone());
                m.depth = depth;
                m.origin = Some(cua_spaces::tree::ORIGIN_CHILD.into());
                m.labels.insert(LABEL_PARENT.into(), self.space.clone());
                m.labels.remove(LABEL_ROOT);
                m.labels.remove(LABEL_DEPTH);
            });
            self.audit("fork", &s.id, &name_of_fork);
            self.adopt(&claims, &s.id);
        }
        Ok(resp)
    }

    type BuildSpaceStream = <Svc as SpaceService>::BuildSpaceStream;

    async fn build_space(&self, _: Request<pb::BuildSpaceRequest>) -> R<Self::BuildSpaceStream> {
        deny()
    }

    async fn list_builds(&self, _: Request<pb::ListBuildsRequest>) -> R<pb::ListBuildsResponse> {
        deny()
    }

    async fn delete_build(&self, _: Request<pb::DeleteBuildRequest>) -> R<pb::DeleteBuildResponse> {
        deny()
    }

    async fn set_space_lifetime(
        &self,
        req: Request<pb::SetSpaceLifetimeRequest>,
    ) -> R<pb::SetSpaceLifetimeResponse> {
        let mut r = req.into_inner();
        let space = self.resolve(&r.space)?;
        // Never past the caller's own end.
        if let (Some(own), Some(d)) = (self.spaces().lifetime(&self.space), r.duration.as_ref()) {
            let left = own
                .expires_at()
                .duration_since(std::time::SystemTime::now())
                .unwrap_or_default();
            let asked = Duration::from_secs(u64::try_from(d.seconds).unwrap_or(0));
            if asked > left {
                return Err(Status::permission_denied(format!(
                    "a child cannot outlive its parent: this Space ends in {}s",
                    left.as_secs()
                )));
            }
        }
        r.space = space.id;
        self.owner().set_space_lifetime(Request::new(r)).await
    }

    async fn set_space_egress(
        &self,
        _: Request<pb::SetSpaceEgressRequest>,
    ) -> R<pb::SetSpaceEgressResponse> {
        Err(refused(
            "give children its own egress rules (set them on the parent)",
        ))
    }

    async fn get_tree_budget(
        &self,
        req: Request<pb::GetTreeBudgetRequest>,
    ) -> R<pb::GetTreeBudgetResponse> {
        // Its own tree's budget, and nothing about another tree.
        let claims = claims(&req)?;
        Ok(Response::new(pb::GetTreeBudgetResponse {
            budget: Some(tree_budget(&self.shared, &claims.root)),
        }))
    }

    async fn set_tree_budget(
        &self,
        _: Request<pb::SetTreeBudgetRequest>,
    ) -> R<pb::SetTreeBudgetResponse> {
        Err(refused("spend within its tree's budget; the owner sets it"))
    }

    async fn list_orphans(&self, _: Request<pb::ListOrphansRequest>) -> R<pb::ListOrphansResponse> {
        deny()
    }

    async fn list_stream_windows(
        &self,
        _: Request<pb::ListStreamWindowsRequest>,
    ) -> R<pb::ListStreamWindowsResponse> {
        // Watching is the person's; an agent shows a window with `stream_space_window`.
        deny()
    }

    async fn open_stream_entry(
        &self,
        _: Request<pb::OpenStreamEntryRequest>,
    ) -> R<pb::OpenStreamEntryResponse> {
        deny()
    }

    async fn get_space_network(
        &self,
        _: Request<pb::GetSpaceNetworkRequest>,
    ) -> R<pb::GetSpaceNetworkResponse> {
        // Network, clipboard and Volume settings are the owner's, never an agent's.
        deny()
    }

    async fn set_space_network(
        &self,
        _: Request<pb::SetSpaceNetworkRequest>,
    ) -> R<pb::SetSpaceNetworkResponse> {
        // Network, clipboard and Volume settings are the owner's, never an agent's.
        deny()
    }

    async fn get_space_network_log(
        &self,
        _: Request<pb::GetSpaceNetworkLogRequest>,
    ) -> R<pb::GetSpaceNetworkLogResponse> {
        // Network, clipboard and Volume settings are the owner's, never an agent's.
        deny()
    }

    async fn set_space_clipboard(
        &self,
        _: Request<pb::SetSpaceClipboardRequest>,
    ) -> R<pb::SetSpaceClipboardResponse> {
        // Network, clipboard and Volume settings are the owner's, never an agent's.
        deny()
    }

    async fn set_space_volume(
        &self,
        _: Request<pb::SetSpaceVolumeRequest>,
    ) -> R<pb::SetSpaceVolumeResponse> {
        // Network, clipboard and Volume settings are the owner's, never an agent's.
        deny()
    }

    async fn set_space_children(
        &self,
        req: Request<pb::SetSpaceChildrenRequest>,
    ) -> R<pb::SetSpaceChildrenResponse> {
        // A Space may turn its children's own children off, never on: the
        // tree's depth decides that.
        let r = req.into_inner();
        let mut space = self.resolve(&r.space)?;
        if r.enabled {
            return Err(refused(
                "let its children make children as deep as its tree allows (that is automatic)",
            ));
        }
        self.spaces()
            .set_children(&space.id, false)
            .map_err(|e| Status::internal(e.to_string()))?;
        if let Some(a) = self.shared.spawn_authority() {
            a.revoke(&space.id);
        }
        space.children = false;
        Ok(Response::new(pb::SetSpaceChildrenResponse {
            space: Some(space),
        }))
    }

    async fn connect_space(
        &self,
        req: Request<pb::ConnectSpaceRequest>,
    ) -> R<pb::ConnectSpaceResponse> {
        // The child's commands go through this channel with the caller's
        // own capability.
        let bearer = req
            .extensions()
            .get::<super::Presented>()
            .map(|p| p.0.clone())
            .unwrap_or_default();
        let origin = req
            .extensions()
            .get::<super::Origin>()
            .map(|o| o.0.clone())
            .unwrap_or_default();
        let space = self.resolve(&req.into_inner().space)?;
        let mut resp = self
            .owner()
            .connect_space(Request::new(pb::ConnectSpaceRequest {
                space: space.id.clone(),
            }))
            .await?
            .into_inner();
        // Through this channel's own passthrough, never the daemon's
        // loopback listener, its token or another computer's route.
        let key = crate::passthrough::space_key(&space.id);
        resp.env_url = if resp.env_url.is_empty() || origin.is_empty() {
            String::new()
        } else {
            format!("http://{origin}/v1/spaces/{key}/env")
        };
        resp.token = bearer;
        resp.services.clear();
        resp.bulk = String::new();
        Ok(Response::new(resp))
    }

    async fn list_space_tools(
        &self,
        _: Request<pb::ListSpaceToolsRequest>,
    ) -> R<pb::ListSpaceToolsResponse> {
        let all = self
            .owner()
            .list_space_tools(Request::new(pb::ListSpaceToolsRequest {}))
            .await?
            .into_inner();
        let tools: Vec<Value> = serde_json::from_str(&all.tools_json).unwrap_or_default();
        let kept: Vec<Value> = tools
            .into_iter()
            .filter(|t| {
                t.get("name")
                    .and_then(Value::as_str)
                    .is_some_and(|n| Self::tool_target(n).is_some())
            })
            .collect();
        Ok(Response::new(pb::ListSpaceToolsResponse {
            tools_json: Value::Array(kept).to_string(),
        }))
    }

    async fn call_space_tool(
        &self,
        req: Request<pb::CallSpaceToolRequest>,
    ) -> R<pb::CallSpaceToolResponse> {
        let r = req.into_inner();
        let mut args: Value = if r.arguments_json.trim().is_empty() {
            Value::Object(Default::default())
        } else {
            serde_json::from_str(&r.arguments_json)
                .map_err(|e| Status::invalid_argument(format!("arguments_json: {e}")))?
        };
        self.scoped_tool_args(&r.name, &mut args)?;
        self.owner()
            .call_space_tool(Request::new(pb::CallSpaceToolRequest {
                name: r.name,
                arguments_json: args.to_string(),
            }))
            .await
    }

    async fn list_hosts(&self, _: Request<pb::ListHostsRequest>) -> R<pb::ListHostsResponse> {
        deny()
    }

    async fn pool_request(
        &self,
        req: Request<pb::PoolRequestRequest>,
    ) -> R<pb::PoolRequestResponse> {
        let claims = claims(&req)?;
        self.scoped_pool_request(&claims.root, claims.max_depth, req.into_inner())
            .await
    }

    async fn get_space_thumbnail(
        &self,
        req: Request<pb::GetSpaceThumbnailRequest>,
    ) -> R<pb::GetSpaceThumbnailResponse> {
        let mut r = req.into_inner();
        r.space = self.resolve(&r.space)?.id;
        self.owner().get_space_thumbnail(Request::new(r)).await
    }

    async fn get_space_route(
        &self,
        req: Request<pb::GetSpaceRouteRequest>,
    ) -> R<pb::GetSpaceRouteResponse> {
        let mut r = req.into_inner();
        r.space = self.resolve(&r.space)?.id;
        self.owner().get_space_route(Request::new(r)).await
    }

    async fn get_resources(
        &self,
        _: Request<pb::GetResourcesRequest>,
    ) -> R<pb::GetResourcesResponse> {
        deny()
    }

    type WatchResourcesStream = <Svc as SpaceService>::WatchResourcesStream;

    async fn watch_resources(
        &self,
        _: Request<pb::WatchResourcesRequest>,
    ) -> R<Self::WatchResourcesStream> {
        deny()
    }

    type WatchSpacesStream = <Svc as SpaceService>::WatchSpacesStream;

    async fn watch_spaces(&self, _: Request<pb::WatchSpacesRequest>) -> R<Self::WatchSpacesStream> {
        deny()
    }
}

#[tonic::async_trait]
impl DaemonService for Scoped {
    async fn get_info(&self, _: Request<pb::GetInfoRequest>) -> R<pb::GetInfoResponse> {
        // Who answers, and nothing of the daemon's own (no token, socket
        // or loopback address).
        Ok(Response::new(pb::GetInfoResponse {
            version: crate::VERSION.into(),
            features: vec!["spaces".into(), super::CHILDREN_FEATURE.into()],
            ..Default::default()
        }))
    }

    async fn open_media_bridge(
        &self,
        _: Request<pb::OpenMediaBridgeRequest>,
    ) -> R<pb::OpenMediaBridgeResponse> {
        deny()
    }

    async fn shutdown(&self, _: Request<pb::ShutdownRequest>) -> R<pb::ShutdownResponse> {
        deny()
    }
}

/// Shares on the spawn channel: an agent in a Space shares a raw TCP
/// service of its own Space (or a child it made) at a public address
/// (public TCP shares, an experiment). Nothing else: no HTTP links, no
/// ports of this computer, no files, and only its own shares are listed or
/// stopped.
#[tonic::async_trait]
impl cua_proto::daemon::v1::share_service_server::ShareService for Scoped {
    async fn create_share(
        &self,
        req: Request<pb::CreateShareRequest>,
    ) -> R<pb::CreateShareResponse> {
        let mut r = req.into_inner();
        if !r.tcp {
            return Err(refused(
                "share a TCP port of itself or its children publicly (tcp)",
            ));
        }
        // Only the owner agrees to a share edge's cost.
        r.consent = false;
        let target = r.target.get_or_insert_with(Default::default);
        if target.kind == pb::ShareTargetKind::Unspecified as i32
            || target.kind == pb::ShareTargetKind::Port as i32
        {
            // "Port 25565" from inside a Space means this Space's port.
            target.kind = pb::ShareTargetKind::SpacePort as i32;
        }
        if target.kind != pb::ShareTargetKind::SpacePort as i32 {
            return Err(refused("share a TCP port of itself or its children"));
        }
        if target.space.is_empty() {
            target.space = self.space.clone();
        }
        let svc = self.owner();
        let rt = svc.rt();
        let create = crate::share_svc::real::create_from_pb(r, Some(self.space.clone()))
            .map_err(crate::server::st)?;
        let hub = crate::share_svc::real::hub(rt).map_err(crate::server::st)?;
        let share = hub.create(rt, create).await.map_err(crate::server::st)?;
        Ok(Response::new(pb::CreateShareResponse {
            share: Some(crate::share_svc::real::share_to_pb(&share)),
        }))
    }

    async fn list_shares(&self, _: Request<pb::ListSharesRequest>) -> R<pb::ListSharesResponse> {
        let svc = self.owner();
        let hub = crate::share_svc::real::hub(svc.rt()).map_err(crate::server::st)?;
        Ok(Response::new(pb::ListSharesResponse {
            shares: hub
                .list_from(&self.space)
                .iter()
                .map(crate::share_svc::real::share_to_pb)
                .collect(),
        }))
    }

    async fn stop_share(&self, req: Request<pb::StopShareRequest>) -> R<pb::StopShareResponse> {
        let r = req.into_inner();
        let svc = self.owner();
        let rt = svc.rt();
        let hub = crate::share_svc::real::hub(rt).map_err(crate::server::st)?;
        let gone = hub
            .stop_from(
                rt,
                &self.space,
                (!r.id.is_empty()).then_some(r.id.as_str()),
                r.all,
            )
            .map_err(crate::server::st)?;
        Ok(Response::new(pb::StopShareResponse {
            stopped: gone.into_iter().map(|s| s.handle).collect(),
        }))
    }

    async fn check_share(&self, _: Request<pb::CheckShareRequest>) -> R<pb::CheckShareResponse> {
        Err(refused(
            "share a TCP port of itself or its children publicly",
        ))
    }

    // The owner's decisions and settings: never from inside a Space.
    async fn approve_share(
        &self,
        _: Request<pb::ApproveShareRequest>,
    ) -> R<pb::ApproveShareResponse> {
        Err(refused("ask the owner: only they approve public shares"))
    }

    async fn deny_share(&self, _: Request<pb::DenyShareRequest>) -> R<pb::DenyShareResponse> {
        Err(refused("ask the owner: only they decide on public shares"))
    }

    async fn get_share_settings(
        &self,
        _: Request<pb::GetShareSettingsRequest>,
    ) -> R<pb::GetShareSettingsResponse> {
        Err(refused("share settings are the owner's"))
    }

    async fn set_share_settings(
        &self,
        _: Request<pb::SetShareSettingsRequest>,
    ) -> R<pb::SetShareSettingsResponse> {
        Err(refused("share settings are the owner's"))
    }

    async fn remove_share_edge(
        &self,
        _: Request<pb::RemoveShareEdgeRequest>,
    ) -> R<pb::RemoveShareEdgeResponse> {
        Err(refused("share settings are the owner's"))
    }
}

#[cfg(test)]
mod agent_words_tests {
    use super::*;

    #[test]
    fn a_network_or_boot_failure_is_said_without_ports_or_internals() {
        let a = agent_words("invalid request: probe port 3211 is not published by this sandbox")
            .unwrap();
        assert!(
            !a.contains("3211") && a.contains("network setup did not come up"),
            "{a}"
        );
        let b = agent_words("local runtime: lume VM 'mac1' stopped while booting").unwrap();
        assert!(
            !b.to_lowercase().contains("lume") && b.contains("stopped while starting up"),
            "{b}"
        );
    }

    #[test]
    fn a_full_disk_is_said_for_the_person_not_the_terminal() {
        let m = "not enough disk space to pull ghcr.io/trycua/macos:26: it needs about 55.0 GiB and cua keeps 5.0 GiB free, but only 24.5 GiB is available on /Users/a/.cua (run `cua cache prune` to free space, or lower CUA_DISK_MIN_FREE)";
        let w = agent_words(m).unwrap();
        assert!(
            w.contains("needs about 55.0 GiB") && w.contains("only 24.5 GiB is free"),
            "{w}"
        );
        assert!(!w.contains("cache prune") && !w.contains("CUA_DISK_MIN_FREE"));
        assert!(w.contains("Tell the person"));
    }

    #[test]
    fn a_missing_gateway_is_an_install_problem() {
        let w = agent_words("cua-netgw is missing: the network gateway ships next to `cua`; set CUA_NETGW to its path (libs/cua/netgw/build.sh builds it)").unwrap();
        assert!(w.contains("update Cua Spaces") && !w.contains("build.sh"));
    }

    #[test]
    fn other_errors_pass_through() {
        assert!(agent_words("the budget is spent").is_none());
        assert_eq!(
            for_the_agent(Status::resource_exhausted("the budget is spent")).message(),
            "the budget is spent"
        );
    }
}

#[cfg(test)]
mod stream_scope_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_child_window_can_only_be_shown_in_the_callers_own_space() {
        for asked in [json!("desktop"), json!("local:other"), Value::Null] {
            let mut args = json!({"space": "local:kid", "app_name": "Safari", "into": asked});
            pin_stream_target("stream_space_window", &mut args, "local:me");
            assert_eq!(args["into"], "local:me");
        }
        // Other tools are left alone.
        let mut args = json!({"space": "local:kid"});
        pin_stream_target("space_bash", &mut args, "local:me");
        assert!(args.get("into").is_none());
    }

    #[test]
    fn the_tools_for_driving_and_showing_a_child_are_all_scoped() {
        for tool in [
            "call_tool",
            "list_tools",
            "space_bash",
            "list_space_windows",
            "stream_space_window",
        ] {
            assert_eq!(Scoped::tool_target(tool), Some("space"), "{tool}");
        }
        assert_eq!(Scoped::tool_target("stream_endpoint"), None);
    }
}

#[cfg(test)]
mod tracked_tests {
    use super::*;

    fn req(name: &str) -> pb::CreateSpaceRequest {
        pb::CreateSpaceRequest {
            name: name.into(),
            image: "linux:24.04".into(),
            ..Default::default()
        }
    }

    #[test]
    fn a_create_whose_stream_is_dropped_is_failed_not_left_on_creating() {
        let d = tempfile::tempdir().unwrap();
        let t = Tracked::start(d.path(), "local:p", &req("a"));
        drop(t);
        let e = &cua_spaces::child_creates::of(d.path(), "local:p")[0];
        assert_eq!(e.state, cua_spaces::child_creates::State::Failed);
        assert!(e.error.contains("interrupted"), "{}", e.error);
    }

    #[test]
    fn a_create_that_finished_is_not_touched_when_dropped() {
        let d = tempfile::tempdir().unwrap();
        let t = Tracked::start(d.path(), "local:p", &req("a"));
        t.ready("local:a");
        drop(t);
        let e = &cua_spaces::child_creates::of(d.path(), "local:p")[0];
        assert_eq!(e.state, cua_spaces::child_creates::State::Ready);
    }

    #[test]
    fn two_creates_under_one_name_are_tracked_apart() {
        let d = tempfile::tempdir().unwrap();
        let a = Tracked::start(d.path(), "local:p", &req("same"));
        let b = Tracked::start(d.path(), "local:p", &req("same"));
        b.failed("nope");
        let all = cua_spaces::child_creates::of(d.path(), "local:p");
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].state, cua_spaces::child_creates::State::Creating);
        drop(a);
    }
}
