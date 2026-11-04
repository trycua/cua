//! Tree networking for VM Spaces: the members of one tree reach each other
//! over TCP, as containers of one tree do on their tree's own network.
//!
//! Containers of a tree share a Docker network. VMs can't: each runs behind
//! a user-mode network of its own (the Cua network gateway on macOS and
//! Windows, a pasta namespace on Linux), where every guest has the same
//! address. So:
//!
//! * **Tree addresses.** Each VM gets an address in [`RANGE`]
//!   (`198.19.0.0/16`, the upper half of the benchmarking range, which no
//!   LAN or VPN uses), kept across restarts in this computer's book
//!   ([`book_path`]). `cua network show` and Space listings show it.
//! * **The rules decide first.** A VM's network context has [`RANGE`] as its
//!   tree ([`context_tree`]), so its compiled rules judge a connection to a
//!   member like any other: platform denies, then the user's lists (an
//!   explicit deny of a member wins), then the tree's allow, then the
//!   defaults. `none` has no tree allow, so it reaches no member.
//! * **Then membership.** The gateway (or, on Linux, the tree relay in the
//!   VM's namespace, `cua-netgw -tree-relay`) carries an allowed connection
//!   only to a *running member of the same tree*: it opens that member's
//!   ingress socket (a Unix socket in a private directory,
//!   [`sock_path`]) and asks for the port; the member's side connects to its
//!   guest on it. Anything else in [`RANGE`] (another tree, a stopped member,
//!   an address nobody holds) is refused and logged as `platform:other-tree`.
//!   Each VM's [`PEERS_FILE`] lists its tree's other running members; this
//!   module rewrites the files of a whole tree whenever one of its members
//!   starts, stops or changes its network, and the gateway or relay reloads
//!   it.
//! * **Ingress.** A member takes tree connections only while its own
//!   settings reach its tree ([`ingress_for`]: not `none`).
//!
//! * **Names.** Each member is also `<space>.<tree>` ([`member_name`]); the
//!   peers file lists every member's name, and the gateway's resolver
//!   answers them (and `host`, [`HOST_NAME`]).
//! * **Containers.** A container in a tree with a VM member gets a tree
//!   address too ([`join_hub`]); the Cua daemon answers its ingress socket
//!   and carries the connection into the container over its own tunnel, so
//!   VMs reach it by address or name.
//!
//! * **Other machines.** A tree can span the account's machines. A member
//!   that runs on another computer is recorded here as a *remote* member
//!   ([`join_remote`]) with the address its own machine gave it, and its
//!   ingress socket is served by this computer's Cua daemon, which carries
//!   the connection over the mesh (`cua-spaces`, `overlay.rs`). The gateway
//!   and the tree relay see no difference: a remote member is a peer with a
//!   socket. Tree ids that cross machines are qualified with the id of the
//!   machine the tree started on ([`qualify`]), so two computers' `local:app`
//!   trees never merge. A machine hands out addresses from a block of its
//!   own (see [`block_start`]), so two computers rarely give the same address; a
//!   clash is refused (that member stays unreachable and `cua network show`
//!   says so).
//!
//! What it doesn't do (and the docs say): TCP only (no UDP or ICMP between
//! VM members); the receiving guest sees the connection come from its own
//! gateway, not from the member's tree address; a container reaches a VM's
//! (or another computer's) tree members through its tree proxy, not by a
//! plain connect to the address (which fails closed).

use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::egress::{Net, NsHook};
use crate::error::{Result, VmmError};
use crate::host;
use crate::network::{NetworkPreset, NetworkSettings};

/// Where VM tree addresses come from.
pub const RANGE: &str = "198.19.0.0/16";
/// A VM's view of its tree, in its directory: its own address, whether it
/// takes connections from its tree, and its tree's other running members
/// (tree address → ingress socket).
pub const PEERS_FILE: &str = "tree.json";
/// The tree relay's configuration, in a Linux VM's directory.
pub const RELAY_CONFIG: &str = "tree-relay.json";
/// The tree relay's log, in a Linux VM's directory.
pub const RELAY_LOG: &str = "tree-relay.log";
/// The port the tree relay listens on inside a Linux VM's namespace (the
/// guest's TCP to [`RANGE`] is redirected there).
pub const RELAY_PORT: u16 = 41_919;

/// What a Linux VM can't do without the relay.
pub const RELAY_MISSING: &str = "this VM's tree has other running members, and VM Spaces reach \
     their tree through the tree relay in cua-netgw, which is not installed. cua-netgw ships next \
     to `cua`; set CUA_NETGW to its path or put it in ~/.cua/bin (libs/cua/netgw/build.sh builds \
     it). Without it this VM would be cut off from its tree, so it does not start";

/// What a running Linux VM that started alone, without the relay, is told
/// when members of its tree start later: it can't see them, nor they it.
pub const RELAY_LATE: &str = "this VM started alone, with no tree relay (cua-netgw is not \
     installed), so it can't reach the members that started after it and they can't reach it. \
     Install cua-netgw (it ships next to `cua`; set CUA_NETGW to its path) and restart this VM";

/// Whether tree networking is in force: it is part of Secure Spaces
/// ([`crate::network::Mode`]); without the experiment the namespaces and
/// gateways are set up exactly as before.
pub fn enabled() -> bool {
    #[cfg(test)]
    if let Some(v) = TEST_ENABLED.with(|c| c.get()) {
        return v;
    }
    crate::network::Mode::from_env() == crate::network::Mode::Secure
}

#[cfg(test)]
thread_local! {
    static TEST_ENABLED: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// Turns tree networking on for the calling test thread.
#[cfg(test)]
pub(crate) fn force_enabled_in_this_test() {
    TEST_ENABLED.with(|c| c.set(Some(true)));
}

/// The tree range as a network, for [`crate::network::NetworkContext::tree`].
pub fn context_tree() -> Vec<Net> {
    vec![Net::parse(RANGE).expect("a valid range")]
}

/// The key of a VM in the book: `<runtime>:<name>` (`microvm:studio`).
pub fn key(runtime: &str, name: &str) -> String {
    format!("{runtime}:{name}")
}

/// Whether a VM with `settings` (and `restrict`: no network at all) takes
/// connections from its tree: whenever its own settings reach its tree.
pub fn ingress_for(settings: Option<&NetworkSettings>, restrict: bool) -> bool {
    !restrict && settings.is_none_or(|s| s.preset != NetworkPreset::None)
}

/// One VM in the book.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Entry {
    /// Its tree (the top-level Space's id); its own key when it has none.
    tree: String,
    addr: Ipv4Addr,
    /// Its directory, where [`PEERS_FILE`] goes (`None`: never started).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dir: Option<PathBuf>,
    /// Its ingress socket while it runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sock: Option<String>,
    #[serde(default)]
    running: bool,
    #[serde(default)]
    ingress: bool,
    /// A member that runs on another computer (its ingress socket is this
    /// machine's daemon carrying it over the mesh).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    remote: bool,
    /// For a remote member: the computer that runs it (its overlay id).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    machine: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Book {
    #[serde(default)]
    entries: BTreeMap<String, Entry>,
    /// The second octet new addresses start from (a block of this computer's
    /// own, from its overlay id). `None`: a book from before it, which
    /// gives out the lowest free address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    start: Option<u8>,
}

/// This computer's book of VM tree addresses and members.
pub fn book_path() -> PathBuf {
    host::cua_home()
        .join("net")
        .join("tree")
        .join("members.json")
}

/// The book, locked for the length of `f` (every runtime's VMs share it).
fn with_book<T>(f: impl FnOnce(&mut Book) -> Result<T>) -> Result<T> {
    let start = block_start(&overlay_id());
    with_book_at(&book_path(), |b| {
        // A new book takes its machine's block; one with addresses keeps
        // handing out from where it did.
        if b.start.is_none() && b.entries.is_empty() {
            b.start = Some(start);
        }
        f(b)
    })
}

fn with_book_at<T>(path: &Path, f: impl FnOnce(&mut Book) -> Result<T>) -> Result<T> {
    let dir = path.parent().expect("the book has a folder");
    std::fs::create_dir_all(dir)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path.with_extension("lock"))?;
    lock.lock()
        .map_err(|e| VmmError::other(format!("lock {}: {e}", path.display())))?;
    let mut book: Book = std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    // A deleted VM's directory is gone: so is its address.
    book.entries
        .retain(|_, e| e.dir.as_ref().is_none_or(|d| d.exists()));
    let out = f(&mut book)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(&book)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(out)
}

/// The lowest address of [`RANGE`] no one holds, from this book's block
/// (never `.0` or `.255` of a /24).
fn allocate(book: &Book) -> Result<Ipv4Addr> {
    let taken: std::collections::BTreeSet<Ipv4Addr> =
        book.entries.values().map(|e| e.addr).collect();
    let start = book.start.unwrap_or(0);
    for step in 0..=255u16 {
        let hi = ((u16::from(start) + step) % 256) as u8;
        for lo in 1..=254u8 {
            let a = Ipv4Addr::new(198, 19, hi, lo);
            if !taken.contains(&a) {
                return Ok(a);
            }
        }
    }
    Err(VmmError::other(
        "this computer has given out every VM tree address (198.19.0.0/16)",
    ))
}

/// The block of [`RANGE`] (its second-to-last octet) machine `id` starts
/// handing out addresses from: a hash of its overlay id, so the computers of
/// one account rarely give two members the same address.
pub fn block_start(id: &str) -> u8 {
    let d = Sha256::digest(format!("cua-tree-block/v1\n{id}").as_bytes());
    d[0]
}

/// Where this computer keeps its overlay id.
pub fn overlay_id_path() -> PathBuf {
    host::cua_home().join("net").join("tree").join("overlay-id")
}

/// This computer's id for the tree overlay: 16 hex digits made once and kept
/// in the Cua home. It names the computer in tree ids that cross machines
/// ([`qualify`]). Not a secret and not an identity: the mesh decides who
/// may talk to this computer.
pub fn overlay_id() -> String {
    overlay_id_at(&overlay_id_path())
}

fn overlay_id_at(path: &Path) -> String {
    if let Some(id) = std::fs::read_to_string(path)
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| t.len() == 16 && t.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return id;
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let d = Sha256::digest(
        format!(
            "cua-tree-id/v1\n{}\n{nanos}\n{}",
            path.display(),
            std::process::id()
        )
        .as_bytes(),
    );
    let id: String = d.iter().take(8).map(|b| format!("{b:02x}")).collect();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // Two processes starting at once: the first file wins.
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    if std::fs::write(&tmp, &id).is_ok() {
        if std::fs::hard_link(&tmp, path).is_err() {
            let _ = std::fs::remove_file(&tmp);
            if let Ok(t) = std::fs::read_to_string(path) {
                return t.trim().to_string();
            }
        } else {
            let _ = std::fs::remove_file(&tmp);
        }
    }
    id
}

/// Tree `tree` as every machine names it: its own id with `machine` (the
/// overlay id of the computer the tree started on) in front. An id that
/// already has one is kept.
pub fn qualify(machine: &str, tree: &str) -> String {
    if tree.contains('/') {
        tree.to_string()
    } else {
        format!("{machine}/{tree}")
    }
}

/// [`qualify`] undone for machine `machine`: a tree that started here is
/// its plain id again (that is how the book keeps it).
pub fn unqualify(machine: &str, tree: &str) -> String {
    tree.strip_prefix(&format!("{machine}/"))
        .unwrap_or(tree)
        .to_string()
}

/// The tree of a top-level member: its own Space id (`local:<name>`), the
/// id its children name as their tree, whatever runtime runs it (the book
/// key is `<runtime>:<name>`).
pub fn own_tree(key: &str) -> String {
    format!(
        "local:{}",
        key.split_once(':').map(|(_, n)| n).unwrap_or(key)
    )
}

fn entry<'a>(book: &'a mut Book, key: &str, tree: Option<&str>) -> Result<&'a mut Entry> {
    if !book.entries.contains_key(key) {
        let addr = allocate(book)?;
        book.entries.insert(
            key.to_string(),
            Entry {
                tree: tree.map(str::to_string).unwrap_or_else(|| own_tree(key)),
                addr,
                dir: None,
                sock: None,
                running: false,
                ingress: false,
                remote: false,
                machine: None,
            },
        );
    }
    let e = book.entries.get_mut(key).expect("just inserted");
    if let Some(t) = tree {
        e.tree = t.to_string();
    } else if e.tree == key {
        // Books written before: a top-level member's tree was its key.
        e.tree = own_tree(key);
    }
    Ok(e)
}

/// Records VM `key` in `tree` (`None`: keeps what the book says, else a
/// tree of its own) and returns its tree address. Creates call it with
/// the start spec's tree.
pub fn assign(key: &str, tree: Option<&str>) -> Result<Ipv4Addr> {
    if !enabled() {
        return Ok(Ipv4Addr::UNSPECIFIED);
    }
    with_book(|b| Ok(entry(b, key, tree)?.addr))
}

/// What a starting VM needs from its tree ([`join`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Joined {
    /// Its tree address.
    pub addr: Ipv4Addr,
    /// Its ingress socket's path.
    pub sock: String,
    /// Other members of its tree run now.
    pub has_peers: bool,
}

/// Marks VM `key` (in `dir`) running in its tree and rewrites the tree's
/// [`PEERS_FILE`]s (its own included). `ingress`: [`ingress_for`].
pub fn join(key: &str, tree: Option<&str>, dir: &Path, ingress: bool) -> Result<Joined> {
    with_book(|b| {
        let e = entry(b, key, tree)?;
        let sock = sock_path(e.addr).display().to_string();
        e.dir = Some(dir.to_path_buf());
        e.sock = Some(sock.clone());
        e.running = true;
        e.ingress = ingress;
        let (addr, tree) = (e.addr, e.tree.clone());
        let has_peers = b
            .entries
            .iter()
            .any(|(k, o)| k != key && o.tree == tree && o.running);
        write_tree(b, &tree)?;
        Ok(Joined {
            addr,
            sock,
            has_peers,
        })
    })
}

/// Records a member that isn't a VM (a container, reached through the
/// Cua daemon's tree hub on its ingress socket) running in `tree`, so the
/// tree's VMs reach it at its tree address and by name. It has no peers
/// file of its own. Undo with [`leave`].
pub fn join_hub(key: &str, tree: &str, ingress: bool) -> Result<Joined> {
    with_book(|b| {
        let e = entry(b, key, Some(tree))?;
        let sock = sock_path(e.addr).display().to_string();
        e.sock = Some(sock.clone());
        e.running = true;
        e.ingress = ingress;
        let addr = e.addr;
        let has_peers = b
            .entries
            .iter()
            .any(|(k, o)| k != key && o.tree == tree && o.running);
        write_tree(b, tree)?;
        Ok(Joined {
            addr,
            sock,
            has_peers,
        })
    })
}

/// One member this computer runs, for the other computers of the account
/// ([`local_members`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalMember {
    /// Its book key (`<runtime>:<name>`).
    pub key: String,
    /// Its tree, qualified ([`qualify`]).
    pub tree: String,
    /// Its tree address.
    pub addr: Ipv4Addr,
    /// It takes connections from its tree.
    pub ingress: bool,
}

/// The members of other computers' trees this computer has recorded
/// ([`join_remote`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteMember {
    /// Its book key (`remote:<machine>:<name>`).
    pub key: String,
    /// Its tree, as the book keeps it (a plain id when the tree started on
    /// this computer).
    pub tree: String,
    /// Its tree address.
    pub addr: Ipv4Addr,
    /// The computer that runs it (its overlay id).
    pub machine: String,
    /// It takes connections from its tree.
    pub ingress: bool,
    /// Its ingress socket (this computer's daemon carries it).
    pub sock: String,
}

/// The book key of member `name` that runs on `machine`.
pub fn remote_key(machine: &str, name: &str) -> String {
    format!("remote:{machine}:{name}")
}

/// Records a member that runs on another computer as running in `tree`,
/// with `addr` (the address its own machine gave it), so this computer's VMs
/// reach it at that address and by name. Its ingress socket is served by
/// this computer's daemon. Fails when `addr` already belongs to another
/// member (two computers gave the same address): that member stays
/// unreachable. Undo with [`forget_remote`].
pub fn join_remote(
    key: &str,
    machine: &str,
    tree: &str,
    addr: Ipv4Addr,
    ingress: bool,
) -> Result<Joined> {
    let range = Net::parse(RANGE).expect("a valid range");
    if !range.contains(addr.into()) {
        return Err(VmmError::other(format!(
            "{addr} is not a tree address ({RANGE})"
        )));
    }
    with_book(|b| {
        if let Some((other, _)) = b
            .entries
            .iter()
            .find(|(k, e)| k.as_str() != key && e.addr == addr)
        {
            return Err(VmmError::other(format!(
                "the tree address {addr} of {key} is already {other}'s on this computer"
            )));
        }
        let e = b.entries.entry(key.to_string()).or_insert_with(|| Entry {
            tree: tree.to_string(),
            addr,
            dir: None,
            sock: None,
            running: false,
            ingress: false,
            remote: true,
            machine: Some(machine.to_string()),
        });
        e.tree = tree.to_string();
        e.addr = addr;
        e.remote = true;
        e.machine = Some(machine.to_string());
        let sock = sock_path(addr).display().to_string();
        e.sock = Some(sock.clone());
        e.running = true;
        e.ingress = ingress;
        let has_peers = b
            .entries
            .iter()
            .any(|(k, o)| k != key && o.tree == tree && o.running);
        write_tree(b, tree)?;
        Ok(Joined {
            addr,
            sock,
            has_peers,
        })
    })
}

/// Drops remote member `key` (it stopped, its machine went away, or it left
/// the tree) and tells its tree. Its address is free again.
pub fn forget_remote(key: &str) -> Result<()> {
    with_book(|b| {
        let Some(e) = b.entries.remove(key) else {
            return Ok(());
        };
        if !e.remote {
            // Not ours to drop: put it back.
            b.entries.insert(key.to_string(), e);
            return Ok(());
        }
        write_tree(b, &e.tree)
    })
}

/// The members of other computers' trees this computer has recorded.
pub fn remote_members() -> Vec<RemoteMember> {
    let Some(b) = std::fs::read(book_path())
        .ok()
        .and_then(|b| serde_json::from_slice::<Book>(&b).ok())
    else {
        return Vec::new();
    };
    b.entries
        .into_iter()
        .filter(|(_, e)| e.remote)
        .map(|(k, e)| RemoteMember {
            key: k,
            tree: e.tree,
            addr: e.addr,
            machine: e.machine.unwrap_or_default(),
            ingress: e.ingress,
            sock: e.sock.unwrap_or_default(),
        })
        .collect()
}

/// The running members this computer runs itself (VMs and the containers its
/// hub carries), for the other computers of the account.
pub fn local_members() -> Vec<LocalMember> {
    let Some(b) = std::fs::read(book_path())
        .ok()
        .and_then(|b| serde_json::from_slice::<Book>(&b).ok())
    else {
        return Vec::new();
    };
    let me = overlay_id();
    b.entries
        .into_iter()
        .filter(|(_, e)| e.running && !e.remote)
        .map(|(k, e)| LocalMember {
            key: k,
            tree: qualify(&me, &e.tree),
            addr: e.addr,
            ingress: e.ingress,
        })
        .collect()
}

/// The running members of `tree` (book keys, `<runtime>:<name>`).
pub fn running_members(tree: &str) -> Vec<String> {
    let Ok(Some(b)) = std::fs::read(book_path()).map(|b| serde_json::from_slice::<Book>(&b).ok())
    else {
        return Vec::new();
    };
    b.entries
        .into_iter()
        .filter(|(_, e)| e.tree == tree && e.running)
        .map(|(k, _)| k)
        .collect()
}

/// Whether tree address `addr` belongs to a running member of `tree`.
pub fn is_running_member(tree: &str, addr: Ipv4Addr) -> bool {
    let Ok(Some(b)) = std::fs::read(book_path()).map(|b| serde_json::from_slice::<Book>(&b).ok())
    else {
        return false;
    };
    b.entries
        .values()
        .any(|e| e.tree == tree && e.running && e.addr == addr)
}

/// Whether the running VM `key` runs without a tree relay (it started alone
/// and `cua-netgw` was not installed: [`prepare_linux`] wrote no
/// [`RELAY_CONFIG`]) while other members of its tree run now. It then can't
/// see those members, and they can't see it, until it restarts with the
/// relay installed. Only a Linux VM has a relay: the caller says it is one.
pub fn relay_gap(key: &str) -> bool {
    let Ok(Some(b)) = std::fs::read(book_path()).map(|b| serde_json::from_slice::<Book>(&b).ok())
    else {
        return false;
    };
    relay_gap_in(&b, key)
}

fn relay_gap_in(b: &Book, key: &str) -> bool {
    let Some(me) = b.entries.get(key) else {
        return false;
    };
    let (Some(dir), true, false) = (&me.dir, me.running, me.remote) else {
        return false;
    };
    !dir.join(RELAY_CONFIG).exists()
        && b.entries
            .iter()
            .any(|(k, o)| k != key && o.tree == me.tree && o.running)
}

/// Whether VM `key` would have running peers in `tree` if it started now.
pub fn has_peers(key: &str, tree: Option<&str>) -> bool {
    let Ok(Some(b)) = std::fs::read(book_path()).map(|b| serde_json::from_slice::<Book>(&b).ok())
    else {
        return false;
    };
    let tree = tree
        .map(str::to_string)
        .or_else(|| b.entries.get(key).map(|e| e.tree.clone()))
        .unwrap_or_else(|| own_tree(key));
    b.entries
        .iter()
        .any(|(k, e)| k != key && e.tree == tree && e.running)
}

/// Marks VM `key` stopped and tells its tree.
pub fn leave(key: &str) -> Result<()> {
    if !enabled() {
        return Ok(());
    }
    with_book(|b| {
        let Some(e) = b.entries.get_mut(key) else {
            return Ok(());
        };
        e.running = false;
        let tree = e.tree.clone();
        if let Some(d) = &e.dir {
            let _ = std::fs::remove_file(d.join(PEERS_FILE));
        }
        write_tree(b, &tree)
    })
}

/// A running VM's settings changed: whether it takes tree connections
/// now ([`ingress_for`]).
pub fn set_ingress(key: &str, ingress: bool) -> Result<()> {
    if !enabled() {
        return Ok(());
    }
    with_book(|b| {
        let Some(e) = b.entries.get_mut(key) else {
            return Ok(());
        };
        e.ingress = ingress;
        let tree = e.tree.clone();
        write_tree(b, &tree)
    })
}

/// Every VM's tree address on this computer, by name (any runtime).
pub fn addresses() -> BTreeMap<String, Ipv4Addr> {
    let Some(b) = std::fs::read(book_path())
        .ok()
        .and_then(|b| serde_json::from_slice::<Book>(&b).ok())
    else {
        return BTreeMap::new();
    };
    b.entries
        .iter()
        .filter_map(|(k, e)| Some((k.split_once(':')?.1.to_string(), e.addr)))
        .collect()
}

/// The book keys (`<runtime>:<name>`) of the VMs named `name`, any runtime.
pub fn keys_named(name: &str) -> Vec<String> {
    let Some(b) = std::fs::read(book_path())
        .ok()
        .and_then(|b| serde_json::from_slice::<Book>(&b).ok())
    else {
        return Vec::new();
    };
    b.entries
        .keys()
        .filter(|k| k.split_once(':').is_some_and(|(_, n)| n == name))
        .cloned()
        .collect()
}

/// The tree address of the VM named `name` (any runtime), if it has one.
pub fn address_by_name(name: &str) -> Option<Ipv4Addr> {
    let b: Book = serde_json::from_slice(&std::fs::read(book_path()).ok()?).ok()?;
    b.entries
        .iter()
        .find(|(k, _)| k.split_once(':').is_some_and(|(_, n)| n == name))
        .map(|(_, e)| e.addr)
}

/// The name a Space uses for the computer that runs it, on every runtime.
pub const HOST_NAME: &str = "host";

/// `s` as a DNS label: lowercase letters, digits and `-`, at most 63
/// characters, never empty (`space`).
pub fn dns_label(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| {
            let c = c.to_ascii_lowercase();
            if c.is_ascii_alphanumeric() { c } else { '-' }
        })
        .collect();
    while out.contains("--") {
        out = out.replace("--", "-");
    }
    let out: String = out.trim_matches('-').chars().take(63).collect();
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() { "space".into() } else { out }
}

/// The name of a tree member: `<space>.<tree>`, from the Space's id and
/// its tree's (the top-level Space's) id, each by the name after `:`.
pub fn member_name(space_id: &str, tree_id: &str) -> String {
    let short = |id: &str| id.rsplit(':').next().unwrap_or(id).to_string();
    format!(
        "{}.{}",
        dns_label(&short(space_id)),
        dns_label(&short(tree_id))
    )
}

/// [`PEERS_FILE`] of member `me` of `book`.
fn peers_of(book: &Book, me: &str) -> Option<Value> {
    let e = book.entries.get(me)?;
    let peers: BTreeMap<String, String> = book
        .entries
        .iter()
        .filter(|(k, o)| k.as_str() != me && o.tree == e.tree && o.running)
        .filter_map(|(_, o)| Some((o.addr.to_string(), o.sock.clone()?)))
        .collect();
    // Every member's name, running or not (a name that answers before its
    // Space runs just gets a refused connection, as its address does).
    let names: BTreeMap<String, String> = book
        .entries
        .iter()
        .filter(|(_, o)| o.tree == e.tree)
        .map(|(k, o)| (member_name(k, &e.tree), o.addr.to_string()))
        .collect();
    Some(json!({
        "version": 1,
        "self": e.addr.to_string(),
        "ingress": e.ingress,
        "peers": peers,
        "names": names,
    }))
}

/// Rewrites the [`PEERS_FILE`] of every running member of `tree`.
fn write_tree(book: &Book, tree: &str) -> Result<()> {
    for (k, e) in book
        .entries
        .iter()
        .filter(|(_, e)| e.tree == tree && e.running)
    {
        let (Some(dir), Some(v)) = (&e.dir, peers_of(book, k)) else {
            continue;
        };
        if !dir.exists() {
            continue;
        }
        let tmp = dir.join(format!("{PEERS_FILE}.tmp"));
        std::fs::write(&tmp, serde_json::to_vec_pretty(&v)?)?;
        std::fs::rename(&tmp, dir.join(PEERS_FILE))?;
    }
    Ok(())
}

/// Where the VM holding `addr` listens for its tree's connections:
/// `<tmp>/cua-tree-<uid>/<addr>.sock`. The folder is created 0700; one that
/// exists must be ours and 0700, else a fresh random one is used (another
/// user could have made it first in a shared `/tmp`).
pub fn sock_path(addr: Ipv4Addr) -> PathBuf {
    private_dir().join(format!("{addr}.sock"))
}

#[cfg(unix)]
fn private_dir() -> PathBuf {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
    // SAFETY: getuid has no preconditions.
    let uid = unsafe { libc::getuid() };
    let ours = |p: &Path| {
        std::fs::symlink_metadata(p)
            .is_ok_and(|m| m.is_dir() && m.uid() == uid && m.permissions().mode() & 0o077 == 0)
    };
    let make = |p: &Path| {
        let _ = std::fs::create_dir(p);
        let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700));
    };
    let private = std::env::temp_dir().join(format!("cua-tree-{uid}"));
    if !private.exists() {
        make(&private);
    }
    if ours(&private) {
        return private;
    }
    // Stable for this process, so a VM's address keeps one socket path.
    static FALLBACK: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    FALLBACK
        .get_or_init(|| {
            let p = std::env::temp_dir().join(format!("cua-tree-{uid}-{}", std::process::id()));
            make(&p);
            p
        })
        .clone()
}

#[cfg(not(unix))]
fn private_dir() -> PathBuf {
    let p = std::env::temp_dir().join("cua-tree");
    let _ = std::fs::create_dir_all(&p);
    p
}

/// The gateway's `tree` section ([`crate::netgw::Network::tree`]): its
/// [`PEERS_FILE`], its ingress socket and the guest's address (where tree
/// connections to it go).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GatewayTree {
    pub sock: String,
    pub guest_ip: String,
}

impl GatewayTree {
    /// The JSON the gateway reads.
    pub fn config(&self) -> Value {
        json!({
            "peers": PEERS_FILE,
            "ingress": self.sock,
            "guest_ip": self.guest_ip,
        })
    }
}

/// [`RELAY_CONFIG`] for a Linux VM: its [`PEERS_FILE`], its ingress socket,
/// the guest's address in its namespace, where the relay listens, its
/// events file, and for QEMU the relay's own monitor socket (it publishes a
/// guest port on the namespace's loopback when a member asks for it).
pub fn relay_config(sock: &str, guest_ip: &str, qmp: Option<&str>) -> Value {
    let mut v = json!({
        "peers": PEERS_FILE,
        "ingress": sock,
        "guest_ip": guest_ip,
        "listen": format!("0.0.0.0:{RELAY_PORT}"),
        "events": crate::netgw::EVENTS_FILE,
    });
    if let Some(q) = qmp {
        v["qmp"] = json!(q);
    }
    v
}

/// The `iptables-restore` lines a Linux VM's namespace needs for its tree:
/// in `*nat`, the guest's TCP to [`RANGE`] goes to the relay (its rules
/// still judge the original destination: [`crate::egress::ipt_rule`] matches
/// `--ctorigdst`); in `*filter`, anything else to [`RANGE`] (UDP, ICMP) is
/// dropped, as the tree carries TCP only.
pub fn ns_nat(hook: NsHook) -> String {
    if !enabled() {
        return String::new();
    }
    match hook {
        NsHook::Tap(dev) => format!(
            "-A PREROUTING -i {dev} -d {RANGE} -p tcp -j REDIRECT --to-ports {RELAY_PORT}\n"
        ),
        NsHook::Output => {
            format!("-A OUTPUT -d {RANGE} -p tcp -j REDIRECT --to-ports {RELAY_PORT}\n")
        }
    }
}

/// See [`ns_nat`].
pub fn ns_filter(hook: NsHook) -> String {
    if !enabled() {
        return String::new();
    }
    match hook {
        NsHook::Tap(dev) => format!("-A FORWARD -i {dev} -d {RANGE} -j DROP\n"),
        // After the redirect a tree connection goes to the loopback: what
        // still names the range is not TCP.
        NsHook::Output => format!("-A OUTPUT -d {RANGE} -j DROP\n"),
    }
}

/// [`ns_nat`] and [`ns_filter`] as shell commands, for a namespace a
/// script sets up (QEMU's, after its rules): the drop goes first in its
/// chain, before the jump to the rules. A host that refuses the redirect
/// (no `nat` table in the namespace) only loses the tree: the drop still
/// stops the guest's connections to it.
pub fn ns_shell(hook: NsHook) -> String {
    if !enabled() {
        return String::new();
    }
    let nat = ns_nat(hook);
    let filter = ns_filter(hook);
    let drop = filter
        .trim_end()
        .strip_prefix("-A ")
        .and_then(|r| r.split_once(' '))
        .map(|(chain, rest)| format!("iptables -I {chain} 1 {rest}\n"))
        .unwrap_or_default();
    format!(
        "iptables -t nat {} || echo 'cua: no redirect to the tree relay here; this VM is cut off from its tree' >&2\n{drop}",
        nat.trim_end()
    )
}

/// The shell lines that start the tree relay in a Linux VM's namespace
/// (run in the VM's directory, before the monitor is exec'd, so the relay
/// ends with the namespace's last process). Nothing when `$CUA_NETGW_BIN`
/// is empty.
pub fn relay_start() -> String {
    if !enabled() {
        return String::new();
    }
    format!(
        "if [ -n \"${{CUA_NETGW_BIN:-}}\" ] && [ -f {RELAY_CONFIG} ]; then\n\
         \x20 \"$CUA_NETGW_BIN\" -tree-relay {RELAY_CONFIG} >>{RELAY_LOG} 2>&1 &\n\
         fi\n"
    )
}

/// Prepares a Linux VM's tree before its namespace starts: joins `tree` and
/// writes [`RELAY_CONFIG`]. Returns the relay's binary (for
/// `$CUA_NETGW_BIN`), or `None` when it isn't installed and the VM is alone
/// in its tree. Fails closed: refuses a VM whose tree has running members
/// when the relay is missing.
pub fn prepare_linux(
    key: &str,
    tree: Option<&str>,
    dir: &Path,
    ingress: bool,
    guest_ip: &str,
    qmp: Option<&str>,
) -> Result<Option<PathBuf>> {
    if !enabled() {
        return Ok(None);
    }
    let joined = join(key, tree, dir, ingress)?;
    let Some(bin) = crate::netgw::bin() else {
        let _ = std::fs::remove_file(dir.join(RELAY_CONFIG));
        if joined.has_peers {
            let _ = leave(key);
            return Err(VmmError::missing("cua-netgw", RELAY_MISSING));
        }
        return Ok(None);
    };
    std::fs::write(
        dir.join(RELAY_CONFIG),
        serde_json::to_vec_pretty(&relay_config(&joined.sock, guest_ip, qmp))?,
    )?;
    Ok(Some(bin))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book(dir: &Path) -> PathBuf {
        dir.join("book").join("members.json")
    }

    #[test]
    fn a_vm_that_started_alone_without_the_relay_is_told_about_later_members() {
        let t = tempfile::tempdir().unwrap();
        let p = book(t.path());
        let (vm_dir, other_dir) = (t.path().join("vm"), t.path().join("other"));
        std::fs::create_dir_all(&vm_dir).unwrap();
        std::fs::create_dir_all(&other_dir).unwrap();
        let book_of = || -> Book { serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap() };
        with_book_at(&p, |b| {
            let e = entry(b, "qemu:alone", Some("local:root"))?;
            e.dir = Some(vm_dir.clone());
            e.running = true;
            Ok(())
        })
        .unwrap();
        // Alone: nothing to be cut off from.
        assert!(!relay_gap_in(&book_of(), "qemu:alone"));
        // A member starts later (a container needs no directory): the VM has no relay.
        with_book_at(&p, |b| {
            let e = entry(b, "docker:late", Some("local:root"))?;
            e.running = true;
            Ok(())
        })
        .unwrap();
        assert!(relay_gap_in(&book_of(), "qemu:alone"));
        // With the relay's config (cua-netgw installed) there is no gap.
        std::fs::write(vm_dir.join(RELAY_CONFIG), "{}").unwrap();
        assert!(!relay_gap_in(&book_of(), "qemu:alone"));
        std::fs::remove_file(vm_dir.join(RELAY_CONFIG)).unwrap();
        // Another tree's members, a stopped one and an unknown key are no gap.
        with_book_at(&p, |b| {
            b.entries.get_mut("docker:late").unwrap().running = false;
            let e = entry(b, "qemu:elsewhere", Some("local:other"))?;
            e.dir = Some(other_dir.clone());
            e.running = true;
            Ok(())
        })
        .unwrap();
        assert!(!relay_gap_in(&book_of(), "qemu:alone"));
        assert!(!relay_gap_in(&book_of(), "qemu:nobody"));
        assert!(RELAY_LATE.contains("restart this VM"));
    }

    #[test]
    fn addresses_are_kept_and_never_given_twice() {
        let t = tempfile::tempdir().unwrap();
        let p = book(t.path());
        let a = with_book_at(&p, |b| Ok(entry(b, "microvm:a", Some("local:root"))?.addr)).unwrap();
        let b = with_book_at(&p, |b| Ok(entry(b, "qemu:b", Some("local:root"))?.addr)).unwrap();
        assert_eq!(a, Ipv4Addr::new(198, 19, 0, 1));
        assert_eq!(b, Ipv4Addr::new(198, 19, 0, 2));
        // Asked again: the same address.
        let again = with_book_at(&p, |b| Ok(entry(b, "microvm:a", None)?.addr)).unwrap();
        assert_eq!(again, a);
        assert!(Net::parse(RANGE).unwrap().contains(a.into()));
    }

    #[test]
    fn peers_are_the_running_members_of_the_same_tree_only() {
        let t = tempfile::tempdir().unwrap();
        let p = book(t.path());
        let dirs: Vec<PathBuf> = ["a", "b", "c", "d"]
            .iter()
            .map(|n| {
                let d = t.path().join(n);
                std::fs::create_dir_all(&d).unwrap();
                d
            })
            .collect();
        with_book_at(&p, |b| {
            for (i, (k, tree, running)) in [
                ("microvm:a", "local:root", true),
                ("qemu:b", "local:root", true),
                ("lume:c", "local:other", true),
                ("qemu:d", "local:root", false),
            ]
            .into_iter()
            .enumerate()
            {
                let e = entry(b, k, Some(tree))?;
                e.dir = Some(dirs[i].clone());
                e.sock = Some(format!("/s/{k}.sock"));
                e.running = running;
                e.ingress = true;
            }
            write_tree(b, "local:root")?;
            write_tree(b, "local:other")
        })
        .unwrap();
        let read = |d: &Path| -> Value {
            serde_json::from_slice(&std::fs::read(d.join(PEERS_FILE)).unwrap()).unwrap()
        };
        let a = read(&dirs[0]);
        assert_eq!(a["self"], "198.19.0.1");
        assert_eq!(a["ingress"], true);
        let peers = a["peers"].as_object().unwrap();
        // b only: c is another tree's, d is stopped.
        assert_eq!(peers.len(), 1, "{a}");
        assert_eq!(peers["198.19.0.2"], "/s/qemu:b.sock");
        // Names: every member of its own tree (stopped ones too), no other.
        let names = a["names"].as_object().unwrap();
        assert_eq!(names["a.root"], "198.19.0.1", "{a}");
        assert_eq!(names["b.root"], "198.19.0.2", "{a}");
        assert!(names.contains_key("d.root"), "{a}");
        assert!(!names.contains_key("c.other"), "{a}");
        let c = read(&dirs[2]);
        assert!(c["peers"].as_object().unwrap().is_empty(), "{c}");
        assert!(
            !dirs[3].join(PEERS_FILE).exists(),
            "a stopped member gets no file"
        );
    }

    #[test]
    fn a_top_level_vm_shares_the_tree_its_children_name() {
        let t = tempfile::tempdir().unwrap();
        let p = book(t.path());
        with_book_at(&p, |b| {
            let root = entry(b, "microvm:root", None)?.tree.clone();
            let child = entry(b, "qemu:kid", Some("local:root"))?.tree.clone();
            assert_eq!(root, child, "one tree");
            Ok(())
        })
        .unwrap();
        assert_eq!(own_tree("lume:mac"), "local:mac");
    }

    #[test]
    fn member_names_are_dns_labels() {
        assert_eq!(
            member_name("qemu:Worker_1", "local:Research Swarm"),
            "worker-1.research-swarm"
        );
        assert_eq!(dns_label("--"), "space");
    }

    #[test]
    fn a_deleted_vm_gives_its_address_back() {
        let t = tempfile::tempdir().unwrap();
        let p = book(t.path());
        let gone = t.path().join("gone");
        std::fs::create_dir_all(&gone).unwrap();
        with_book_at(&p, |b| {
            entry(b, "microvm:gone", None)?.dir = Some(gone.clone());
            Ok(())
        })
        .unwrap();
        std::fs::remove_dir_all(&gone).unwrap();
        let next = with_book_at(&p, |b| Ok(entry(b, "microvm:new", None)?.addr)).unwrap();
        assert_eq!(next, Ipv4Addr::new(198, 19, 0, 1));
    }

    #[test]
    fn trees_that_cross_machines_are_qualified_by_their_first_machine() {
        assert_eq!(qualify("aa11", "local:app"), "aa11/local:app");
        assert_eq!(qualify("bb22", "aa11/local:app"), "aa11/local:app", "kept");
        assert_eq!(unqualify("aa11", "aa11/local:app"), "local:app");
        assert_eq!(unqualify("bb22", "aa11/local:app"), "aa11/local:app");
        // The name a member answers to is the tree's last part.
        assert_eq!(
            member_name("remote:bb22:worker", "aa11/local:app"),
            "worker.app"
        );
    }

    #[test]
    fn a_machines_addresses_start_in_its_own_block() {
        let t = tempfile::tempdir().unwrap();
        let p = book(t.path());
        assert_eq!(block_start("aa11"), block_start("aa11"));
        // Two ids rarely share a block (these two don't).
        assert_ne!(
            block_start("0123456789abcdef"),
            block_start("fedcba9876543210")
        );
        let start = block_start("0123456789abcdef");
        let a = with_book_at(&p, |b| {
            b.start = Some(start);
            Ok(entry(b, "qemu:a", None)?.addr)
        })
        .unwrap();
        assert_eq!(a, Ipv4Addr::new(198, 19, start, 1));
        let b2 = with_book_at(&p, |b| Ok(entry(b, "qemu:b", None)?.addr)).unwrap();
        assert_eq!(b2, Ipv4Addr::new(198, 19, start, 2), "the block is kept");
    }

    #[test]
    fn the_overlay_id_is_made_once_and_kept() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("net").join("tree").join("overlay-id");
        let a = overlay_id_at(&p);
        assert_eq!(a.len(), 16);
        assert!(a.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(overlay_id_at(&p), a);
        assert_eq!(std::fs::read_to_string(&p).unwrap().trim(), a);
    }

    #[test]
    fn a_remote_member_is_a_peer_with_a_socket_and_a_name() {
        let t = tempfile::tempdir().unwrap();
        let p = book(t.path());
        let d = t.path().join("a");
        std::fs::create_dir_all(&d).unwrap();
        with_book_at(&p, |b| {
            let e = entry(b, "microvm:a", Some("aa11/local:app"))?;
            e.dir = Some(d.clone());
            e.sock = Some("/s/a.sock".into());
            e.running = true;
            e.ingress = true;
            b.entries.insert(
                "remote:bb22:worker".into(),
                Entry {
                    tree: "aa11/local:app".into(),
                    addr: Ipv4Addr::new(198, 19, 7, 1),
                    dir: None,
                    sock: Some("/s/remote.sock".into()),
                    running: true,
                    ingress: true,
                    remote: true,
                    machine: Some("bb22".into()),
                },
            );
            write_tree(b, "aa11/local:app")
        })
        .unwrap();
        let v: Value = serde_json::from_slice(&std::fs::read(d.join(PEERS_FILE)).unwrap()).unwrap();
        assert_eq!(v["peers"]["198.19.7.1"], "/s/remote.sock", "{v}");
        assert_eq!(v["names"]["worker.app"], "198.19.7.1", "{v}");
        // A remote member has no directory, so it gets no peers file of its
        // own; the book keeps it apart from the computer's own members.
        let b: Book = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
        assert!(b.entries["remote:bb22:worker"].remote);
        assert!(!b.entries["microvm:a"].remote);
    }

    #[test]
    fn none_takes_no_tree_connections() {
        assert!(ingress_for(None, false));
        assert!(ingress_for(Some(&NetworkSettings::internet()), false));
        assert!(!ingress_for(Some(&NetworkSettings::none()), false));
        assert!(!ingress_for(Some(&NetworkSettings::internet()), true));
    }

    #[test]
    fn the_namespace_sends_tree_tcp_to_the_relay_and_drops_the_rest() {
        force_enabled_in_this_test();
        let nat = ns_nat(NsHook::Tap("tap0"));
        assert_eq!(
            nat,
            "-A PREROUTING -i tap0 -d 198.19.0.0/16 -p tcp -j REDIRECT --to-ports 41919\n"
        );
        assert!(
            ns_filter(NsHook::Tap("tap0")).contains("-A FORWARD -i tap0 -d 198.19.0.0/16 -j DROP")
        );
        assert!(
            ns_nat(NsHook::Output).starts_with("-A OUTPUT -d 198.19.0.0/16 -p tcp -j REDIRECT")
        );
        assert!(relay_start().contains("-tree-relay tree-relay.json"));
        let sh = ns_shell(NsHook::Output);
        assert!(
            sh.starts_with(
                "iptables -t nat -A OUTPUT -d 198.19.0.0/16 -p tcp -j REDIRECT --to-ports 41919 ||"
            ),
            "{sh}"
        );
        assert!(
            sh.ends_with("iptables -I OUTPUT 1 -d 198.19.0.0/16 -j DROP\n"),
            "{sh}"
        );
        let c = relay_config("/t/198.19.0.1.sock", "10.0.2.15", Some("unix:/t/q.sock"));
        assert_eq!(c["listen"], "0.0.0.0:41919");
        assert_eq!(c["qmp"], "unix:/t/q.sock");
    }
}
