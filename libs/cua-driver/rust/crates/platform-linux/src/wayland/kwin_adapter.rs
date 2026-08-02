//! Trusted KDE Plasma/KWin Wayland window identity and activation adapter.
//!
//! KWin does not expose wlroots foreign-toplevel activation.  Its supported
//! privileged window API is the KWin JavaScript environment, and scripts can
//! only make outbound D-Bus calls.  For each request we therefore load a
//! bounded one-shot script into the *verified KWin process*.  The script calls
//! directly back to a short-lived object on this process's unique bus name.
//! The callback accepts only the immutable KWin unique name pinned at the
//! beginning of the transaction.
//!
//! Titles, PIDs, and geometry are correlation metadata.  The only compositor
//! identity is KWin's complete `Window.internalId` UUID.  Public numeric window
//! IDs are collision-checked handles bound to one KWin/package incarnation;
//! the UUID is retained in both directions and stale handles are tombstoned.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use zbus::blocking::{fdo::DBusProxy, Connection, Proxy};
use zbus::message::Header;
use zbus::names::BusName;

use crate::x11::WindowInfo;

pub const PROTOCOL_VERSION: u32 = 1;
pub const INSTALL_COMMAND: &str = "~/.cua-driver/packages/current/wayland-helper/kwin/install.sh";
pub const PLUGIN_ID: &str = "cua-kwin.cua";

const KWIN_DEST: &str = "org.kde.KWin";
const SCRIPTING_PATH: &str = "/Scripting";
const SCRIPTING_IFACE: &str = "org.kde.kwin.Scripting";
const SCRIPT_IFACE: &str = "org.kde.kwin.Script";
const CALLBACK_PATH: &str = "/org/cua/KWinAdapter/Callback";
const CALLBACK_IFACE: &str = "org.cua.KWinAdapter.Callback";
const RESPONSE_LIMIT: usize = 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);
const KWIN_ID_NAMESPACE: u32 = 0xE000_0000;
const KWIN_ID_MASK: u32 = 0x0FFF_FFFF;

const BUNDLED_SOURCE: &str =
    include_str!("../../../../../wayland-helper/kwin/cua-kwin@cua/contents/code/main.js");

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AdapterDiagnostic {
    pub installed: bool,
    pub protocol_supported: bool,
    pub owner_trusted: bool,
    pub enumeration_available: bool,
    pub activation_verified: bool,
    pub message: String,
}

impl AdapterDiagnostic {
    pub fn fully_available(&self) -> bool {
        self.installed
            && self.protocol_supported
            && self.owner_trusted
            && self.enumeration_available
            && self.activation_verified
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct KWinWindow {
    uuid: String,
    pid: u32,
    title: String,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    active: bool,
    minimized: bool,
    hidden: bool,
    visible: bool,
    stacking: usize,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    protocol_version: u32,
    active_window_uuid: Option<String>,
    windows: Vec<KWinWindow>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ActivationReply {
    protocol_version: u32,
    accepted: bool,
    target_uuid: String,
}

struct TrustedSession {
    owner: String,
    pid: u32,
    instance: String,
    source: &'static str,
    // The bus-provided pidfd, when supported, pins the process across the
    // complete focus transaction and closes PID-reuse races.
    _process_fd: Option<OwnedFd>,
}

#[derive(Clone, Debug)]
struct VerifiedGuard {
    pid: u32,
    numeric_id: u32,
}

thread_local! {
    static VERIFIED_GUARD: RefCell<Option<VerifiedGuard>> = const { RefCell::new(None) };
}

/// Whether the current thread is executing inside a freshly verified KWin
/// focus transaction.  Targetless portal operations are allowed only inside
/// this scope on KDE.
pub fn verified_focus_guard_active() -> bool {
    VERIFIED_GUARD.with(|guard| guard.borrow().is_some())
}

pub fn verified_focus_guard_matches(pid: u32, window_id: u64) -> bool {
    let Ok(window_id) = u32::try_from(window_id) else {
        return false;
    };
    VERIFIED_GUARD.with(|guard| {
        guard
            .borrow()
            .as_ref()
            .is_some_and(|guard| guard.pid == pid && guard.numeric_id == window_id)
    })
}

fn with_verified_guard<T>(
    guard: VerifiedGuard,
    body: impl FnOnce() -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    VERIFIED_GUARD.with(|cell| {
        let previous = cell.replace(Some(guard));
        let result = body();
        cell.replace(previous);
        result
    })
}

pub fn is_kde_wayland_session() -> bool {
    detect_kde_wayland(
        std::env::var("XDG_SESSION_TYPE").ok().as_deref(),
        std::env::var("XDG_CURRENT_DESKTOP").ok().as_deref(),
        std::env::var_os("WAYLAND_DISPLAY").is_some(),
    )
}

fn detect_kde_wayland(
    session_type: Option<&str>,
    current_desktop: Option<&str>,
    has_wayland_display: bool,
) -> bool {
    has_wayland_display
        && session_type.is_some_and(|value| value.eq_ignore_ascii_case("wayland"))
        && current_desktop.is_some_and(|value| {
            value
                .split([':', ';'])
                .any(|part| part.trim().eq_ignore_ascii_case("kde"))
        })
}

#[derive(Debug)]
struct InstalledAdapter {
    source_hash: String,
}

fn data_home() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
}

fn installed_package_root() -> Option<PathBuf> {
    Some(data_home()?.join("kwin/scripts").join(PLUGIN_ID))
}

fn secure_regular_file(path: &Path, expected_uid: u32) -> anyhow::Result<()> {
    let metadata = std::fs::metadata(path)
        .with_context(|| format!("kwin_adapter_missing: {} is not installed", path.display()))?;
    if !metadata.is_file() {
        anyhow::bail!(
            "kwin_adapter_untrusted: {} is not a regular file",
            path.display()
        );
    }
    if metadata.uid() != expected_uid && metadata.uid() != 0 {
        anyhow::bail!(
            "kwin_adapter_untrusted: {} is owned by uid {}",
            path.display(),
            metadata.uid()
        );
    }
    if metadata.permissions().mode() & 0o022 != 0 {
        anyhow::bail!(
            "kwin_adapter_untrusted: {} is group/world writable",
            path.display()
        );
    }
    Ok(())
}

fn installed_adapter() -> anyhow::Result<InstalledAdapter> {
    let root = installed_package_root().ok_or_else(|| {
        anyhow!(
            "kwin_adapter_missing: HOME/XDG_DATA_HOME does not identify the user data directory"
        )
    })?;
    let source_path = root.join("contents/code/main.js");
    let metadata_path = root.join("metadata.json");
    let uid = current_uid();
    secure_regular_file(&source_path, uid)?;
    secure_regular_file(&metadata_path, uid)?;

    let source = std::fs::read_to_string(&source_path).with_context(|| {
        format!(
            "kwin_adapter_missing: could not read {}",
            source_path.display()
        )
    })?;
    if source != BUNDLED_SOURCE {
        anyhow::bail!(
            "kwin_adapter_outdated: installed KWin adapter source does not match this cua-driver build"
        );
    }
    let metadata: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&metadata_path)
            .with_context(|| format!("could not read {}", metadata_path.display()))?,
    )
    .context("kwin_adapter_malformed: metadata.json is invalid")?;
    let plugin_id = metadata
        .pointer("/KPlugin/Id")
        .and_then(serde_json::Value::as_str);
    let protocol = metadata
        .get("X-Cua-Protocol-Version")
        .and_then(serde_json::Value::as_u64);
    if plugin_id != Some(PLUGIN_ID) || protocol != Some(u64::from(PROTOCOL_VERSION)) {
        anyhow::bail!(
            "kwin_adapter_outdated: installed metadata has an unsupported id or protocol version"
        );
    }

    Ok(InstalledAdapter {
        source_hash: hex_digest(source.as_bytes()),
    })
}

fn current_uid() -> u32 {
    std::fs::metadata("/proc/self")
        .map(|metadata| metadata.uid())
        .unwrap_or(u32::MAX)
}

#[derive(Clone, Debug)]
struct OwnerFacts {
    unique_name: String,
    pid: u32,
    uid: u32,
    comm: String,
    argv0: Option<PathBuf>,
    argv0_uid: Option<u32>,
    argv0_mode: Option<u32>,
    executable: Option<PathBuf>,
    executable_uid: Option<u32>,
    executable_mode: Option<u32>,
    cgroup: String,
    parent_argv0: Option<PathBuf>,
    parent_executable_uid: Option<u32>,
    parent_executable_mode: Option<u32>,
    start_time: u64,
}

fn validate_secure_binary(
    path: Option<&Path>,
    uid: Option<u32>,
    mode: Option<u32>,
    expected_name: &str,
) -> bool {
    path.and_then(Path::file_name)
        .and_then(|name| name.to_str())
        == Some(expected_name)
        && uid == Some(0)
        && mode.is_some_and(|mode| mode & 0o022 == 0)
}

fn validate_owner_facts(facts: &OwnerFacts, expected_uid: u32) -> anyhow::Result<()> {
    if !facts.unique_name.starts_with(':') {
        anyhow::bail!("kwin_adapter_untrusted_owner: KWin has no immutable unique bus name");
    }
    if facts.uid != expected_uid {
        anyhow::bail!(
            "kwin_adapter_wrong_uid: KWin bus owner uid {} does not match current uid {expected_uid}",
            facts.uid
        );
    }
    if facts.pid == 0 || facts.comm.trim() != "kwin_wayland" || facts.start_time == 0 {
        anyhow::bail!("kwin_adapter_wrong_process: bus owner is not kwin_wayland");
    }

    let direct = validate_secure_binary(
        facts.executable.as_deref(),
        facts.executable_uid,
        facts.executable_mode,
        "kwin_wayland",
    );
    let argv_binary = validate_secure_binary(
        facts.argv0.as_deref(),
        facts.argv0_uid,
        facts.argv0_mode,
        "kwin_wayland",
    );
    let systemd_chain = argv_binary
        && facts
            .cgroup
            .lines()
            .any(|line| line.ends_with("/plasma-kwin_wayland.service"))
        && validate_secure_binary(
            facts.parent_argv0.as_deref(),
            facts.parent_executable_uid,
            facts.parent_executable_mode,
            "kwin_wayland_wrapper",
        );
    if !direct && !systemd_chain {
        anyhow::bail!(
            "kwin_adapter_wrong_executable: KWin executable ownership and systemd wrapper identity could not be proven"
        );
    }
    Ok(())
}

fn proc_stat_fields(pid: u32) -> Option<(u32, u64)> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let (_, fields) = stat.rsplit_once(") ")?;
    let fields = fields.split_whitespace().collect::<Vec<_>>();
    // fields[0] is state (field 3), fields[1] is ppid (field 4), and
    // fields[19] is starttime (field 22).
    Some((fields.get(1)?.parse().ok()?, fields.get(19)?.parse().ok()?))
}

fn argv0(pid: u32) -> Option<PathBuf> {
    let bytes = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let first = bytes.split(|byte| *byte == 0).next()?;
    (!first.is_empty()).then(|| PathBuf::from(std::ffi::OsStr::from_bytes(first)))
}

use std::os::unix::ffi::OsStrExt;

fn binary_metadata(path: Option<&Path>) -> (Option<u32>, Option<u32>) {
    path.and_then(|path| std::fs::metadata(path).ok())
        .map(|metadata| (Some(metadata.uid()), Some(metadata.permissions().mode())))
        .unwrap_or((None, None))
}

fn owner_facts(owner: &str, pid: u32, uid: u32) -> anyhow::Result<OwnerFacts> {
    let executable = std::fs::read_link(format!("/proc/{pid}/exe")).ok();
    let (executable_uid, executable_mode) = binary_metadata(executable.as_deref());
    let process_argv0 = argv0(pid);
    let (argv0_uid, argv0_mode) = binary_metadata(process_argv0.as_deref());
    let (parent_pid, start_time) = proc_stat_fields(pid).ok_or_else(|| {
        anyhow!("kwin_adapter_wrong_process: could not read KWin process identity")
    })?;
    let parent_argv0 = argv0(parent_pid);
    let (parent_executable_uid, parent_executable_mode) = binary_metadata(parent_argv0.as_deref());
    Ok(OwnerFacts {
        unique_name: owner.to_owned(),
        pid,
        uid,
        comm: std::fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default(),
        argv0: process_argv0,
        argv0_uid,
        argv0_mode,
        executable,
        executable_uid,
        executable_mode,
        cgroup: std::fs::read_to_string(format!("/proc/{pid}/cgroup")).unwrap_or_default(),
        parent_argv0,
        parent_executable_uid,
        parent_executable_mode,
        start_time,
    })
}

fn trusted_session_on(connection: &Connection) -> anyhow::Result<TrustedSession> {
    let installed = installed_adapter()?;
    let dbus = DBusProxy::new(connection).context("kwin_adapter_unavailable: session bus")?;
    let owner = dbus
        .get_name_owner(BusName::try_from(KWIN_DEST)?)
        .context("kwin_adapter_missing: org.kde.KWin has no owner")?;
    let owner_name = owner.to_string();
    let owner_bus = BusName::try_from(owner_name.as_str())?;
    let credentials = dbus
        .get_connection_credentials(owner_bus.clone())
        .context("kwin_adapter_untrusted_owner: could not obtain KWin D-Bus credentials")?;
    let pid = credentials
        .process_id()
        .or_else(|| dbus.get_connection_unix_process_id(owner_bus.clone()).ok())
        .ok_or_else(|| anyhow!("kwin_adapter_untrusted_owner: KWin PID is unavailable"))?;
    let uid = credentials
        .unix_user_id()
        .or_else(|| dbus.get_connection_unix_user(owner_bus).ok())
        .ok_or_else(|| anyhow!("kwin_adapter_untrusted_owner: KWin UID is unavailable"))?;
    let facts = owner_facts(&owner_name, pid, uid)?;
    validate_owner_facts(&facts, current_uid())?;
    let process_fd = credentials
        .process_fd()
        .and_then(|fd| fd.as_fd().try_clone_to_owned().ok());
    let instance = hex_digest(
        format!(
            "{}\0{}\0{}\0{}",
            owner_name, facts.start_time, PROTOCOL_VERSION, installed.source_hash
        )
        .as_bytes(),
    );
    Ok(TrustedSession {
        owner: owner_name,
        pid,
        instance,
        source: BUNDLED_SOURCE,
        _process_fd: process_fd,
    })
}

fn revalidate_owner(connection: &Connection, session: &TrustedSession) -> anyhow::Result<()> {
    let dbus = DBusProxy::new(connection)?;
    let current = dbus.get_name_owner(BusName::try_from(KWIN_DEST)?)?;
    if current.as_str() != session.owner {
        anyhow::bail!("kwin_adapter_restarted: KWin owner changed during the transaction");
    }
    let pid = dbus.get_connection_unix_process_id(BusName::try_from(session.owner.as_str())?)?;
    if pid != session.pid {
        anyhow::bail!("kwin_adapter_restarted: KWin process changed during the transaction");
    }
    Ok(())
}

fn hex_digest(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn request_token() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    hex_digest(
        format!(
            "{}:{now}:{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )
        .as_bytes(),
    )
}

struct Callback {
    expected_sender: String,
    expected_token: String,
    sender: mpsc::SyncSender<anyhow::Result<String>>,
}

#[zbus::interface(name = "org.cua.KWinAdapter.Callback")]
impl Callback {
    fn reply(
        &self,
        token: &str,
        payload: &str,
        #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<()> {
        let sender = header.sender().map(ToString::to_string);
        if sender.as_deref() != Some(self.expected_sender.as_str()) {
            return Err(zbus::fdo::Error::AccessDenied(
                "callback sender is not the pinned KWin owner".to_owned(),
            ));
        }
        if token != self.expected_token {
            return Err(zbus::fdo::Error::AccessDenied(
                "callback nonce does not match the request".to_owned(),
            ));
        }
        if payload.len() > RESPONSE_LIMIT {
            let _ = self.sender.try_send(Err(anyhow!(
                "kwin_adapter_malformed: response exceeded {RESPONSE_LIMIT} bytes"
            )));
            return Ok(());
        }
        let _ = self.sender.try_send(Ok(payload.to_owned()));
        Ok(())
    }
}

struct TempScript {
    path: PathBuf,
    _file: File,
}

impl TempScript {
    fn create(contents: &str, token: &str) -> anyhow::Result<Self> {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let path = runtime.join(format!("cua-kwin-{token}.js"));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .with_context(|| format!("could not create one-shot KWin script {}", path.display()))?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        Ok(Self { path, _file: file })
    }
}

impl Drop for TempScript {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn js_string(value: &str) -> anyhow::Result<String> {
    serde_json::to_string(value).context("could not quote KWin script value")
}

enum ScriptRequest<'a> {
    Snapshot,
    Activate(&'a str),
}

fn generated_script(
    session: &TrustedSession,
    destination: &str,
    token: &str,
    request: ScriptRequest<'_>,
) -> anyhow::Result<String> {
    let expression = match request {
        ScriptRequest::Snapshot => "cuaKWinSnapshot()".to_owned(),
        ScriptRequest::Activate(uuid) => format!("cuaKWinActivate({})", js_string(uuid)?),
    };
    Ok(format!(
        "{}\ntry {{\n  const __cuaResult = {};\n  callDBus({}, {}, {}, \"Reply\", {}, JSON.stringify(__cuaResult));\n}} catch (__cuaError) {{\n  callDBus({}, {}, {}, \"Reply\", {}, JSON.stringify({{protocol_version:{}, error:String(__cuaError)}}));\n}}\n",
        session.source,
        expression,
        js_string(destination)?,
        js_string(CALLBACK_PATH)?,
        js_string(CALLBACK_IFACE)?,
        js_string(token)?,
        js_string(destination)?,
        js_string(CALLBACK_PATH)?,
        js_string(CALLBACK_IFACE)?,
        js_string(token)?,
        PROTOCOL_VERSION,
    ))
}

struct LoadedScript<'a> {
    proxy: Proxy<'a>,
    script_proxy: Proxy<'a>,
    name: String,
}

impl Drop for LoadedScript<'_> {
    fn drop(&mut self) {
        let _: Result<(), _> = self.script_proxy.call("stop", &());
        let _: Result<bool, _> = self.proxy.call("unloadScript", &(self.name.as_str(),));
    }
}

fn run_script(session: &TrustedSession, request: ScriptRequest<'_>) -> anyhow::Result<String> {
    let connection = Connection::session().context("kwin_adapter_unavailable: session bus")?;
    revalidate_owner(&connection, session)?;
    let token = request_token();
    let destination = connection
        .unique_name()
        .ok_or_else(|| anyhow!("kwin_adapter_untrusted_owner: callback has no unique bus name"))?
        .to_string();
    let (sender, receiver) = mpsc::sync_channel(1);
    connection.object_server().at(
        CALLBACK_PATH,
        Callback {
            expected_sender: session.owner.clone(),
            expected_token: token.clone(),
            sender,
        },
    )?;
    let contents = generated_script(session, &destination, &token, request)?;
    let temp = TempScript::create(&contents, &token)?;
    let script_name = format!("cua-kwin-request-{}", &token[..16]);
    let scripting = Proxy::new(
        &connection,
        session.owner.as_str(),
        SCRIPTING_PATH,
        SCRIPTING_IFACE,
    )?;
    let path = temp
        .path
        .to_str()
        .ok_or_else(|| anyhow!("KWin adapter path is not UTF-8"))?;
    let script_id: i32 = scripting.call("loadScript", &(path, script_name.as_str()))?;
    if script_id < 0 {
        anyhow::bail!("kwin_adapter_unavailable: KWin refused to load the one-shot adapter");
    }
    let object_path = format!("/Scripting/Script{script_id}");
    let script_proxy = Proxy::new(
        &connection,
        session.owner.as_str(),
        object_path.as_str(),
        SCRIPT_IFACE,
    )?;
    let loaded = LoadedScript {
        proxy: scripting,
        script_proxy,
        name: script_name,
    };
    let _: () = loaded.script_proxy.call("run", &())?;
    let result = receiver
        .recv_timeout(REQUEST_TIMEOUT)
        .map_err(|_| anyhow!("kwin_adapter_timeout: KWin did not return a response"))??;
    revalidate_owner(&connection, session)?;
    drop(loaded);
    drop(temp);
    Ok(result)
}

fn parse_snapshot(raw: &str) -> anyhow::Result<Snapshot> {
    let snapshot: Snapshot =
        serde_json::from_str(raw).context("kwin_adapter_malformed: invalid snapshot JSON")?;
    validate_snapshot(snapshot)
}

fn validate_snapshot(snapshot: Snapshot) -> anyhow::Result<Snapshot> {
    if snapshot.protocol_version != PROTOCOL_VERSION {
        anyhow::bail!(
            "kwin_adapter_protocol_unsupported: expected {}, received {}",
            PROTOCOL_VERSION,
            snapshot.protocol_version
        );
    }
    let mut uuids = HashSet::with_capacity(snapshot.windows.len());
    let mut active = None;
    for (index, window) in snapshot.windows.iter().enumerate() {
        if !valid_uuid(&window.uuid) {
            anyhow::bail!(
                "kwin_adapter_malformed: invalid KWin UUID {:?}",
                window.uuid
            );
        }
        if !uuids.insert(window.uuid.as_str()) {
            anyhow::bail!(
                "kwin_adapter_malformed: duplicate KWin UUID {}",
                window.uuid
            );
        }
        if window.stacking != index {
            anyhow::bail!("kwin_adapter_malformed: non-contiguous stacking positions");
        }
        if window.visible
            && (window.minimized || window.hidden || window.width == 0 || window.height == 0)
        {
            anyhow::bail!(
                "kwin_adapter_malformed: inconsistent visibility for {}",
                window.uuid
            );
        }
        if window.active && active.replace(window.uuid.as_str()).is_some() {
            anyhow::bail!("kwin_adapter_malformed: multiple active KWin windows");
        }
    }
    if active != snapshot.active_window_uuid.as_deref() {
        anyhow::bail!("kwin_adapter_malformed: active UUID disagrees with window state");
    }
    if snapshot
        .active_window_uuid
        .as_deref()
        .is_some_and(|uuid| !valid_uuid(uuid) || !uuids.contains(uuid))
    {
        anyhow::bail!("kwin_adapter_malformed: active UUID is absent from the snapshot");
    }
    Ok(snapshot)
}

fn valid_uuid(value: &str) -> bool {
    let inner = value
        .strip_prefix('{')
        .and_then(|value| value.strip_suffix('}'))
        .unwrap_or(value);
    if inner.len() != 36 {
        return false;
    }
    inner.char_indices().all(|(index, character)| match index {
        8 | 13 | 18 | 23 => character == '-',
        _ => character.is_ascii_hexdigit(),
    }) && !inner
        .chars()
        .all(|character| character == '0' || character == '-')
}

fn snapshot_for(session: &TrustedSession) -> anyhow::Result<Snapshot> {
    parse_snapshot(&run_script(session, ScriptRequest::Snapshot)?)
}

fn activate_uuid(session: &TrustedSession, uuid: &str) -> anyhow::Result<()> {
    let raw = run_script(session, ScriptRequest::Activate(uuid))?;
    let reply: ActivationReply =
        serde_json::from_str(&raw).context("kwin_adapter_malformed: invalid activation reply")?;
    if reply.protocol_version != PROTOCOL_VERSION {
        anyhow::bail!("kwin_adapter_protocol_unsupported: activation reply version");
    }
    if reply.target_uuid != uuid {
        anyhow::bail!("kwin_adapter_malformed: activation reply changed target UUID");
    }
    if !reply.accepted {
        anyhow::bail!("kwin_adapter_target_disappeared: KWin did not find the exact target UUID");
    }
    Ok(())
}

#[derive(Default)]
struct IdRegistry {
    instance: Option<String>,
    by_uuid: HashMap<String, u32>,
    by_id: HashMap<u32, String>,
    stale: HashSet<u32>,
}

impl IdRegistry {
    fn begin_instance(&mut self, instance: &str) {
        if self
            .instance
            .as_deref()
            .is_some_and(|known| known != instance)
        {
            self.stale.extend(self.by_id.keys().copied());
            self.by_uuid.clear();
            self.by_id.clear();
        }
        self.instance = Some(instance.to_owned());
    }

    fn candidate(instance: &str, uuid: &str) -> u32 {
        let digest = Sha256::digest(format!("{instance}\0{uuid}").as_bytes());
        let raw = u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]);
        KWIN_ID_NAMESPACE | (raw & KWIN_ID_MASK)
    }

    fn id_for(&mut self, instance: &str, uuid: &str) -> anyhow::Result<u32> {
        let candidate = Self::candidate(instance, uuid);
        self.id_for_candidate(instance, uuid, candidate)
    }

    fn id_for_candidate(
        &mut self,
        instance: &str,
        uuid: &str,
        candidate: u32,
    ) -> anyhow::Result<u32> {
        self.begin_instance(instance);
        if let Some(id) = self.by_uuid.get(uuid) {
            return Ok(*id);
        }
        if self.stale.contains(&candidate) {
            anyhow::bail!(
                "kwin_adapter_id_collision: new adapter incarnation would reuse stale id {candidate}"
            );
        }
        if let Some(other) = self.by_id.get(&candidate) {
            anyhow::bail!(
                "kwin_adapter_id_collision: UUIDs {other} and {uuid} map to numeric id {candidate}"
            );
        }
        self.by_uuid.insert(uuid.to_owned(), candidate);
        self.by_id.insert(candidate, uuid.to_owned());
        Ok(candidate)
    }

    fn uuid_for(&mut self, instance: &str, id: u32) -> anyhow::Result<String> {
        self.begin_instance(instance);
        if let Some(uuid) = self.by_id.get(&id) {
            return Ok(uuid.clone());
        }
        if self.stale.contains(&id) {
            anyhow::bail!("kwin_adapter_stale_id: numeric window id {id} belongs to an older adapter incarnation");
        }
        anyhow::bail!("kwin_adapter_unknown_id: numeric window id {id} is not known to KWin")
    }
}

fn registry() -> &'static Mutex<IdRegistry> {
    static REGISTRY: OnceLock<Mutex<IdRegistry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(IdRegistry::default()))
}

fn register_snapshot(session: &TrustedSession, snapshot: &Snapshot) -> anyhow::Result<()> {
    let mut registry = registry()
        .lock()
        .map_err(|_| anyhow!("kwin_adapter_unavailable: id registry lock poisoned"))?;
    registry.begin_instance(&session.instance);
    for window in &snapshot.windows {
        registry.id_for(&session.instance, &window.uuid)?;
    }
    Ok(())
}

fn numeric_id(session: &TrustedSession, uuid: &str) -> anyhow::Result<u32> {
    registry()
        .lock()
        .map_err(|_| anyhow!("kwin_adapter_unavailable: id registry lock poisoned"))?
        .id_for(&session.instance, uuid)
}

fn resolve_uuid(
    session: &TrustedSession,
    snapshot: &Snapshot,
    pid: u32,
    window_id: u64,
) -> anyhow::Result<String> {
    let id = u32::try_from(window_id)
        .map_err(|_| anyhow!("kwin_adapter_unknown_id: window id is outside the u32 contract"))?;
    register_snapshot(session, snapshot)?;
    let uuid = registry()
        .lock()
        .map_err(|_| anyhow!("kwin_adapter_unavailable: id registry lock poisoned"))?
        .uuid_for(&session.instance, id)?;
    let window = snapshot
        .windows
        .iter()
        .find(|window| window.uuid == uuid)
        .ok_or_else(|| anyhow!("kwin_adapter_stale_id: target window was destroyed"))?;
    if window.pid != pid {
        anyhow::bail!(
            "kwin_adapter_wrong_pid: exact KWin window {} belongs to pid {}, not {pid}",
            uuid,
            window.pid
        );
    }
    Ok(uuid)
}

fn window_info(session: &TrustedSession, window: &KWinWindow) -> anyhow::Result<WindowInfo> {
    let id = numeric_id(session, &window.uuid)?;
    Ok(WindowInfo {
        xid: u64::from(id),
        pid: (window.pid != 0).then_some(window.pid),
        app_name: window.title.clone(),
        title: window.title.clone(),
        is_on_screen: window.visible && !window.minimized && !window.hidden,
        z_index: Some(window.stacking),
        x: window.x,
        y: window.y,
        width: window.width,
        height: window.height,
    })
}

pub fn list_windows(filter_pid: Option<u32>) -> anyhow::Result<Vec<WindowInfo>> {
    let connection = Connection::session()?;
    let session = trusted_session_on(&connection)?;
    let snapshot = snapshot_for(&session)?;
    register_snapshot(&session, &snapshot)?;
    snapshot
        .windows
        .iter()
        .filter(|window| filter_pid.is_none_or(|pid| window.pid == pid))
        .map(|window| window_info(&session, window))
        .collect()
}

pub fn trusted_window_for_id(pid: u32, window_id: u64) -> anyhow::Result<Option<WindowInfo>> {
    let connection = Connection::session()?;
    let session = trusted_session_on(&connection)?;
    let snapshot = snapshot_for(&session)?;
    let uuid = resolve_uuid(&session, &snapshot, pid, window_id)?;
    snapshot
        .windows
        .iter()
        .find(|window| window.uuid == uuid)
        .map(|window| window_info(&session, window))
        .transpose()
}

pub fn trusted_window_ids_for_pid(pid: u32) -> anyhow::Result<Vec<u64>> {
    let connection = Connection::session()?;
    let session = trusted_session_on(&connection)?;
    let snapshot = snapshot_for(&session)?;
    register_snapshot(&session, &snapshot)?;
    snapshot
        .windows
        .iter()
        .filter(|window| window.pid == pid)
        .map(|window| numeric_id(&session, &window.uuid).map(u64::from))
        .collect()
}

/// Resolve a numeric KWin handle back to its compositor-attested PID.  This is
/// an exact UUID lookup; it never guesses from a title, geometry, or PID-only
/// match.
pub fn pid_for_window_id(window_id: u64) -> anyhow::Result<u32> {
    let id = u32::try_from(window_id)
        .map_err(|_| anyhow!("kwin_adapter_unknown_id: window id is outside the u32 contract"))?;
    let connection = Connection::session()?;
    let session = trusted_session_on(&connection)?;
    let snapshot = snapshot_for(&session)?;
    register_snapshot(&session, &snapshot)?;
    let uuid = registry()
        .lock()
        .map_err(|_| anyhow!("kwin_adapter_unavailable: id registry lock poisoned"))?
        .uuid_for(&session.instance, id)?;
    snapshot
        .windows
        .iter()
        .find(|window| window.uuid == uuid)
        .map(|window| window.pid)
        .ok_or_else(|| anyhow!("kwin_adapter_stale_id: target window was destroyed"))
}

pub fn unique_window_for_pid(pid: u32) -> anyhow::Result<WindowInfo> {
    let windows = list_windows(Some(pid))?;
    match windows.as_slice() {
        [window] => Ok(window.clone()),
        [] => anyhow::bail!("kwin_adapter_unknown_pid: KWin exposes no window for pid {pid}"),
        _ => anyhow::bail!(
            "kwin_adapter_ambiguous_pid: KWin exposes {} windows for pid {pid}; an exact compositor id is required",
            windows.len()
        ),
    }
}

pub fn window_origin_for_pid(pid: u32) -> Option<(i32, i32)> {
    unique_window_for_pid(pid)
        .ok()
        .map(|window| (window.x, window.y))
}

trait FocusTransport {
    fn snapshot(&mut self) -> anyhow::Result<Snapshot>;
    fn activate(&mut self, uuid: &str) -> anyhow::Result<()>;
}

struct LiveTransport<'a> {
    session: &'a TrustedSession,
}

impl FocusTransport for LiveTransport<'_> {
    fn snapshot(&mut self) -> anyhow::Result<Snapshot> {
        snapshot_for(self.session)
    }

    fn activate(&mut self, uuid: &str) -> anyhow::Result<()> {
        activate_uuid(self.session, uuid)
    }
}

fn exact_window<'a>(
    snapshot: &'a Snapshot,
    uuid: &str,
    pid: u32,
) -> anyhow::Result<&'a KWinWindow> {
    let window = snapshot
        .windows
        .iter()
        .find(|window| window.uuid == uuid)
        .ok_or_else(|| anyhow!("kwin_adapter_target_disappeared: exact target was destroyed"))?;
    if window.pid != pid {
        anyhow::bail!("kwin_adapter_wrong_pid: exact target changed process ownership");
    }
    Ok(window)
}

fn verify_active(snapshot: &Snapshot, uuid: &str, pid: u32) -> anyhow::Result<()> {
    let window = exact_window(snapshot, uuid, pid)?;
    if snapshot.active_window_uuid.as_deref() != Some(uuid) || !window.active {
        anyhow::bail!(
            "kwin_adapter_activation_not_verified: KWin accepted activation but exact target is not active"
        );
    }
    if window.minimized || window.hidden || !window.visible {
        anyhow::bail!(
            "kwin_adapter_activation_not_verified: exact target is active but not input-visible"
        );
    }
    Ok(())
}

fn combine_operation_and_restore<T>(
    operation: anyhow::Result<T>,
    restoration: anyhow::Result<()>,
) -> anyhow::Result<T> {
    match (operation, restoration) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(restore)) => Err(anyhow!(
            "kwin_adapter_restoration_failed: operation succeeded but prior focus was not restored: {restore}"
        )),
        (Err(operation), Err(restore)) => Err(anyhow!(
            "{operation}; kwin_adapter_restoration_failed: prior focus was not restored: {restore}"
        )),
    }
}

fn focus_transaction<T>(
    transport: &mut impl FocusTransport,
    pid: u32,
    numeric_id: u32,
    uuid: &str,
    body: impl FnOnce() -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let before = transport.snapshot()?;
    let target = exact_window(&before, uuid, pid)?;
    let previous = before.active_window_uuid.clone().ok_or_else(|| {
        anyhow!("kwin_adapter_no_focused_window: KWin exposes no restorable active window")
    })?;
    let guard = VerifiedGuard { pid, numeric_id };

    if target.active && previous == uuid {
        verify_active(&before, uuid, pid)?;
        return with_verified_guard(guard, body);
    }

    transport.activate(uuid)?;
    let activated = transport.snapshot()?;
    verify_active(&activated, uuid, pid)?;
    let operation = with_verified_guard(guard, body);

    let restoration = (|| {
        transport.activate(&previous)?;
        let restored = transport.snapshot()?;
        if restored.active_window_uuid.as_deref() != Some(previous.as_str())
            || !restored
                .windows
                .iter()
                .any(|window| window.uuid == previous && window.active)
        {
            anyhow::bail!("fresh KWin snapshot did not confirm the previous UUID active");
        }
        Ok(())
    })();
    combine_operation_and_restore(operation, restoration)
}

pub fn with_focused_window<T>(
    pid: u32,
    window_id: u64,
    body: impl FnOnce() -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let numeric_id = u32::try_from(window_id)
        .map_err(|_| anyhow!("kwin_adapter_unknown_id: window id is outside the u32 contract"))?;
    let connection = Connection::session()?;
    let session = trusted_session_on(&connection)?;
    let before = snapshot_for(&session)?;
    let uuid = resolve_uuid(&session, &before, pid, window_id)?;
    // The lifecycle obtains its own fresh pre-operation snapshot.  This is
    // intentional: numeric resolution cannot double as the focus proof.
    let mut transport = LiveTransport { session: &session };
    focus_transaction(&mut transport, pid, numeric_id, &uuid, body)
}

/// Persistently activate an exact KWin window (used only by explicit
/// bring-to-front style operations).  Unlike the bounded wrapper, this does not
/// restore focus, but it still requires a fresh exact-UUID verification.
pub fn activate_window(pid: u32, window_id: u64) -> anyhow::Result<()> {
    let connection = Connection::session()?;
    let session = trusted_session_on(&connection)?;
    let before = snapshot_for(&session)?;
    let uuid = resolve_uuid(&session, &before, pid, window_id)?;
    activate_uuid(&session, &uuid)?;
    let after = snapshot_for(&session)?;
    verify_active(&after, &uuid, pid)
}

pub fn diagnose() -> AdapterDiagnostic {
    let mut diagnostic = AdapterDiagnostic::default();
    let installed = match installed_adapter() {
        Ok(_) => {
            diagnostic.installed = true;
            diagnostic.protocol_supported = true;
            true
        }
        Err(error) => {
            let text = error.to_string();
            diagnostic.installed = !text.contains("kwin_adapter_missing");
            diagnostic.protocol_supported = false;
            diagnostic.message = format!("{text}. Run {INSTALL_COMMAND}");
            false
        }
    };
    if !installed {
        return diagnostic;
    }

    let connection = match Connection::session() {
        Ok(connection) => connection,
        Err(error) => {
            diagnostic.message = format!("KWin session bus unavailable: {error}");
            return diagnostic;
        }
    };
    let session = match trusted_session_on(&connection) {
        Ok(session) => {
            diagnostic.owner_trusted = true;
            session
        }
        Err(error) => {
            diagnostic.message = error.to_string();
            return diagnostic;
        }
    };
    let snapshot = match snapshot_for(&session) {
        Ok(snapshot) => {
            diagnostic.enumeration_available = true;
            snapshot
        }
        Err(error) => {
            diagnostic.message = error.to_string();
            return diagnostic;
        }
    };
    let Some(active) = snapshot.active_window_uuid.as_deref() else {
        diagnostic.message =
            "kwin_adapter_no_focused_window: enumeration works, but activation cannot be verified without an active window".to_owned();
        return diagnostic;
    };
    let Some(window) = snapshot.windows.iter().find(|window| window.uuid == active) else {
        diagnostic.message = "kwin_adapter_malformed: active UUID is absent".to_owned();
        return diagnostic;
    };
    match activate_uuid(&session, active)
        .and_then(|()| snapshot_for(&session))
        .and_then(|fresh| verify_active(&fresh, active, window.pid))
    {
        Ok(()) => {
            diagnostic.activation_verified = true;
            diagnostic.message = format!(
                "trusted KWin adapter protocol v{PROTOCOL_VERSION}; exact enumeration and activation verification succeeded"
            );
        }
        Err(error) => diagnostic.message = error.to_string(),
    }
    diagnostic
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    const A: &str = "{11111111-1111-4111-8111-111111111111}";
    const B: &str = "{22222222-2222-4222-8222-222222222222}";

    fn window(uuid: &str, pid: u32, active: bool) -> KWinWindow {
        KWinWindow {
            uuid: uuid.to_owned(),
            pid,
            title: format!("window-{pid}"),
            x: 10,
            y: 20,
            width: 800,
            height: 600,
            active,
            minimized: false,
            hidden: false,
            visible: true,
            stacking: 0,
        }
    }

    fn snapshot(active: Option<&str>, mut windows: Vec<KWinWindow>) -> Snapshot {
        for (index, window) in windows.iter_mut().enumerate() {
            window.stacking = index;
            window.active = active == Some(window.uuid.as_str());
        }
        Snapshot {
            protocol_version: PROTOCOL_VERSION,
            active_window_uuid: active.map(str::to_owned),
            windows,
        }
    }

    fn raw_snapshot() -> String {
        serde_json::json!({
            "protocol_version": 1,
            "active_window_uuid": A,
            "windows": [{
                "uuid": A,
                "pid": 42,
                "title": "Konsole",
                "x": 10,
                "y": 20,
                "width": 800,
                "height": 600,
                "active": true,
                "minimized": false,
                "hidden": false,
                "visible": true,
                "stacking": 0
            }]
        })
        .to_string()
    }

    #[test]
    fn detects_only_kde_wayland() {
        assert!(detect_kde_wayland(Some("wayland"), Some("KDE"), true));
        assert!(detect_kde_wayland(
            Some("WAYLAND"),
            Some("KDE:PLASMA"),
            true
        ));
        assert!(!detect_kde_wayland(Some("x11"), Some("KDE"), true));
        assert!(!detect_kde_wayland(Some("wayland"), Some("GNOME"), true));
        assert!(!detect_kde_wayland(Some("wayland"), Some("KDE"), false));
    }

    #[test]
    fn parses_valid_snapshot_and_rejects_malformed_response() {
        let parsed = parse_snapshot(&raw_snapshot()).expect("valid snapshot");
        assert_eq!(parsed.windows[0].uuid, A);
        assert!(parse_snapshot("not-json").is_err());

        let mut unknown = serde_json::from_str::<serde_json::Value>(&raw_snapshot()).unwrap();
        unknown["unexpected"] = serde_json::json!(true);
        assert!(parse_snapshot(&unknown.to_string()).is_err());

        let mut duplicate = serde_json::from_str::<serde_json::Value>(&raw_snapshot()).unwrap();
        let first = duplicate["windows"][0].clone();
        duplicate["windows"].as_array_mut().unwrap().push(first);
        assert!(parse_snapshot(&duplicate.to_string()).is_err());
    }

    #[test]
    fn rejects_protocol_version_and_inconsistent_active_identity() {
        let mut wrong = serde_json::from_str::<serde_json::Value>(&raw_snapshot()).unwrap();
        wrong["protocol_version"] = serde_json::json!(2);
        assert!(parse_snapshot(&wrong.to_string())
            .unwrap_err()
            .to_string()
            .contains("protocol_unsupported"));

        let mut active = serde_json::from_str::<serde_json::Value>(&raw_snapshot()).unwrap();
        active["windows"][0]["active"] = serde_json::json!(false);
        assert!(parse_snapshot(&active.to_string()).is_err());
    }

    fn secure_facts() -> OwnerFacts {
        OwnerFacts {
            unique_name: ":1.15".to_owned(),
            pid: 2560,
            uid: 1000,
            comm: "kwin_wayland\n".to_owned(),
            argv0: Some(PathBuf::from("/usr/bin/kwin_wayland")),
            argv0_uid: Some(0),
            argv0_mode: Some(0o100755),
            executable: Some(PathBuf::from("/usr/bin/kwin_wayland")),
            executable_uid: Some(0),
            executable_mode: Some(0o100755),
            cgroup: "0::/session.slice/plasma-kwin_wayland.service\n".to_owned(),
            parent_argv0: Some(PathBuf::from("/usr/bin/kwin_wayland_wrapper")),
            parent_executable_uid: Some(0),
            parent_executable_mode: Some(0o100755),
            start_time: 123,
        }
    }

    #[test]
    fn rejects_untrusted_owner_wrong_uid_process_and_executable() {
        let facts = secure_facts();
        validate_owner_facts(&facts, 1000).unwrap();

        let mut owner = facts.clone();
        owner.unique_name = "org.kde.KWin".to_owned();
        assert!(validate_owner_facts(&owner, 1000).is_err());

        let mut uid = facts.clone();
        uid.uid = 1001;
        assert!(validate_owner_facts(&uid, 1000)
            .unwrap_err()
            .to_string()
            .contains("wrong_uid"));

        let mut process = facts.clone();
        process.comm = "fake-kwin".to_owned();
        assert!(validate_owner_facts(&process, 1000)
            .unwrap_err()
            .to_string()
            .contains("wrong_process"));

        let mut executable = facts;
        executable.executable_uid = Some(1000);
        executable.cgroup.clear();
        assert!(validate_owner_facts(&executable, 1000)
            .unwrap_err()
            .to_string()
            .contains("wrong_executable"));
    }

    #[test]
    fn validates_fedora_systemd_chain_when_proc_exe_is_unreadable() {
        let mut facts = secure_facts();
        facts.executable = None;
        facts.executable_uid = None;
        facts.executable_mode = None;
        validate_owner_facts(&facts, 1000).unwrap();
    }

    #[test]
    fn uuid_mapping_retains_identity_and_rejects_collision() {
        let mut registry = IdRegistry::default();
        assert_eq!(
            registry.id_for_candidate("one", A, 0xE000_0001).unwrap(),
            0xE000_0001
        );
        assert_eq!(registry.uuid_for("one", 0xE000_0001).unwrap(), A);
        let collision = registry
            .id_for_candidate("one", B, 0xE000_0001)
            .unwrap_err()
            .to_string();
        assert!(collision.contains("id_collision"));
    }

    #[test]
    fn helper_restart_tombstones_old_ids() {
        let mut registry = IdRegistry::default();
        registry.id_for_candidate("one", A, 0xE000_0001).unwrap();
        registry.begin_instance("two");
        let stale = registry
            .uuid_for("two", 0xE000_0001)
            .unwrap_err()
            .to_string();
        assert!(stale.contains("stale_id"));
        assert!(registry
            .id_for_candidate("two", A, 0xE000_0001)
            .unwrap_err()
            .to_string()
            .contains("id_collision"));
    }

    #[test]
    fn destroyed_window_is_stale_and_pid_only_is_ambiguous() {
        let before = snapshot(Some(A), vec![window(A, 42, true), window(B, 42, false)]);
        assert_eq!(
            before
                .windows
                .iter()
                .filter(|window| window.pid == 42)
                .count(),
            2
        );
        let mut registry = IdRegistry::default();
        let id = registry.id_for_candidate("one", B, 0xE000_0002).unwrap();
        assert_eq!(registry.uuid_for("one", id).unwrap(), B);
        let after = snapshot(Some(A), vec![window(A, 42, true)]);
        assert!(!after.windows.iter().any(|window| window.uuid == B));
    }

    struct MockTransport {
        snapshots: VecDeque<anyhow::Result<Snapshot>>,
        activations: VecDeque<anyhow::Result<()>>,
        activated: Vec<String>,
    }

    impl MockTransport {
        fn new(snapshots: Vec<Snapshot>) -> Self {
            Self {
                snapshots: snapshots.into_iter().map(Ok).collect(),
                activations: VecDeque::new(),
                activated: Vec::new(),
            }
        }
    }

    impl FocusTransport for MockTransport {
        fn snapshot(&mut self) -> anyhow::Result<Snapshot> {
            self.snapshots
                .pop_front()
                .unwrap_or_else(|| Err(anyhow!("unexpected snapshot")))
        }

        fn activate(&mut self, uuid: &str) -> anyhow::Result<()> {
            self.activated.push(uuid.to_owned());
            self.activations.pop_front().unwrap_or(Ok(()))
        }
    }

    fn before() -> Snapshot {
        snapshot(Some(A), vec![window(A, 1, true), window(B, 2, false)])
    }

    fn target_active() -> Snapshot {
        snapshot(Some(B), vec![window(A, 1, false), window(B, 2, true)])
    }

    fn restored() -> Snapshot {
        before()
    }

    #[test]
    fn successful_activation_operation_and_restoration() {
        let mut transport = MockTransport::new(vec![before(), target_active(), restored()]);
        let value = focus_transaction(&mut transport, 2, 9, B, || Ok(7)).unwrap();
        assert_eq!(value, 7);
        assert_eq!(transport.activated, vec![B, A]);
    }

    #[test]
    fn activation_accepted_without_focus_never_runs_operation() {
        let mut transport = MockTransport::new(vec![before(), before()]);
        let called = std::cell::Cell::new(false);
        let error = focus_transaction(&mut transport, 2, 9, B, || {
            called.set(true);
            Ok(())
        })
        .unwrap_err();
        assert!(!called.get());
        assert!(error.to_string().contains("activation_not_verified"));
    }

    #[test]
    fn target_already_focused_runs_without_restore() {
        let already = target_active();
        let mut transport = MockTransport::new(vec![already]);
        focus_transaction(&mut transport, 2, 9, B, || Ok(())).unwrap();
        assert!(transport.activated.is_empty());
    }

    #[test]
    fn target_disappearing_during_activation_is_refused() {
        let disappeared = snapshot(Some(A), vec![window(A, 1, true)]);
        let mut transport = MockTransport::new(vec![before(), disappeared]);
        let called = std::cell::Cell::new(false);
        assert!(focus_transaction(&mut transport, 2, 9, B, || {
            called.set(true);
            Ok(())
        })
        .unwrap_err()
        .to_string()
        .contains("target_disappeared"));
        assert!(!called.get());
    }

    #[test]
    fn minimized_target_must_become_visible_before_operation() {
        let mut minimized = window(B, 2, false);
        minimized.minimized = true;
        minimized.visible = false;
        let before = snapshot(Some(A), vec![window(A, 1, true), minimized.clone()]);
        let still_minimized = snapshot(
            Some(B),
            vec![window(A, 1, false), {
                minimized.active = true;
                minimized
            }],
        );
        let mut transport = MockTransport::new(vec![before, still_minimized]);
        assert!(focus_transaction(&mut transport, 2, 9, B, || Ok(()))
            .unwrap_err()
            .to_string()
            .contains("not input-visible"));
    }

    #[test]
    fn no_focused_window_refuses_before_activation() {
        let none = snapshot(None, vec![window(B, 2, false)]);
        let mut transport = MockTransport::new(vec![none]);
        assert!(focus_transaction(&mut transport, 2, 9, B, || Ok(()))
            .unwrap_err()
            .to_string()
            .contains("no_focused_window"));
        assert!(transport.activated.is_empty());
    }

    #[test]
    fn operation_failure_with_successful_restore_preserves_operation_error() {
        let mut transport = MockTransport::new(vec![before(), target_active(), restored()]);
        let error = focus_transaction::<() /* result */>(&mut transport, 2, 9, B, || {
            anyhow::bail!("operation failed")
        })
        .unwrap_err();
        assert_eq!(error.to_string(), "operation failed");
    }

    #[test]
    fn operation_success_with_failed_restore_reports_restoration() {
        let mut transport = MockTransport::new(vec![before(), target_active(), target_active()]);
        let error = focus_transaction(&mut transport, 2, 9, B, || Ok(())).unwrap_err();
        assert!(error.to_string().contains("restoration_failed"));
        assert!(error.to_string().contains("operation succeeded"));
    }

    #[test]
    fn operation_and_restoration_failures_are_combined() {
        let mut transport = MockTransport::new(vec![before(), target_active(), target_active()]);
        let error = focus_transaction::<()>(&mut transport, 2, 9, B, || {
            anyhow::bail!("operation failed")
        })
        .unwrap_err()
        .to_string();
        assert!(error.contains("operation failed"));
        assert!(error.contains("restoration_failed"));
    }

    #[test]
    fn valid_uuid_accepts_braced_and_bare_and_rejects_degenerate_forms() {
        assert!(valid_uuid(A));
        assert!(valid_uuid("11111111-1111-4111-8111-111111111111"));
        // The compositor UUID must be a real identity: all-zero, wrong length,
        // and misplaced separators are rejected so a spoofed handle cannot pass.
        assert!(!valid_uuid("{00000000-0000-0000-0000-000000000000}"));
        assert!(!valid_uuid("00000000-0000-0000-0000-000000000000"));
        assert!(!valid_uuid("{1111-1111-4111-8111-111111111111}"));
        assert!(!valid_uuid("11111111x1111-4111-8111-111111111111"));
        assert!(!valid_uuid("{11111111-1111-4111-8111-11111111111g}"));
        assert!(!valid_uuid(""));
    }

    #[test]
    fn validate_snapshot_rejects_noncontiguous_stacking() {
        let mut window = window(A, 42, true);
        window.stacking = 3;
        let snapshot = Snapshot {
            protocol_version: PROTOCOL_VERSION,
            active_window_uuid: Some(A.to_owned()),
            windows: vec![window],
        };
        assert!(validate_snapshot(snapshot)
            .unwrap_err()
            .to_string()
            .contains("non-contiguous stacking"));
    }

    #[test]
    fn validate_snapshot_rejects_multiple_active_windows() {
        let snapshot = Snapshot {
            protocol_version: PROTOCOL_VERSION,
            active_window_uuid: Some(A.to_owned()),
            windows: vec![window(A, 1, true), {
                let mut second = window(B, 2, true);
                second.stacking = 1;
                second
            }],
        };
        assert!(validate_snapshot(snapshot)
            .unwrap_err()
            .to_string()
            .contains("multiple active"));
    }

    #[test]
    fn validate_snapshot_rejects_inconsistent_visibility() {
        let mut window = window(A, 42, true);
        window.minimized = true;
        window.visible = true;
        let snapshot = Snapshot {
            protocol_version: PROTOCOL_VERSION,
            active_window_uuid: Some(A.to_owned()),
            windows: vec![window],
        };
        assert!(validate_snapshot(snapshot)
            .unwrap_err()
            .to_string()
            .contains("inconsistent visibility"));
    }

    #[test]
    fn validate_snapshot_rejects_active_uuid_absent_from_windows() {
        let snapshot = Snapshot {
            protocol_version: PROTOCOL_VERSION,
            active_window_uuid: Some(B.to_owned()),
            windows: vec![window(A, 1, false)],
        };
        assert!(validate_snapshot(snapshot).is_err());
    }

    #[test]
    fn source_contains_exact_current_kwin_api_and_no_top_level_request() {
        assert!(BUNDLED_SOURCE.contains("workspace.stackingOrder"));
        assert!(BUNDLED_SOURCE.contains("workspace.activeWindow"));
        assert!(BUNDLED_SOURCE.contains("internalId.toString()"));
        assert!(BUNDLED_SOURCE.contains("function cuaKWinSnapshot"));
        assert!(BUNDLED_SOURCE.contains("function cuaKWinActivate"));
        assert!(!BUNDLED_SOURCE.contains("org.cua.KWinAdapter.Callback"));
        // KWin frame geometry is fractional under fractional scaling; the
        // snapshot contract is integer pixels, so the script must round every
        // geometry field or the driver rejects the snapshot as malformed.
        assert!(BUNDLED_SOURCE.contains("Math.round(Number(geometry.x))"));
        assert!(BUNDLED_SOURCE.contains("Math.round(Number(geometry.y))"));
        assert!(BUNDLED_SOURCE.contains("Math.round(Number(geometry.width))"));
        assert!(BUNDLED_SOURCE.contains("Math.round(Number(geometry.height))"));
    }
}
