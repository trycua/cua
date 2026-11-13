//! Public TCP shares and their routes (CUA-2602): the Space's own cloud
//! machine (`host`), a share edge in the owner's cloud (`edge`) or Cua's
//! relay (`relay`), picked per share by [`cua_share::route::choose`] within
//! the owner's share settings.
//!
//! Also here: shares that wait for the owner (an agent's public share needs
//! approval; the first share edge in a cloud needs consent), the daily data
//! cap, and the sweep that leaves nothing behind in the owner's cloud.

use super::*;
use cua_share::Waiting;
use cua_share::route::{
    DailyUse, LinkedCloud, NoRoute, Route, RoutePlan, RouteRequest, ShareSettings, SpaceLocation,
    choose, consent_text, edge_cost,
};
use serde::{Deserialize, Serialize};

/// How long a share may wait for its owner before it is dropped.
pub const PENDING_TTL: Duration = Duration::from_secs(30 * 60);

/// How long the daemon waits for this computer to join a new share edge
/// (the edge boots, gets its certificate, then cua-spacesd connects out).
pub const EDGE_JOIN_WAIT: Duration = Duration::from_secs(4 * 60);

/// A share waiting for its owner, with what was asked.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Pending {
    pub share: Share,
    pub req: CreateShare,
}

/// `<home>/share/usage.json`: the day's data and what each share had moved
/// when last seen.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct Usage {
    #[serde(default)]
    pub today: DailyUse,
    #[serde(default)]
    pub seen: HashMap<String, u64>,
}

/// `YYYY-MM-DD` (UTC) of unix seconds `secs`.
pub fn utc_day(secs: u64) -> String {
    // Howard Hinnant's days-to-civil.
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

fn now_secs() -> u64 {
    cua_share::now_ms() / 1000
}

/// Why a new public TCP share must wait for the owner, if it must: an agent
/// asked (unless its Space is always allowed), or the route needs a share
/// edge the owner has not agreed to (unless this request agrees).
pub fn waiting_for(
    origin: Option<&str>,
    port: u16,
    plan: &RoutePlan,
    settings: &ShareSettings,
    consent: bool,
) -> Option<Waiting> {
    let consent_line = || {
        let cloud = plan.cloud.as_deref().unwrap_or_default();
        consent_text(
            plan.cloud_title.as_deref().unwrap_or(cloud),
            &edge_cost(cloud),
            settings.daily_cap_mib,
        )
    };
    if let Some(space) = origin
        && settings.needs_approval(space)
    {
        let mut text = format!(
            "An agent in {space} wants to share port {port} publicly. Anyone with the address can connect."
        );
        if plan.needs_consent {
            text.push(' ');
            text.push_str(&consent_line());
        }
        return Some(Waiting {
            reason: "approval".into(),
            text,
            route_line: plan.line(),
        });
    }
    if plan.needs_consent && !consent {
        return Some(Waiting {
            reason: "consent".into(),
            text: consent_line(),
            route_line: plan.line(),
        });
    }
    None
}

/// The bytes a new share may move today (0: no cap), or why none.
pub fn budget(
    settings: &ShareSettings,
    usage: &DailyUse,
    day: &str,
) -> std::result::Result<u64, String> {
    if settings.daily_cap_mib == 0 {
        return Ok(0);
    }
    let cap = settings.daily_cap_mib.saturating_mul(1 << 20);
    let used = if usage.day == day { usage.bytes } else { 0 };
    if used >= cap {
        return Err(format!(
            "public shares used today's data cap ({}); they start again tomorrow (UTC), or raise the cap in Settings, Sharing",
            cua_share::route::human_mib(settings.daily_cap_mib)
        ));
    }
    Ok(cap - used)
}

fn no_route(e: NoRoute) -> Error {
    Error::Unsupported(e.0)
}

impl ShareHub {
    fn pending_path(&self) -> PathBuf {
        self.home.join("share").join("pending.json")
    }

    pub(crate) fn pending(&self) -> Vec<Pending> {
        let _g = self.side.lock().unwrap();
        self.read_pending()
    }

    fn read_pending(&self) -> Vec<Pending> {
        let now = cua_share::now_ms();
        std::fs::read(self.pending_path())
            .ok()
            .and_then(|b| serde_json::from_slice::<Vec<Pending>>(&b).ok())
            .unwrap_or_default()
            .into_iter()
            .filter(|p| !p.share.expired(now))
            .collect()
    }

    fn write_pending(&self, all: &[Pending]) -> Result<()> {
        let path = self.pending_path();
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(all).unwrap_or_default())?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }

    fn take_pending(&self, id: &str) -> Option<Pending> {
        let _g = self.side.lock().unwrap();
        let mut all = self.read_pending();
        let i = all
            .iter()
            .position(|p| p.share.handle == id || p.share.name.as_deref() == Some(id))?;
        let p = all.remove(i);
        let _ = self.write_pending(&all);
        Some(p)
    }

    /// Removes a waiting share by its id (no audit).
    pub(super) fn take_pending_share(&self, handle: &str) -> Option<Share> {
        self.take_pending(handle).map(|p| p.share)
    }

    fn usage_path(&self) -> PathBuf {
        self.home.join("share").join("usage.json")
    }

    pub(crate) fn usage(&self) -> Usage {
        std::fs::read(self.usage_path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn save_usage(&self, u: &Usage) {
        let path = self.usage_path();
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, serde_json::to_vec(u).unwrap_or_default()).is_ok() {
            let _ = std::fs::rename(tmp, path);
        }
    }

    /// Data public TCP shares moved today.
    pub fn used_today(&self) -> u64 {
        let u = self.usage();
        if u.today.day == utc_day(now_secs()) {
            u.today.bytes
        } else {
            0
        }
    }

    /// The share settings.
    pub fn settings(&self) -> ShareSettings {
        ShareSettings::load(&self.home)
    }

    /// Saves new share settings (allowlist entries checked and normalized).
    pub fn set_settings(&self, mut s: ShareSettings) -> Result<ShareSettings> {
        s.default_allow =
            ShareSettings::check_allow(&s.default_allow).map_err(Error::InvalidArgument)?;
        if s.default_allow.len() > 32 {
            return Err(Error::InvalidArgument(
                "at most 32 allowlist entries".into(),
            ));
        }
        s.always_allow_spaces.sort();
        s.always_allow_spaces.dedup();
        s.edge_consent.sort();
        s.edge_consent.dedup();
        s.save(&self.home)?;
        use cua_share::AuditSink as _;
        self.audit.record(
            "user",
            "settings",
            "share-settings",
            &format!(
                "landing={:?} relay={} cap_mib={} approve_agents={} allow={}",
                s.landing,
                s.relay_fallback,
                s.daily_cap_mib,
                s.approve_agent_shares,
                s.default_allow.join(",")
            ),
        );
        Ok(s)
    }

    #[cfg(feature = "byoc")]
    fn location(&self, space: &str) -> (SpaceLocation, Vec<LinkedCloud>) {
        (self.cloud.location(space), self.cloud.linked())
    }

    #[cfg(not(feature = "byoc"))]
    fn location(&self, _space: &str) -> (SpaceLocation, Vec<LinkedCloud>) {
        (SpaceLocation::Local, Vec::new())
    }

    /// Shares a Space port as raw TCP at a public address, on the route the
    /// request and settings allow. `approved`: the owner approved a share
    /// that waited (its consent included); `handle`: the waiting share's id.
    pub(super) async fn create_tcp(
        &self,
        rt: &Runtime,
        req: CreateShare,
        approved: bool,
        handle: Option<String>,
    ) -> Result<Share> {
        check_tcp_request(&req)?;
        let tcp = req.tcp.clone().unwrap_or_default();
        let ttl = match req.ttl {
            Some(t) => check_ttl(t).map_err(|e| Error::InvalidArgument(e.to_string()))?,
            None => TCP_TTL,
        };
        let target = self.validate_target(rt, req.target.clone()).await?;
        if let Some(origin) = &req.origin {
            self.check_origin(rt, origin, &target).await?;
        }
        self.check_tree_cap(rt, &target)?;
        let Target::SpacePort { space, port } = target.clone() else {
            unreachable!("check_tcp_request");
        };
        let mut settings = self.settings();
        let allow = match &tcp.allow {
            Some(a) if !a.is_empty() => {
                if a.len() > 32 {
                    return Err(Error::InvalidArgument(
                        "at most 32 allowlist entries".into(),
                    ));
                }
                ShareSettings::check_allow(a).map_err(Error::InvalidArgument)?
            }
            _ => settings.default_allow.clone(),
        };
        if tcp.check_host && tcp.service.as_deref() != Some("minecraft") {
            return Err(Error::InvalidArgument(
                "the host check reads the Minecraft Java handshake: it needs --service minecraft"
                    .into(),
            ));
        }
        let asked = RouteRequest::parse(&tcp.route).ok_or_else(|| {
            Error::InvalidArgument(format!(
                "route {:?}: use auto, host, edge or relay",
                tcp.route
            ))
        })?;
        let (location, clouds) = self.location(&space);
        let plan = choose(&location, &clouds, &settings, asked).map_err(no_route)?;
        let now = cua_share::now_ms();
        if !approved
            && let Some(waiting) =
                waiting_for(req.origin.as_deref(), port, &plan, &settings, tcp.consent)
        {
            let share = Share {
                handle: format!("share-{}", random_hex(6)),
                key: String::new(),
                name: req.name.clone().filter(|n| !n.trim().is_empty()),
                target: target.clone(),
                access: Access::Public,
                owner: Owner {
                    account: "owner".into(),
                    email: None,
                },
                teams: Vec::new(),
                created_ms: now,
                expires_ms: Some(now + PENDING_TTL.as_millis() as u64),
                url: String::new(),
                tcp: None,
                origin: req.origin.clone(),
                waiting: Some(waiting),
            };
            {
                let _g = self.side.lock().unwrap();
                let mut all = self.read_pending();
                // One waiting request per Space port: a repeat replaces it.
                all.retain(|p| p.share.target != share.target);
                all.push(Pending {
                    share: share.clone(),
                    req: CreateShare {
                        target: target.clone(),
                        ..req.clone()
                    },
                });
                self.write_pending(&all)?;
            }
            let who = match &req.origin {
                Some(o) => format!("agent in {o}"),
                None => "user".into(),
            };
            self.audit_record(&who, "waiting", &share);
            return Ok(share);
        }
        // Consent given here (the card was shown, or the owner approved a
        // share that said so): remember it for this cloud.
        if plan.needs_consent
            && let Some(cloud) = &plan.cloud
        {
            settings.edge_consent.push(cloud.clone());
            settings.edge_consent.sort();
            settings.edge_consent.dedup();
            settings.save(&self.home)?;
        }
        let max_bytes = budget(&settings, &self.usage().today, &utc_day(now_secs()))
            .map_err(Error::HostCapacity)?;
        // Cua's relay needs the cua.ai sign-in; the owner's own cloud does
        // not (the account only names the owner in lists and the audit).
        let ctx = match (
            plan.route,
            crate::share_link::LinkContext::resolve(rt, &self.home).await,
        ) {
            (Route::Relay, Err(e)) => return Err(e),
            (_, r) => r,
        };
        let owner = match &ctx {
            Ok(c) => Owner {
                account: c.owner_account.clone(),
                email: c.owner_email.clone(),
            },
            Err(_) => Owner {
                account: "owner".into(),
                email: None,
            },
        };
        let mut share = Share {
            handle: handle.unwrap_or_else(|| format!("share-{}", random_hex(6))),
            key: random_hex(cua_share::TOKEN_LEN / 2),
            name: req.name.clone().filter(|n| !n.trim().is_empty()),
            target: target.clone(),
            access: Access::Public,
            owner,
            teams: Vec::new(),
            created_ms: now,
            expires_ms: Some(now + ttl.as_millis() as u64),
            url: String::new(),
            tcp: None,
            origin: req.origin.clone(),
            waiting: None,
        };
        let mut endpoint = TcpEndpoint {
            address: String::new(),
            host: String::new(),
            port: 0,
            srv_name: None,
            service: tcp.service.clone(),
            route: plan.route.as_str().into(),
            cloud: plan.cloud.clone(),
            route_line: plan.line(),
            allow: allow.clone(),
            check_host: tcp.check_host,
            max_bytes,
            slot: 0,
        };
        match plan.route {
            Route::Relay => {
                let Ok(ctx) = &ctx else { unreachable!() };
                // Fail now, not on the first player, when the port is unreachable.
                self.space_base(rt, &space, port).await?;
                let token = ctx.access_token().await?;
                let view = ctx
                    .client
                    .tcp_share_allocate(
                        &token,
                        ctx.machine_id(),
                        &cua_host::TcpShareRequest {
                            token: share.key.clone(),
                            target_port: port,
                            ttl_secs: Some(ttl.as_secs()),
                            service: tcp.service.clone(),
                            name: share.name.clone(),
                            allow: allow.clone(),
                            check_host: tcp.check_host,
                            max_bytes: (max_bytes > 0).then_some(max_bytes),
                        },
                    )
                    .await
                    .map_err(|e| match e {
                        cua_host::Error::NotFound(_) => {
                            Error::Unsupported("this relay does not offer public TCP shares".into())
                        }
                        other => {
                            Error::Unsupported(format!("the relay refused the TCP share: {other}"))
                        }
                    })?;
                endpoint.address = view.address;
                endpoint.host = view.host;
                endpoint.port = view.port;
                endpoint.srv_name = view.srv_name;
            }
            Route::Host => {
                self.open_host_route(rt, &mut share, &mut endpoint, &space, port, ttl)
                    .await?;
            }
            Route::Edge => {
                self.open_edge_route(rt, &mut share, &mut endpoint, &space, port, ttl)
                    .await?;
            }
        }
        share.url = format!(
            "tcp://{}",
            endpoint
                .srv_name
                .clone()
                .unwrap_or_else(|| endpoint.address.clone())
        );
        share.tcp = Some(endpoint);
        if let Err(e) = self.store.add(share.clone()) {
            self.release_tcp(rt, &share).await;
            return Err(Error::Internal(e.to_string()));
        }
        let principal = match (&share.origin, &ctx) {
            (Some(origin), Ok(c)) => format!("{} (agent in {origin})", c.principal()),
            (Some(origin), Err(_)) => format!("owner (agent in {origin})"),
            (None, Ok(c)) => c.principal(),
            (None, Err(_)) => "owner".into(),
        };
        self.audit_record(
            &principal,
            if approved { "approve" } else { "create" },
            &share,
        );
        Ok(share)
    }

    #[cfg(feature = "byoc")]
    async fn open_host_route(
        &self,
        rt: &Runtime,
        share: &mut Share,
        endpoint: &mut TcpEndpoint,
        space: &str,
        port: u16,
        ttl: Duration,
    ) -> Result<()> {
        let expires = now_secs() + ttl.as_secs();
        let opened = self
            .cloud
            .open_host(&share.handle, space, &endpoint.allow, expires)
            .await?;
        let publish = async {
            let sandbox = rt.handle(space).await?;
            let client = sandbox.spacesd().await?;
            client
                .tunnel()
                .publish_share_slot(cua_spacesd_client::pb::PublishShareSlotRequest {
                    slot: u32::from(opened.slot),
                    port: u32::from(port),
                    allow: endpoint.allow.clone(),
                    expect_hosts: if endpoint.check_host {
                        vec![opened.ip.clone()]
                    } else {
                        Vec::new()
                    },
                    ttl: Some(crate::convert::dur(ttl)),
                    max_bytes: endpoint.max_bytes,
                })
                .await?;
            Ok::<(), Error>(())
        };
        if let Err(e) = publish.await {
            // Nothing stays open in the cloud for a share that never served.
            let _ = self.cloud.close(&share.handle).await;
            return Err(Error::Unsupported(format!(
                "the Space's host could not serve the share (its image may predate share slots; recreate the Space): {e}"
            )));
        }
        endpoint.address = format!("{}:{}", opened.ip, opened.slot);
        endpoint.host = opened.ip;
        endpoint.port = opened.slot;
        endpoint.slot = opened.slot;
        Ok(())
    }

    #[cfg(not(feature = "byoc"))]
    async fn open_host_route(
        &self,
        _rt: &Runtime,
        _share: &mut Share,
        _endpoint: &mut TcpEndpoint,
        _space: &str,
        _port: u16,
        _ttl: Duration,
    ) -> Result<()> {
        Err(Error::Unsupported(
            "this daemon was built without your-cloud support".into(),
        ))
    }

    /// The share edge's client: its URL, the token it accepts and this
    /// computer's id on it.
    #[cfg(feature = "byoc")]
    fn edge_client(&self) -> Option<(cua_host::RelayClient, String, String)> {
        let edge = self.cloud.edge()?;
        let policy = cua_host::Host::new(&self.home).policy().ok().flatten()?;
        let join = policy.edges.iter().find(|e| e.url == edge.tunnel_url())?;
        let token = std::fs::read_to_string(&join.token_file)
            .ok()?
            .trim()
            .to_owned();
        let client = cua_host::RelayClient::new(&join.url).ok()?;
        Some((client, token, join.machine_id.clone()))
    }

    #[cfg(feature = "byoc")]
    async fn open_edge_route(
        &self,
        rt: &Runtime,
        share: &mut Share,
        endpoint: &mut TcpEndpoint,
        space: &str,
        port: u16,
        ttl: Duration,
    ) -> Result<()> {
        let cloud = endpoint
            .cloud
            .clone()
            .ok_or_else(|| Error::Internal("an edge route without a cloud".into()))?;
        let host = cua_host::Host::new(&self.home);
        if host.policy().ok().flatten().is_none() {
            return Err(Error::Unsupported(
                "this computer does not serve shares yet: run `cua host setup` once".into(),
            ));
        }
        // The players' side reaches this Space port through the share
        // gateway, like a relay share: fail now when it cannot.
        self.space_base(rt, space, port).await?;
        let version = std::env::var("CUA_SHARE_EDGE_VERSION")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_owned());
        let (edge, token) = self.cloud.ensure_edge(&cloud, &version, now_secs()).await?;
        host.set_share_edges(&[(edge.tunnel_url(), token)])
            .map_err(|e| Error::Internal(e.to_string()))?;
        let (client, token, machine) = self
            .edge_client()
            .ok_or_else(|| Error::Internal("the share edge is not in the host policy".into()))?;
        let req = cua_host::TcpShareRequest {
            token: share.key.clone(),
            target_port: port,
            ttl_secs: Some(ttl.as_secs()),
            service: None,
            name: share.name.clone(),
            allow: endpoint.allow.clone(),
            check_host: endpoint.check_host,
            max_bytes: (endpoint.max_bytes > 0).then_some(endpoint.max_bytes),
        };
        // A new edge boots and gets its certificate, then this computer
        // joins it: retry until it answers.
        let start = std::time::Instant::now();
        let view = loop {
            match client.tcp_share_allocate(&token, &machine, &req).await {
                Ok(v) => break v,
                Err(e) if start.elapsed() < EDGE_JOIN_WAIT => {
                    tracing::debug!(error = %e, "share edge not ready yet");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
                Err(e) => {
                    return Err(Error::Unsupported(format!(
                        "the share edge in your {} account did not answer in time ({e}); try again in a minute",
                        cua_byoc::cloud_shares::cloud_title(&cloud)
                    )));
                }
            }
        };
        let _ = self.cloud.touch_edge(now_secs()).await;
        endpoint.address = view.address;
        endpoint.host = view.host;
        endpoint.port = view.port;
        endpoint.slot = view.port;
        Ok(())
    }

    #[cfg(not(feature = "byoc"))]
    async fn open_edge_route(
        &self,
        _rt: &Runtime,
        _share: &mut Share,
        _endpoint: &mut TcpEndpoint,
        _space: &str,
        _port: u16,
        _ttl: Duration,
    ) -> Result<()> {
        Err(Error::Unsupported(
            "this daemon was built without your-cloud support".into(),
        ))
    }

    /// Ends a TCP share's public side on its route (best effort: leases,
    /// slot lifetimes and the cloud sweep end what this misses).
    pub(super) async fn release_tcp(&self, rt: &Runtime, s: &Share) {
        let Some(t) = &s.tcp else { return };
        match t.route.as_str() {
            "host" => {
                #[cfg(feature = "byoc")]
                {
                    if let Target::SpacePort { space, .. } = &s.target
                        && let Ok(sandbox) = rt.handle(space).await
                        && let Ok(client) = sandbox.spacesd().await
                    {
                        let _ = client
                            .tunnel()
                            .unpublish_share_slot(
                                cua_spacesd_client::pb::UnpublishShareSlotRequest {
                                    slot: u32::from(t.slot),
                                },
                            )
                            .await;
                    }
                    if let Err(e) = self.cloud.close(&s.handle).await {
                        tracing::warn!(error = %e, share = %s.handle, "closing a share's firewall rule failed (the sweep retries)");
                    }
                }
            }
            "edge" =>
            {
                #[cfg(feature = "byoc")]
                if let Some((client, token, machine)) = self.edge_client() {
                    let _ = client.tcp_share_close(&token, &machine, t.port).await;
                }
            }
            _ => {
                let Ok(ctx) = crate::share_link::LinkContext::resolve(rt, &self.home).await else {
                    return;
                };
                let Ok(token) = ctx.access_token().await else {
                    return;
                };
                if let Err(e) = ctx
                    .client
                    .tcp_share_close(&token, ctx.machine_id(), t.port)
                    .await
                {
                    tracing::debug!(error = %e, "closing a tcp share on the relay failed (its lease ends it)");
                }
            }
        }
    }

    /// Renews every TCP share on its route, adds what each moved to the
    /// day's data, and ends shares that are gone on their route or over the
    /// day's cap. Then sweeps the owner's cloud.
    pub async fn keepalive_tcp(&self, rt: &Runtime) {
        let tcp: Vec<Share> = self
            .store
            .list()
            .into_iter()
            .filter(|s| s.tcp.is_some())
            .collect();
        let mut moved: HashMap<String, u64> = HashMap::new();
        let mut gone: Vec<String> = Vec::new();
        let relay = if tcp
            .iter()
            .any(|s| s.tcp.as_ref().is_some_and(|t| t.route == "relay"))
        {
            match crate::share_link::LinkContext::resolve(rt, &self.home).await {
                Ok(ctx) => ctx.access_token().await.ok().map(|t| (ctx, t)),
                Err(_) => None,
            }
        } else {
            None
        };
        #[cfg(feature = "byoc")]
        let edge = self.edge_client();
        #[cfg(feature = "byoc")]
        let mut slots: HashMap<String, Option<Vec<cua_spacesd_client::pb::ShareSlot>>> =
            HashMap::new();
        for s in &tcp {
            let Some(t) = &s.tcp else { continue };
            match t.route.as_str() {
                "relay" => {
                    let Some((ctx, token)) = &relay else { continue };
                    match ctx
                        .client
                        .tcp_share_keepalive(token, ctx.machine_id(), t.port)
                        .await
                    {
                        Ok(v) => {
                            moved.insert(s.handle.clone(), v.bytes);
                        }
                        Err(cua_host::Error::NotFound(_)) => gone.push(s.handle.clone()),
                        Err(e) => {
                            tracing::debug!(error = %e, "tcp share keep-alive failed (retried)")
                        }
                    }
                }
                #[cfg(feature = "byoc")]
                "edge" => {
                    let Some((client, token, machine)) = &edge else {
                        gone.push(s.handle.clone());
                        continue;
                    };
                    match client.tcp_share_keepalive(token, machine, t.port).await {
                        Ok(v) => {
                            moved.insert(s.handle.clone(), v.bytes);
                        }
                        Err(cua_host::Error::NotFound(_)) => gone.push(s.handle.clone()),
                        Err(e) => {
                            tracing::debug!(error = %e, "share edge keep-alive failed (retried)")
                        }
                    }
                }
                #[cfg(feature = "byoc")]
                "host" => {
                    let Target::SpacePort { space, .. } = &s.target else {
                        continue;
                    };
                    if !slots.contains_key(space) {
                        let listed = async {
                            let sandbox = rt.handle(space).await.ok()?;
                            let client = sandbox.spacesd().await.ok()?;
                            let r = client
                                .tunnel()
                                .list_share_slots(cua_spacesd_client::pb::ListShareSlotsRequest {})
                                .await
                                .ok()?;
                            Some(r.into_inner().slots)
                        };
                        slots.insert(space.clone(), listed.await);
                    }
                    // (A Space unreachable now is retried next time.)
                    if let Some(list) = slots.get(space).and_then(|v| v.as_ref()) {
                        match list.iter().find(|x| x.slot == u32::from(t.slot)) {
                            Some(x) => {
                                moved.insert(s.handle.clone(), x.bytes);
                            }
                            // The Space no longer serves it (restarted).
                            None => gone.push(s.handle.clone()),
                        }
                    }
                }
                _ => {}
            }
        }
        // The day's data.
        let settings = self.settings();
        let day = utc_day(now_secs());
        let mut usage = self.usage();
        let mut over = usage.today.over(&day, settings.daily_cap_mib);
        for (h, b) in &moved {
            let last = usage.seen.get(h).copied().unwrap_or(0);
            let delta = if *b >= last { b - last } else { *b };
            over |= usage.today.add(&day, delta, settings.daily_cap_mib);
            usage.seen.insert(h.clone(), *b);
        }
        let live: std::collections::HashSet<String> =
            tcp.iter().map(|s| s.handle.clone()).collect();
        usage.seen.retain(|h, _| live.contains(h));
        self.save_usage(&usage);
        let ended = |hub: &Self, handles: &[String], why: &str| {
            let removed = hub
                .store
                .remove_where(|o| handles.contains(&o.handle))
                .unwrap_or_default();
            for r in &removed {
                hub.audit_record("system", why, r);
                hub.release_forward(rt, r);
            }
            removed
        };
        let removed = ended(self, &gone, "ended");
        for r in &removed {
            self.release_tcp(rt, r).await;
        }
        if over {
            let all: Vec<String> = self
                .store
                .list()
                .into_iter()
                .filter(|s| s.tcp.is_some())
                .map(|s| s.handle)
                .collect();
            let removed = ended(self, &all, "data cap");
            for r in &removed {
                self.release_tcp(rt, r).await;
            }
        }
        self.sweep_cloud().await;
    }

    /// Closes what the owner's cloud holds for shares that ended (rules,
    /// an idle or orphaned edge), and leaves the edge when it is gone.
    #[cfg(feature = "byoc")]
    pub async fn sweep_cloud(&self) {
        let shares = self.store.list();
        let live: std::collections::HashSet<String> = shares
            .iter()
            .filter(|s| s.tcp.as_ref().is_some_and(|t| t.route == "host"))
            .map(|s| s.handle.clone())
            .collect();
        let edge_live = shares
            .iter()
            .any(|s| s.tcp.as_ref().is_some_and(|t| t.route == "edge"));
        let rep = self.cloud.sweep(&live, edge_live, now_secs()).await;
        for r in &rep.rules {
            tracing::info!(rule = %r, "share sweep closed a firewall rule");
        }
        for e in &rep.edges {
            tracing::info!(edge = %e, "share sweep removed a share edge");
        }
        if self.cloud.edge().is_none() {
            let _ = cua_host::Host::new(&self.home).set_share_edges(&[]);
        }
    }

    #[cfg(not(feature = "byoc"))]
    pub async fn sweep_cloud(&self) {}

    /// Removes the share edge now; the shares on it end.
    pub async fn remove_edge(&self, rt: &Runtime) -> Result<Vec<Share>> {
        let gone = self
            .store
            .remove_where(|s| s.tcp.as_ref().is_some_and(|t| t.route == "edge"))
            .map_err(|e| Error::Internal(e.to_string()))?;
        for s in &gone {
            self.audit_record("user", "stop", s);
            self.release_forward(rt, s);
        }
        #[cfg(feature = "byoc")]
        {
            self.cloud.remove_edge().await?;
            let _ = cua_host::Host::new(&self.home).set_share_edges(&[]);
        }
        Ok(gone)
    }

    /// The share edge, when there is one: its cloud and address.
    pub fn edge_info(&self) -> Option<(String, String)> {
        #[cfg(feature = "byoc")]
        {
            return self.cloud.edge().map(|e| (e.provider, e.host));
        }
        #[allow(unreachable_code)]
        None
    }

    /// Lets a waiting share go live (the owner approved it; its consent is
    /// included). `always`: the asking Space may share without asking from
    /// now on.
    pub async fn approve(&self, rt: &Runtime, id: &str, always: bool) -> Result<Share> {
        let p = self
            .take_pending(id)
            .ok_or_else(|| Error::NotFound(format!("no share {id} is waiting")))?;
        if always && let Some(origin) = &p.req.origin {
            let mut s = self.settings();
            s.always_allow_spaces.push(origin.clone());
            self.set_settings(s)?;
        }
        self.create_tcp(rt, p.req, true, Some(p.share.handle)).await
    }

    /// Turns down a waiting share.
    pub fn deny(&self, id: &str) -> Result<Share> {
        let p = self
            .take_pending(id)
            .ok_or_else(|| Error::NotFound(format!("no share {id} is waiting")))?;
        self.audit_record("user", "deny", &p.share);
        Ok(p.share)
    }
}

#[cfg(test)]
mod route_tests {
    use super::*;
    use cua_share::route::Landing;

    fn plan(route: Route, consent: bool) -> RoutePlan {
        RoutePlan {
            route,
            cloud: (route != Route::Relay).then(|| "aws".into()),
            cloud_title: (route != Route::Relay).then(|| "AWS".into()),
            needs_consent: consent,
        }
    }

    #[test]
    fn days_are_utc_dates() {
        assert_eq!(utc_day(0), "1970-01-01");
        assert_eq!(utc_day(951_782_400), "2000-02-29");
        assert_eq!(utc_day(1_798_761_600 - 1), "2026-12-31");
        assert_eq!(utc_day(1_798_761_600), "2027-01-01");
    }

    #[test]
    fn an_agents_share_waits_for_approval_unless_its_space_is_allowed() {
        let mut s = ShareSettings::default();
        let w = waiting_for(
            Some("local:mc"),
            25565,
            &plan(Route::Relay, false),
            &s,
            false,
        )
        .unwrap();
        assert_eq!(w.reason, "approval");
        assert!(
            w.text.contains("local:mc") && w.text.contains("25565"),
            "{}",
            w.text
        );
        assert_eq!(w.route_line, "Through Cua");
        // An agent cannot consent for the owner: its consent flag is ignored
        // while approval is needed, and the card's text comes with it.
        let w = waiting_for(Some("local:mc"), 25565, &plan(Route::Edge, true), &s, true).unwrap();
        assert_eq!(w.reason, "approval");
        assert!(
            w.text.contains("small server in your AWS account"),
            "{}",
            w.text
        );
        s.always_allow_spaces.push("local:mc".into());
        assert!(
            waiting_for(
                Some("local:mc"),
                25565,
                &plan(Route::Relay, false),
                &s,
                false
            )
            .is_none()
        );
        s.always_allow_spaces.clear();
        s.approve_agent_shares = false;
        assert!(
            waiting_for(
                Some("local:mc"),
                25565,
                &plan(Route::Host, false),
                &s,
                false
            )
            .is_none()
        );
    }

    #[test]
    fn the_first_edge_waits_for_consent_unless_given() {
        let s = ShareSettings::default();
        let w = waiting_for(None, 25565, &plan(Route::Edge, true), &s, false).unwrap();
        assert_eq!(w.reason, "consent");
        assert!(
            w.text
                .starts_with("This creates a small server in your AWS account (about $7 a month"),
            "{}",
            w.text
        );
        assert!(w.text.contains("Remove it any time."));
        assert!(w.text.contains("2 GB"), "the cap is named: {}", w.text);
        assert_eq!(w.route_line, "Through your AWS account");
        assert!(waiting_for(None, 25565, &plan(Route::Edge, true), &s, true).is_none());
        assert!(waiting_for(None, 25565, &plan(Route::Edge, false), &s, false).is_none());
    }

    #[test]
    fn the_daily_cap_splits_into_what_is_left() {
        let mut s = ShareSettings::default();
        let day = "2026-10-27";
        let cap = s.daily_cap_mib << 20;
        assert_eq!(budget(&s, &DailyUse::default(), day), Ok(cap));
        let used = DailyUse {
            day: day.into(),
            bytes: cap - 10,
        };
        assert_eq!(budget(&s, &used, day), Ok(10));
        let full = DailyUse {
            day: day.into(),
            bytes: cap,
        };
        assert!(budget(&s, &full, day).unwrap_err().contains("data cap"));
        // Yesterday's use does not count.
        assert_eq!(budget(&s, &full, "2026-10-28"), Ok(cap));
        s.daily_cap_mib = 0;
        assert_eq!(budget(&s, &full, day), Ok(0), "no cap");
    }

    #[test]
    fn landing_settings_pick_or_refuse_routes() {
        let mut s = ShareSettings::default();
        let local = SpaceLocation::Local;
        assert_eq!(
            choose(&local, &[], &s, RouteRequest::Auto).unwrap().route,
            Route::Relay
        );
        s.landing = Landing::OnlyMyCloud;
        assert!(choose(&local, &[], &s, RouteRequest::Auto).is_err());
        s.landing = Landing::Auto;
        s.relay_fallback = false;
        assert!(choose(&local, &[], &s, RouteRequest::Exactly(Route::Relay)).is_err());
    }
}
