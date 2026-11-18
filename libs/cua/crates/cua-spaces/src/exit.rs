//! Exits: where a Space's internet traffic leaves ("Exit through").
//!
//! A Space's network settings may name an exit ([`EgressVia`]): this
//! machine (what the hotspot gives a Space), another Space of its tree, or a
//! SOCKS5 proxy. The exit is the hotspot's machinery, kept running by this
//! process for every running Space whose settings name one:
//!
//! - **Closed by default.** With an exit, the Space's own direct internet is
//!   closed by its host ([`cua_vmm::network::compile`]). The only way out
//!   is the SOCKS5 listener on its loopback (`ALL_PROXY` and the other proxy
//!   variables point new processes at it), relayed to this process.
//! - **Rules first.** Every connection the exit makes for the Space passes
//!   the Space's own rules ([`ExitPolicy`], from
//!   [`NetworkSettings::exit_settings`]): platform denies, this computer's own
//!   addresses and the daemon, the user's lists and the preset (Internet only
//!   keeps the local network out). Only then is it dialed.
//! - **Fail closed.** When the exit goes away (this computer offline, the exit
//!   Space stopped, the proxy unreachable) connections fail: the Space has
//!   no internet, never an open path.
//! - **Inherited.** A child keeps its parent's exit and can't leave it
//!   ([`NetworkSettings::intersect`]).
//!
//! The hotspot (`hotspot_start`) is the same thing started by hand for one
//! run: "This computer" as the exit, until `hotspot_stop`. A hotspot started
//! by hand wins over a Space's saved exit until it stops.

use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use cua_vmm::egress::{Domains, Proto, Rules};
use cua_vmm::network::{EgressVia, Mode, NetworkContext, NetworkSettings};
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio_tungstenite::tungstenite::{self, client::IntoClientRequest};

use crate::error::{Error, Result};
use crate::hotspot::{Dialer, ExitStream, HotspotOptions, boxed};
use crate::space::Space;
use cua_spacesd_client::pb;

/// Where the topology hint goes in a guest, and where it goes when `/etc`
/// is not writable by the guest's daemon.
pub const TOPOLOGY_PATHS: [&str; 2] = ["/etc/cua/network.json", "/run/cua/network.json"];

/// What the exit checks before it dials: the Space's rules, as if it had
/// no exit, compiled for the computer that makes the connection (its own
/// addresses and the daemon closed), and its rules by name.
#[derive(Clone, Debug, Default)]
pub struct ExitPolicy {
    pub rules: Rules,
    pub domains: Domains,
}

impl ExitPolicy {
    /// The policy of a Space with `settings`.
    pub fn of(settings: &NetworkSettings) -> Result<ExitPolicy> {
        let (rules, domains) = cua_vmm::network::compile_named(
            &settings.exit_settings(),
            &exit_context(),
            Mode::Secure,
        )
        .map_err(|e| Error::invalid(e.0))?;
        Ok(ExitPolicy { rules, domains })
    }

    /// Internet only (the hotspot's default).
    pub fn default_internet() -> ExitPolicy {
        ExitPolicy::of(&NetworkSettings::internet()).unwrap_or_default()
    }

    /// Why a connection to `host:port` is refused, or `None`: a name rule
    /// first, then every address it resolves to (all must pass, so a name
    /// that points one answer at a private address is refused). Returns the
    /// addresses to dial.
    pub async fn admit(&self, host: &str, port: u16) -> std::io::Result<Vec<SocketAddr>> {
        let refused = |why: String| std::io::Error::new(std::io::ErrorKind::PermissionDenied, why);
        let named = host.parse::<IpAddr>().is_err();
        let mut name_allowed = false;
        if named {
            if self.domains.deny.iter().any(|p| name_matches(p, host)) {
                return Err(refused(format!("{host}: a name its rules block")));
            }
            name_allowed = self.domains.allow.iter().any(|p| name_matches(p, host));
        }
        let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host, port)).await?.collect();
        if addrs.is_empty() {
            return Err(refused(format!("{host}: no address")));
        }
        for a in &addrs {
            let ip = a.ip();
            match self.rules.decide(ip, Some(Proto::Tcp), Some(port)) {
                Some(r) if !r.accept => {
                    // An allowed name beats a default deny, never a
                    // platform or user deny (as the gateway does).
                    let default_only = r.reason.as_str().starts_with("default:");
                    if !(name_allowed && default_only) {
                        return Err(refused(format!(
                            "{host} ({ip}:{port}): {}",
                            r.reason.as_str()
                        )));
                    }
                }
                _ => {}
            }
        }
        Ok(addrs)
    }
}

/// What the computer making the exit's connections knows: its own addresses
/// and the daemon's ports, closed.
pub fn exit_context() -> NetworkContext {
    NetworkContext {
        host_addrs: cua_vmm::network::host_addresses(),
        daemon_ports: cua_vmm::network::daemon_ports(),
        ..Default::default()
    }
}

/// `pattern` (`example.com` or `*.example.com`) matches `name`.
fn name_matches(pattern: &str, name: &str) -> bool {
    let p = pattern.trim().trim_end_matches('.').to_ascii_lowercase();
    let n = name.trim().trim_end_matches('.').to_ascii_lowercase();
    match p.strip_prefix("*.") {
        Some(rest) => n.ends_with(&format!(".{rest}")),
        None => n == p,
    }
}

/// Dials from this computer, rules first.
pub fn this_machine(policy: Arc<ExitPolicy>) -> Dialer {
    Arc::new(move |host: String, port: u16| {
        let policy = policy.clone();
        Box::pin(async move {
            let addrs = policy.admit(&host, port).await?;
            // The checked addresses, not a second lookup: a name can't
            // answer differently between the check and the dial.
            let s = tokio::net::TcpStream::connect(&addrs[..]).await?;
            Ok(boxed(s))
        })
    })
}

/// Dials through the SOCKS5 proxy at `url`, rules first (checked by the
/// addresses the name resolves to here; the proxy resolves it again).
pub fn socks5(policy: Arc<ExitPolicy>, url: String) -> Result<Dialer> {
    let target = cua_vmm::network::socks5_target(&url).map_err(|e| Error::invalid(e.0))?;
    let target = Arc::new(target);
    Ok(Arc::new(move |host: String, port: u16| {
        let (policy, target) = (policy.clone(), target.clone());
        Box::pin(async move {
            policy.admit(&host, port).await?;
            let mut s = tokio::net::TcpStream::connect((target.host.as_str(), target.port)).await?;
            socks5_connect(
                &mut s,
                &host,
                port,
                target.user.as_deref(),
                target.pass.as_deref(),
            )
            .await?;
            Ok(boxed(s))
        })
    }))
}

/// Dials through the exit of Space `exit` (its `StartExit` listener,
/// reached over its tunnel), rules first. The exit Space's own rules apply
/// there too: its host enforces them on what it dials.
pub fn through_space(spaces: crate::Spaces, exit: String, policy: Arc<ExitPolicy>) -> Dialer {
    let port: Arc<tokio::sync::Mutex<Option<u16>>> = Arc::default();
    Arc::new(move |host: String, dport: u16| {
        let (spaces, exit, policy, port) =
            (spaces.clone(), exit.clone(), policy.clone(), port.clone());
        Box::pin(async move {
            policy.admit(&host, dport).await?;
            let io = |e: Error| std::io::Error::other(e.to_string());
            let space = spaces.space(&exit).await.map_err(io)?;
            for attempt in 0..2 {
                let p = {
                    let mut cached = port.lock().await;
                    match *cached {
                        Some(p) if attempt == 0 => p,
                        _ => {
                            let p = space.start_exit().await.map_err(io)?;
                            *cached = Some(p);
                            p
                        }
                    }
                };
                let mut s = match space.open_loopback(p).await {
                    Ok(s) => s,
                    Err(_) if attempt == 0 => continue,
                    Err(e) => return Err(io(e)),
                };
                match socks5_connect(&mut s, &host, dport, None, None).await {
                    Ok(()) => return Ok(s),
                    // The exit's daemon restarted (a new listener): ask
                    // again once.
                    Err(_) if attempt == 0 => continue,
                    Err(e) => return Err(e),
                }
            }
            Err(std::io::Error::other("the exit Space did not answer"))
        })
    })
}

/// A SOCKS5 `CONNECT` to `host:port` over `s` (no authentication, or
/// username and password).
pub async fn socks5_connect<S: AsyncRead + AsyncWrite + Unpin + ?Sized>(
    s: &mut S,
    host: &str,
    port: u16,
    user: Option<&str>,
    pass: Option<&str>,
) -> std::io::Result<()> {
    let bad = |why: &str| std::io::Error::other(format!("socks5: {why}"));
    let creds = user.is_some();
    if creds {
        s.write_all(&[5, 2, 0, 2]).await?;
    } else {
        s.write_all(&[5, 1, 0]).await?;
    }
    let mut hello = [0u8; 2];
    s.read_exact(&mut hello).await?;
    if hello[0] != 5 {
        return Err(bad("not a SOCKS5 proxy"));
    }
    match hello[1] {
        0 => {}
        2 if creds => {
            let (u, p) = (user.unwrap_or_default(), pass.unwrap_or_default());
            if u.len() > 255 || p.len() > 255 {
                return Err(bad("username or password too long"));
            }
            let mut auth = vec![1, u.len() as u8];
            auth.extend_from_slice(u.as_bytes());
            auth.push(p.len() as u8);
            auth.extend_from_slice(p.as_bytes());
            s.write_all(&auth).await?;
            let mut ok = [0u8; 2];
            s.read_exact(&mut ok).await?;
            if ok[1] != 0 {
                return Err(bad("the proxy refused the username and password"));
            }
        }
        _ => {
            return Err(bad(
                "the proxy wants an authentication this exit doesn't offer",
            ));
        }
    }
    let mut req = vec![5, 1, 0];
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => {
            req.push(1);
            req.extend_from_slice(&ip.octets());
        }
        Ok(IpAddr::V6(ip)) => {
            req.push(4);
            req.extend_from_slice(&ip.octets());
        }
        Err(_) => {
            if host.len() > 255 {
                return Err(bad("name too long"));
            }
            req.push(3);
            req.push(host.len() as u8);
            req.extend_from_slice(host.as_bytes());
        }
    }
    req.extend_from_slice(&port.to_be_bytes());
    s.write_all(&req).await?;
    let mut head = [0u8; 4];
    s.read_exact(&mut head).await?;
    if head[1] != 0 {
        return Err(bad(&format!(
            "the proxy refused the connection (code {})",
            head[1]
        )));
    }
    let skip = match head[3] {
        1 => 4,
        4 => 16,
        3 => {
            let mut len = [0u8; 1];
            s.read_exact(&mut len).await?;
            usize::from(len[0])
        }
        _ => return Err(bad("an answer this exit can't read")),
    };
    let mut rest = vec![0u8; skip + 2];
    s.read_exact(&mut rest).await?;
    Ok(())
}

impl Space {
    /// `TunnelService.StartExit`: this Space as an exit of its tree; the
    /// port of its SOCKS5 listener on its loopback.
    pub async fn start_exit(&self) -> Result<u16> {
        self.require("exit")?;
        let resp = self
            .spacesd()?
            .tunnel()
            .start_exit(pb::StartExitRequest { socks_port: 0 })
            .await
            .map_err(cua_spacesd_client::Error::from)?
            .into_inner();
        resp.socks_address
            .parse::<SocketAddr>()
            .map(|a| a.port())
            .map_err(|_| Error::Stream(format!("exit address {:?}", resp.socks_address)))
    }

    /// `TunnelService.StopExit`.
    pub async fn stop_exit(&self) -> Result<()> {
        self.spacesd()?
            .tunnel()
            .stop_exit(pb::StopExitRequest {})
            .await
            .map_err(cua_spacesd_client::Error::from)?;
        Ok(())
    }

    /// A TCP connection to `port` on this Space's loopback, over its tunnel
    /// (`TunnelService.Forward`).
    pub async fn open_loopback(&self, port: u16) -> Result<ExitStream> {
        let resp = self
            .spacesd()?
            .tunnel()
            .forward(pb::ForwardRequest {
                port: u32::from(port),
                host: "127.0.0.1".into(),
                ttl: None,
            })
            .await
            .map_err(cua_spacesd_client::Error::from)?
            .into_inner();
        let mut request = self
            .websocket_url(&resp.ws_path)?
            .into_client_request()
            .map_err(|e| Error::Stream(e.to_string()))?;
        for (k, v) in self.websocket_headers().await? {
            if let (Ok(k), Ok(v)) = (
                http::HeaderName::from_bytes(k.as_bytes()),
                http::HeaderValue::from_str(&v),
            ) {
                request.headers_mut().insert(k, v);
            }
        }
        cua_spacesd_client::transport::ensure_crypto_provider();
        let (ws, _) = tokio_tungstenite::connect_async(request)
            .await
            .map_err(|e| Error::Stream(format!("tunnel socket: {e}")))?;
        Ok(Box::new(ws_stream(ws)))
    }
}

/// A WebSocket of binary frames as a byte stream.
fn ws_stream<S>(ws: tokio_tungstenite::WebSocketStream<S>) -> tokio::io::DuplexStream
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (ours, theirs) = tokio::io::duplex(256 * 1024);
    let (mut sink, mut source) = ws.split();
    let (mut rd, mut wr) = tokio::io::split(theirs);
    tokio::spawn(async move {
        let up = async {
            let mut buf = vec![0u8; 32 * 1024];
            loop {
                let n = match rd.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                if sink
                    .send(tungstenite::Message::Binary(buf[..n].to_vec().into()))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            let _ = sink.close().await;
        };
        let down = async {
            while let Some(Ok(m)) = source.next().await {
                match m {
                    tungstenite::Message::Binary(b) => {
                        if wr.write_all(&b).await.is_err() {
                            break;
                        }
                    }
                    tungstenite::Message::Close(_) => break,
                    _ => {}
                }
            }
            let _ = wr.shutdown().await;
        };
        tokio::join!(up, down);
    });
    ours
}

/// The exit a running Space has now.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct ExitStatus {
    /// Space id.
    pub space: String,
    /// `this-computer`, `space:<id>` or the proxy (credentials left out).
    pub via: String,
    /// One plain line ("This computer").
    pub label: String,
    /// `active`, `waiting_for_peer`, `starting` or `unreachable`.
    pub state: String,
    /// Why it isn't active, in plain words.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// The `(address, name)` of every other member of the tree in a topology
/// hint that has a tree address, for the guest's hosts file.
pub fn hosts_entries(topology: &serde_json::Value) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = topology["tree"]["members"]
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        .filter(|m| m["relation"] != "self")
        .filter_map(|m| {
            let addr = m["address"].as_str()?;
            let name = m["name"].as_str()?;
            addr.parse::<std::net::Ipv4Addr>().ok()?;
            // Only DNS labels and dots reach a shell line.
            name.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
                .then(|| (addr.to_string(), name.to_string()))
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

/// The shell script that makes the hosts file of a guest list `entries`
/// between two marker lines (replacing an earlier block; without entries,
/// removing it). The file is rewritten in place, so a bind-mounted
/// `/etc/hosts` stays one.
pub fn hosts_script(entries: &[(String, String)]) -> String {
    let mut s = String::from(
        "f=/etc/hosts; t=$(mktemp) || exit 1; \
         sed '/^# cua-tree begin$/,/^# cua-tree end$/d' \"$f\" > \"$t\" || exit 1; ",
    );
    if !entries.is_empty() {
        s.push_str("{ printf '%s\\n' '# cua-tree begin'; ");
        for (addr, name) in entries {
            s.push_str(&format!("printf '%s %s\\n' '{addr}' '{name}'; "));
        }
        s.push_str("printf '%s\\n' '# cua-tree end'; } >> \"$t\"; ");
    }
    s.push_str("cat \"$t\" > \"$f\"; rm -f \"$t\"");
    s
}

/// A running exit, as this process keeps it.
#[derive(Clone, Debug)]
pub(crate) struct ExitRun {
    via: EgressVia,
    note: Option<String>,
}

/// The exits this process keeps (`Spaces::exits`).
pub(crate) type ExitTable = tokio::sync::Mutex<BTreeMap<String, ExitRun>>;

/// The destinations that keep the Space's own network next to an exit: its
/// tree, its loopback and this computer's name for the host.
pub fn exit_bypass(tree_suffix: Option<&str>) -> Vec<String> {
    let mut v: Vec<String> = vec![
        cua_vmm::tree::RANGE.into(),
        "127.0.0.0/8".into(),
        "localhost".into(),
        crate::topology::HOST_NAME.into(),
    ];
    if let Some(t) = tree_suffix {
        v.push(t.to_string());
    }
    v
}

/// What a Space's saved exit says in plain words, credentials left out.
fn via_word_public(v: &EgressVia) -> String {
    match v {
        EgressVia::Socks5(url) => match cua_vmm::network::socks5_target(url) {
            Ok(t) => format!(
                "{}://{}:{}",
                url.split("://").next().unwrap_or("socks5"),
                t.host,
                t.port
            ),
            Err(_) => "socks5".into(),
        },
        other => other.as_word(),
    }
}

impl crate::Spaces {
    /// A Space's exit changed (or might have): start, change or stop it in
    /// the background.
    pub(crate) fn exit_changed(&self, key: &str) {
        let (spaces, key) = (self.clone(), key.to_string());
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            rt.spawn(async move {
                if let Err(e) = spaces.reconcile_exit(&key).await {
                    tracing::warn!(space = %key, error = %e, "exit");
                }
                if let Err(e) = spaces.reconcile_host_ports(&key).await {
                    tracing::warn!(space = %key, error = %e, "host ports");
                }
            });
        }
    }

    /// Brings every Space's exit and published host ports in line with its
    /// saved settings. The daemon calls it periodically (and a Space that
    /// starts, or whose guest daemon restarted, is caught up here).
    pub async fn reconcile_network(&self) {
        self.reconcile_exits().await;
        // Members on other computers first: the hub and the topology below
        // include them.
        self.reconcile_overlay().await;
        self.reconcile_tree_hub().await;
        self.write_topologies().await;
        let mut keys: Vec<String> = self
            .inner
            .registry
            .all_meta()
            .into_iter()
            .filter(|(k, m)| {
                k.starts_with("local:")
                    && m.network
                        .as_ref()
                        .is_some_and(|n| !crate::host_ports::published_ports(n).is_empty())
            })
            .map(|(k, _)| k)
            .collect();
        // Containers that get the tree proxy.
        let linked = self.linked_trees();
        for (k, m) in self.inner.registry.all_meta() {
            if !k.starts_with("local:") || keys.contains(&k) {
                continue;
            }
            let tree = self.book_tree_of(&k);
            let running = cua_vmm::tree::running_members(&tree);
            let settings = crate::network::settings_of(Some(&m));
            if self.is_running(&k)
                && crate::tree_proxy::wanted(&settings, &running, linked.contains(&tree))
            {
                keys.push(k);
            }
        }
        keys.extend(self.inner.host_ports.lock().await.keys().cloned());
        keys.sort();
        keys.dedup();
        for key in keys {
            if let Err(e) = self.reconcile_host_ports(&key).await {
                tracing::debug!(space = %key, error = %e, "host ports");
            }
        }
    }

    /// Brings every exit in line with the saved settings: running Spaces
    /// with an exit get one, the others lose theirs. The daemon calls it
    /// periodically; changes call [`Self::exit_changed`] at once.
    pub async fn reconcile_exits(&self) {
        let mut keys: Vec<String> = self
            .inner
            .registry
            .all_meta()
            .into_iter()
            .filter(|(k, m)| {
                k.starts_with("local:") && m.network.as_ref().is_some_and(|n| n.via.is_some())
            })
            .map(|(k, _)| k)
            .collect();
        keys.extend(self.inner.exits.lock().await.keys().cloned());
        keys.sort();
        keys.dedup();
        for key in keys {
            if let Err(e) = self.reconcile_exit(&key).await {
                tracing::debug!(space = %key, error = %e, "exit");
            }
        }
    }

    /// [`Self::reconcile_exits`] for one Space.
    pub async fn reconcile_exit(&self, key: &str) -> Result<()> {
        let settings = self
            .inner
            .registry
            .meta(key)
            .and_then(|m| m.network.clone());
        let via = settings.as_ref().and_then(|s| s.via.clone());
        let running = self.is_running(key);
        let current = self.inner.exits.lock().await.get(key).cloned();
        let (Some(via), true) = (via, running) else {
            if current.is_some() {
                self.inner.exits.lock().await.remove(key);
                let _ = self.hotspot_stop(Some(key)).await;
                tracing::info!(space = %key, "exit stopped");
            }
            return Ok(());
        };
        let settings = settings.unwrap_or_default();
        // A hotspot someone started by hand wins until it stops.
        let manual = self.inner.hotspots.lock().await.contains_key(key) && current.is_none();
        if manual {
            return Ok(());
        }
        let alive = self
            .inner
            .hotspots
            .lock()
            .await
            .get(key)
            .is_some_and(|h| h.is_running());
        let same = current.as_ref().is_some_and(|c| c.via == via);
        if !(same && alive) {
            let policy = Arc::new(ExitPolicy::of(&settings)?);
            let dialer = match &via {
                EgressVia::ThisMachine => this_machine(policy),
                EgressVia::Socks5(url) => socks5(policy, url.clone())?,
                EgressVia::Space(id) => through_space(self.clone(), id.clone(), policy),
            };
            let space = self.space(key).await?;
            let opts = HotspotOptions {
                socks_port: 0,
                set_system_proxy: true,
                bypass: exit_bypass(self.tree_suffix(key).as_deref()),
                dialer,
            };
            let note = if !space.supports("hotspot") {
                // Fail closed: its direct internet stays shut, and nothing
                // carries it out. Say so.
                Some(format!(
                    "This Space can't use {}: its image has no Cua exit support, so it has no \
                     internet. Use an image with cua-spacesd, or remove the exit.",
                    via.label()
                ))
            } else {
                match self.hotspot_start(key, opts).await {
                    Ok(_) => None,
                    Err(e) => Some(format!(
                        "{} can't carry this Space's traffic right now ({e}); it has no internet \
                         until it can.",
                        via.label()
                    )),
                }
            };
            self.inner.exits.lock().await.insert(
                key.to_string(),
                ExitRun {
                    via: via.clone(),
                    note,
                },
            );
            tracing::info!(space = %key, via = %via_word_public(&via), "exit started");
        }
        self.write_topology(key).await;
        Ok(())
    }

    /// The exits of the running Spaces (the indicator on this computer).
    pub async fn exits(&self) -> Vec<ExitStatus> {
        let runs: Vec<(String, ExitRun)> = self
            .inner
            .exits
            .lock()
            .await
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let mut out = Vec::new();
        for (space, run) in runs {
            let state = match &run.note {
                Some(_) => "unreachable".to_string(),
                None => match self.hotspot_statuses(Some(&space)).await {
                    Ok(s) => s
                        .first()
                        .map(|s| s.state.clone())
                        .unwrap_or_else(|| "starting".into()),
                    Err(_) => "starting".into(),
                },
            };
            out.push(ExitStatus {
                via: via_word_public(&run.via),
                label: run.via.label(),
                state,
                note: run.note.clone(),
                space,
            });
        }
        out
    }

    /// Writes the topology hint into Space `key` when it changed, and, for
    /// a Linux VM, the names of its tree's members into its `/etc/hosts`.
    async fn write_topology(&self, key: &str) {
        let Ok(value) = self.topology(key).await else {
            return;
        };
        let text = serde_json::to_string_pretty(&value).unwrap_or_default();
        let entries = hosts_entries(&value);
        let hash = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            text.hash(&mut h);
            entries.hash(&mut h);
            h.finish()
        };
        if self.inner.topologies.lock().await.get(key) == Some(&hash) {
            return;
        }
        let Ok(space) = self.space(key).await else {
            return;
        };
        let mut written = false;
        for path in TOPOLOGY_PATHS {
            if space.write(path, text.clone().into_bytes()).await.is_ok() {
                written = true;
                break;
            }
        }
        // Linux VMs have no gateway resolver answering tree names (macOS
        // and Windows VMs do, containers use their tree's network): the
        // names go into the guest's own hosts file.
        if written && self.is_linux_vm(key, &space).await {
            let script = hosts_script(&entries);
            match space
                .bash(&script, std::time::Duration::from_secs(20))
                .await
            {
                Ok(out) if out.success() => {}
                Ok(out) => {
                    tracing::debug!(space = %key, out = %out.render(), "tree names in /etc/hosts")
                }
                Err(e) => tracing::debug!(space = %key, error = %e, "tree names in /etc/hosts"),
            }
        }
        if written {
            self.inner
                .topologies
                .lock()
                .await
                .insert(key.to_string(), hash);
        }
    }

    /// Writes the topology hint into every running Space on this computer
    /// whose hint changed (a member came or went, its settings changed).
    async fn write_topologies(&self) {
        let keys: Vec<String> = self
            .inner
            .registry
            .all_meta()
            .into_keys()
            .filter(|k| k.starts_with("local:") && self.is_running(k))
            .collect();
        self.inner
            .topologies
            .lock()
            .await
            .retain(|k, _| keys.contains(k));
        for key in keys {
            self.write_topology(&key).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_match_exactly_or_by_suffix() {
        assert!(name_matches("pypi.org", "PyPI.org."));
        assert!(name_matches("*.github.com", "api.github.com"));
        assert!(!name_matches("*.github.com", "github.com"));
        assert!(!name_matches("pypi.org", "evilpypi.org"));
    }

    #[tokio::test]
    async fn the_exit_applies_the_spaces_rules_before_it_dials() {
        let p = ExitPolicy::default_internet();
        // Literal addresses resolve to themselves: no network needed.
        assert!(p.admit("1.1.1.1", 443).await.is_ok());
        for blocked in ["127.0.0.1", "169.254.169.254", "192.168.1.20", "10.0.0.5"] {
            let e = p.admit(blocked, 80).await.unwrap_err();
            assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied, "{blocked}");
        }
        // A Space that allows its local network gets it at the exit too.
        let lan = ExitPolicy::of(&NetworkSettings::preset(
            cua_vmm::network::NetworkPreset::Lan,
        ))
        .unwrap();
        assert!(lan.admit("192.168.1.20", 80).await.is_ok());
        assert!(
            lan.admit("169.254.169.254", 80).await.is_err(),
            "metadata never"
        );
    }

    #[test]
    fn linux_vms_get_tree_names_in_their_hosts_file() {
        let t = serde_json::json!({"tree": {"members": [
            {"name": "me.app", "address": "198.19.0.1", "relation": "self"},
            {"name": "db.app", "address": "198.19.0.2", "relation": "member"},
            {"name": "worker.app", "address": "198.19.7.1", "relation": "member", "machine": "studio"},
            {"name": "web.app", "relation": "member"},
            {"name": "evil'; rm -rf /", "address": "198.19.0.9", "relation": "member"},
            {"name": "odd.app", "address": "not an address", "relation": "member"},
        ]}});
        let e = hosts_entries(&t);
        assert_eq!(
            e,
            vec![
                ("198.19.0.2".to_string(), "db.app".to_string()),
                ("198.19.7.1".to_string(), "worker.app".to_string()),
            ],
            "other members with an address and a plain name"
        );
        let s = hosts_script(&e);
        assert!(s.contains("# cua-tree begin"));
        assert!(
            s.contains("printf '%s %s\\n' '198.19.7.1' 'worker.app'"),
            "{s}"
        );
        assert!(s.contains("cat \"$t\" > \"$f\""), "in place: {s}");
        // No members left: the block is removed, nothing is added.
        let none = hosts_script(&[]);
        assert!(none.contains("sed '/^# cua-tree begin$/,/^# cua-tree end$/d'"));
        assert!(!none.contains("printf"), "{none}");
    }

    #[test]
    fn the_bypass_keeps_the_tree_and_the_host_direct() {
        let b = exit_bypass(Some("research"));
        assert!(b.contains(&"198.19.0.0/16".to_string()));
        assert!(b.contains(&"host".to_string()));
        assert!(b.contains(&"research".to_string()));
    }

    /// A SOCKS5 server that accepts one CONNECT and echoes.
    async fn fake_socks(expect_auth: bool) -> (SocketAddr, tokio::task::JoinHandle<Vec<u8>>) {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (mut c, _) = l.accept().await.unwrap();
            let mut hello = [0u8; 2];
            c.read_exact(&mut hello).await.unwrap();
            let mut methods = vec![0u8; usize::from(hello[1])];
            c.read_exact(&mut methods).await.unwrap();
            if expect_auth {
                c.write_all(&[5, 2]).await.unwrap();
                let mut v = [0u8; 2];
                c.read_exact(&mut v).await.unwrap();
                let mut u = vec![0u8; usize::from(v[1])];
                c.read_exact(&mut u).await.unwrap();
                let mut pl = [0u8; 1];
                c.read_exact(&mut pl).await.unwrap();
                let mut p = vec![0u8; usize::from(pl[0])];
                c.read_exact(&mut p).await.unwrap();
                c.write_all(&[1, 0]).await.unwrap();
            } else {
                c.write_all(&[5, 0]).await.unwrap();
            }
            let mut head = [0u8; 5];
            c.read_exact(&mut head).await.unwrap();
            let mut name = vec![0u8; usize::from(head[4]) + 2];
            c.read_exact(&mut name).await.unwrap();
            c.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).await.unwrap();
            name
        });
        (addr, task)
    }

    #[tokio::test]
    async fn socks5_connect_speaks_the_protocol() {
        let (addr, task) = fake_socks(false).await;
        let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
        socks5_connect(&mut s, "example.com", 443, None, None)
            .await
            .unwrap();
        let name = task.await.unwrap();
        assert_eq!(&name[..11], b"example.com");
        assert_eq!(&name[11..], &443u16.to_be_bytes());
        let (addr, task) = fake_socks(true).await;
        let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
        socks5_connect(&mut s, "example.com", 80, Some("u"), Some("p"))
            .await
            .unwrap();
        task.await.unwrap();
    }
}
