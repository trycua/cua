//! Filling, refreshing, verifying and evicting mirrors.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::git::{self, GitError};
use crate::store::{self, Entry, Pack, now_ms};
use crate::url::{self, RepoUrl};
use crate::{EVICTION_GRACE, RepoCache, shim, state};

const LS_REMOTE_LIMIT: Duration = Duration::from_secs(25);
const FILL_LIMIT: Duration = Duration::from_secs(20 * 60);
const VERIFY_LIMIT: Duration = Duration::from_secs(10 * 60);
/// A repository found private is not asked again for this long; one that
/// could not be reached, for less.
const PRIVATE_FOR: Duration = Duration::from_secs(30 * 60);
const UNREACHABLE_FOR: Duration = Duration::from_secs(60);
/// A mirror confirmed this recently is not asked about again.
const CONFIRMED_FOR_SECS: u64 = 5;
/// Packs a mirror may hold before it is repacked into one.
const MAX_PACKS: usize = 8;
/// Free disk the cache leaves alone.
const KEEP_FREE_BYTES: u64 = 1 << 30;

/// What [`RepoCache::ensure`] did.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Outcome {
    /// A new mirror: its size, and what the network clone took.
    Filled { bytes: u64, secs: f64 },
    /// An existing mirror, checked against the remote's refs: whether the
    /// remote had changed.
    Refreshed { changed: bool },
    /// Nothing cached, and why.
    Skipped(Reason),
}

/// Why a repository is not cached or a mirror was not used: a fixed list
/// (these are the words telemetry sends).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    Disabled,
    NoGit,
    UnsupportedUrl,
    HostNotPublic,
    /// Needs a credential, or is not found without one.
    Private,
    Unreachable,
    /// Larger than the cache may hold.
    TooBig,
    /// No room, with everything in it recently used.
    CacheFull,
    DiskFull,
    VerifyFailed,
    FillFailed,
}

impl Reason {
    /// The fixed word.
    pub fn word(self) -> &'static str {
        match self {
            Reason::Disabled => "disabled",
            Reason::NoGit => "no_git",
            Reason::UnsupportedUrl => "unsupported_url",
            Reason::HostNotPublic => "host_not_public",
            Reason::Private => "private",
            Reason::Unreachable => "unreachable",
            Reason::TooBig => "too_big",
            Reason::CacheFull => "cache_full",
            Reason::DiskFull => "disk_full",
            Reason::VerifyFailed => "verify_failed",
            Reason::FillFailed => "fill_failed",
        }
    }
}

/// The refs of a remote, as `ls-remote` lists them.
#[derive(Debug, Default, PartialEq, Eq)]
struct Refs {
    /// `refs/heads/x` and `refs/tags/x` with their object ids (sorted).
    refs: Vec<(String, String)>,
    /// What `HEAD` points at (`refs/heads/main`), when the remote says.
    head: Option<String>,
}

fn parse_ls_remote(out: &str) -> Refs {
    let mut r = Refs::default();
    for l in out.lines() {
        if let Some(rest) = l.strip_prefix("ref: ") {
            if let Some((target, name)) = rest.split_once('\t')
                && name == "HEAD"
            {
                r.head = Some(target.to_string());
            }
            continue;
        }
        let Some((oid, name)) = l.split_once('\t') else {
            continue;
        };
        if name.ends_with("^{}") {
            continue;
        }
        if (name.starts_with("refs/heads/") || name.starts_with("refs/tags/"))
            && oid.len() >= 40
            && oid.bytes().all(|b| b.is_ascii_hexdigit())
        {
            r.refs.push((name.to_string(), oid.to_string()));
        }
    }
    r.refs.sort();
    r.refs.dedup();
    r
}

fn skip(c: &RepoCache, r: Reason) -> Outcome {
    tracing::debug!(reason = r.word(), root = %c.root().display(), "repo cache: skipped");
    Outcome::Skipped(r)
}

pub(crate) async fn ensure(c: &RepoCache, repo: &RepoUrl) -> Outcome {
    if !c.cfg.enabled {
        return skip(c, Reason::Disabled);
    }
    let Some(bin) = git::binary() else {
        return skip(c, Reason::NoGit);
    };
    let cap = c.cap_bytes();
    if cap == 0 {
        return skip(c, Reason::CacheFull);
    }
    let neg_key = (c.root.clone(), repo.key.clone());
    if let Ok(n) = state().negative.lock()
        && n.get(&neg_key).is_some_and(|until| *until > Instant::now())
    {
        return skip(c, Reason::Private);
    }
    if c.prepare().is_err() {
        return skip(c, Reason::DiskFull);
    }
    let _gate = state().gate.lock().await;

    // Confirmed a moment ago (another clone of the same repository).
    if store::entry(&c.root, &repo.key).is_some()
        && std::fs::read_to_string(store::side(&c.root, &repo.key, "fresh"))
            .ok()
            .and_then(|t| t.trim().parse::<u64>().ok())
            .is_some_and(|s| (now_ms() / 1000).saturating_sub(s) < CONFIRMED_FOR_SECS)
    {
        return Outcome::Refreshed { changed: false };
    }

    if c.policy.check_host && !url::host_is_public(&repo.host).await {
        return skip(c, Reason::HostNotPublic);
    }
    let home = store::home_dir(&c.root);
    let remote = match ls_remote(&bin, &home, c.policy.allow, repo).await {
        Ok(r) => r,
        Err(reason) => {
            let for_ = if reason == Reason::Private {
                PRIVATE_FOR
            } else {
                UNREACHABLE_FOR
            };
            if let Ok(mut n) = state().negative.lock() {
                n.insert(neg_key, Instant::now() + for_);
            }
            // A mirror of a repository that turned private is not served.
            if reason == Reason::Private && store::entry(&c.root, &repo.key).is_some() {
                store::drop_entry(&c.root, &repo.key);
                let _ = shim::write_config(&c.root);
            }
            return skip(c, reason);
        }
    };
    if remote.refs.is_empty() {
        // An empty repository: nothing to mirror.
        return skip(c, Reason::FillFailed);
    }

    match store::entry(&c.root, &repo.key) {
        Some(e) => refresh(c, &bin, repo, e, &remote).await,
        None => fill(c, &bin, repo, &remote, cap).await,
    }
}

async fn ls_remote(bin: &Path, home: &Path, allow: &str, repo: &RepoUrl) -> Result<Refs, Reason> {
    let mut cmd = git::command(bin, home, allow);
    cmd.args([
        "ls-remote",
        "--symref",
        "--",
        &repo.url,
        "HEAD",
        "refs/heads/*",
        "refs/tags/*",
    ]);
    match git::run(cmd, LS_REMOTE_LIMIT).await {
        Ok(r) if r.ok => Ok(parse_ls_remote(&r.stdout)),
        Ok(r) => {
            let e = r.stderr.to_ascii_lowercase();
            let needs_credentials = [
                "authentication failed",
                "could not read username",
                "terminal prompts disabled",
                "repository not found",
                "returned error: 40",
                "invalid credentials",
            ]
            .iter()
            .any(|w| e.contains(w));
            Err(if needs_credentials {
                Reason::Private
            } else {
                Reason::Unreachable
            })
        }
        Err(GitError::Missing) => Err(Reason::NoGit),
        Err(GitError::Spawn(why)) => {
            tracing::debug!(%why, "repo cache: git could not be started");
            Err(Reason::Unreachable)
        }
        Err(GitError::TimedOut) => Err(Reason::Unreachable),
    }
}

async fn local_refs(bin: &Path, home: &Path, allow: &str, dir: &Path) -> Option<Refs> {
    let mut cmd = git::command(bin, home, allow);
    cmd.arg("-C").arg(dir).args([
        "for-each-ref",
        "--format=%(objectname)\t%(refname)",
        "refs/heads",
        "refs/tags",
    ]);
    let r = git::run(cmd, Duration::from_secs(60)).await.ok()?;
    if !r.ok {
        return None;
    }
    let mut refs: Vec<(String, String)> = r
        .stdout
        .lines()
        .filter_map(|l| {
            let (oid, name) = l.split_once('\t')?;
            Some((name.to_string(), oid.to_string()))
        })
        .collect();
    refs.sort();
    // A tag object's own id is what `for-each-ref` lists and what `ls-remote`
    // lists (the peeled `^{}` lines are left out of both).
    Some(Refs { refs, head: None })
}

fn random_name() -> String {
    format!("{:016x}", rand_u64())
}

fn rand_u64() -> u64 {
    // No `rand` dependency for one unpredictable-enough directory name.
    use std::hash::{BuildHasher as _, Hasher as _};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(u128::from(now_ms()) << 32 | u128::from(std::process::id()));
    h.finish()
}

async fn fill(c: &RepoCache, bin: &Path, repo: &RepoUrl, remote: &Refs, cap: u64) -> Outcome {
    let home = store::home_dir(&c.root);
    if store::free_bytes(&c.root).is_some_and(|f| f < KEEP_FREE_BYTES) {
        return skip(c, Reason::DiskFull);
    }
    let tmp_root = store::tmp_dir(&c.root);
    if std::fs::create_dir_all(&tmp_root).is_err() {
        return skip(c, Reason::DiskFull);
    }
    let tmp = tmp_root.join(format!("{}.git", random_name()));
    let started = Instant::now();
    let mut cmd = git::command(bin, &home, c.policy.allow);
    // Packs only: a Space copies packs, never loose objects.
    cmd.args(["-c", "transfer.unpackLimit=1", "-c", "fetch.unpackLimit=1"])
        .args(["clone", "--bare", "--quiet", "--"])
        .arg(&repo.url)
        .arg(&tmp);
    // The cache's cap is the most one repository may take.
    let budget = cap.saturating_sub(held_by_others(c, &repo.key));
    let ran = run_bounded(cmd, &tmp, budget, FILL_LIMIT).await;
    let secs = started.elapsed().as_secs_f64();
    match ran {
        Ok(true) => {}
        Ok(false) => {
            let _ = std::fs::remove_dir_all(&tmp);
            return skip(c, Reason::FillFailed);
        }
        Err(r) => {
            let _ = std::fs::remove_dir_all(&tmp);
            return skip(c, r);
        }
    }
    // Later refreshes fetch heads and tags only.
    for (k, v) in [
        ("remote.origin.fetch", "+refs/heads/*:refs/heads/*"),
        ("transfer.unpackLimit", "1"),
        ("fetch.unpackLimit", "1"),
        ("gc.auto", "0"),
    ] {
        let mut cmd = git::command(bin, &home, c.policy.allow);
        cmd.arg("-C").arg(&tmp).args(["config", k, v]);
        let _ = git::run(cmd, Duration::from_secs(30)).await;
    }
    let verified = verify(c, bin, &tmp, true).await;
    let (head, packs) = match verified {
        Ok(v) => v,
        Err(r) => {
            let _ = std::fs::remove_dir_all(&tmp);
            return skip(c, r);
        }
    };
    let bytes = store::dir_size(&tmp);
    if !make_room(c, bytes, &repo.key, cap) {
        let _ = std::fs::remove_dir_all(&tmp);
        return skip(c, Reason::CacheFull);
    }
    let dest = store::mirror_dir(&c.root, &repo.key);
    // A damaged leftover of the same key is replaced.
    if dest.exists() {
        store::drop_entry(&c.root, &repo.key);
    }
    if std::fs::rename(&tmp, &dest).is_err() {
        let _ = std::fs::remove_dir_all(&tmp);
        return skip(c, Reason::DiskFull);
    }
    let _ = remote;
    let now = now_ms();
    let entry = Entry {
        url: repo.url.clone(),
        key: repo.key.clone(),
        filled_ms: now,
        refreshed_ms: now,
        used_ms: now,
        bytes,
        fill_secs: secs,
        head,
        packs,
    };
    if store::publish(&c.root, &entry, true).is_err() {
        store::drop_entry(&c.root, &repo.key);
        return skip(c, Reason::DiskFull);
    }
    let _ = shim::write_config(&c.root);
    Outcome::Filled { bytes, secs }
}

fn held_by_others(c: &RepoCache, key: &str) -> u64 {
    store::entries(&c.root)
        .iter()
        .filter(|e| e.key != key)
        .map(|e| e.bytes)
        .sum()
}

/// Runs `cmd` (which writes `dir`) until it ends, stopping it when `dir`
/// grows past `budget` or `limit` passes. `Ok(true)`: it succeeded.
async fn run_bounded(
    mut cmd: tokio::process::Command,
    dir: &Path,
    budget: u64,
    limit: Duration,
) -> Result<bool, Reason> {
    use std::process::Stdio;
    cmd.stdout(Stdio::null()).stderr(Stdio::null());
    let mut child = cmd.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            Reason::NoGit
        } else {
            Reason::FillFailed
        }
    })?;
    let deadline = Instant::now() + limit;
    loop {
        tokio::select! {
            status = child.wait() => {
                return Ok(status.is_ok_and(|s| s.success()));
            }
            () = tokio::time::sleep(Duration::from_millis(400)) => {
                if store::dir_size(dir) > budget {
                    let _ = child.kill().await;
                    return Err(Reason::TooBig);
                }
                if Instant::now() > deadline {
                    let _ = child.kill().await;
                    return Err(Reason::FillFailed);
                }
            }
        }
    }
}

/// Frees what `needed` bytes more (for `key`) take beyond `cap` by dropping
/// the least recently used mirrors that were not used within the grace
/// period. `false` when that is not enough.
fn make_room(c: &RepoCache, needed: u64, key: &str, cap: u64) -> bool {
    let others = held_by_others(c, key);
    if needed > cap {
        return false;
    }
    if others + needed <= cap {
        return true;
    }
    let mut held = others;
    let now = now_ms();
    for e in store::entries(&c.root) {
        if held + needed <= cap {
            break;
        }
        if e.key == key || now.saturating_sub(e.used_ms) < EVICTION_GRACE.as_millis() as u64 {
            continue;
        }
        store::drop_entry(&c.root, &e.key);
        held = held.saturating_sub(e.bytes);
    }
    let _ = shim::write_config(&c.root);
    held + needed <= cap
}

/// Drops least recently used mirrors until the cache fits its cap.
pub(crate) fn enforce_cap(c: &RepoCache, keep: Option<&str>) {
    let cap = c.cap_bytes();
    let total: u64 = store::entries(&c.root).iter().map(|e| e.bytes).sum();
    if total > cap {
        let _ = make_room(c, 0, keep.unwrap_or(""), cap);
    }
}

async fn refresh(
    c: &RepoCache,
    bin: &Path,
    repo: &RepoUrl,
    mut entry: Entry,
    remote: &Refs,
) -> Outcome {
    let home = store::home_dir(&c.root);
    let dir = store::mirror_dir(&c.root, &repo.key);
    let local = local_refs(bin, &home, c.policy.allow, &dir).await;
    let head_now = current_head(bin, &home, c.policy.allow, &dir).await;
    let same = local.as_ref().is_some_and(|l| l.refs == remote.refs)
        && remote
            .head
            .as_ref()
            .is_none_or(|h| head_now.as_deref() == Some(h.as_str()));
    if same && local.is_some() {
        let _ = store::stamp_fresh(&c.root, &repo.key);
        entry.refreshed_ms = now_ms();
        entry.used_ms = now_ms();
        let _ = store::publish(&c.root, &entry, false);
        return Outcome::Refreshed { changed: false };
    }

    // Fetch what changed. Packs only, so a Space can copy them.
    let mut cmd = git::command(bin, &home, c.policy.allow);
    cmd.arg("-C").arg(&dir).args([
        "-c",
        "transfer.unpackLimit=1",
        "-c",
        "fetch.unpackLimit=1",
        "fetch",
        "--quiet",
        "--prune",
        "--no-write-fetch-head",
        "--force",
        "--no-auto-gc",
        "--",
        &repo.url,
        "+refs/heads/*:refs/heads/*",
        "+refs/tags/*:refs/tags/*",
    ]);
    let cap = c.cap_bytes();
    let budget = cap.saturating_sub(held_by_others(c, &repo.key));
    match run_bounded(cmd, &dir, budget, FILL_LIMIT).await {
        Ok(true) => {}
        Ok(false) => return skip(c, Reason::Unreachable),
        Err(r) => {
            // The mirror may hold a partial fetch past the cap: what is
            // not verified is not kept.
            if r == Reason::TooBig {
                store::drop_entry(&c.root, &repo.key);
                let _ = shim::write_config(&c.root);
            }
            return skip(c, r);
        }
    }
    if let Some(h) = &remote.head {
        let mut cmd = git::command(bin, &home, c.policy.allow);
        cmd.arg("-C").arg(&dir).args(["symbolic-ref", "HEAD", h]);
        let _ = git::run(cmd, Duration::from_secs(30)).await;
    }
    // Idle mirrors are tidied; one a Space may be reading is left as it is
    // (a new pack beside the old ones is safe, removing the old is not).
    let idle = now_ms().saturating_sub(entry.used_ms) > EVICTION_GRACE.as_millis() as u64;
    match verify(c, bin, &dir, idle).await {
        Ok((head, packs)) => {
            entry.head = head;
            entry.packs = packs;
            entry.bytes = store::dir_size(&dir);
            entry.refreshed_ms = now_ms();
            entry.used_ms = now_ms();
            if store::publish(&c.root, &entry, true).is_err() {
                store::drop_entry(&c.root, &repo.key);
                let _ = shim::write_config(&c.root);
                return skip(c, Reason::DiskFull);
            }
            if entry.bytes > cap {
                enforce_cap(c, Some(&repo.key));
            }
            let _ = shim::write_config(&c.root);
            Outcome::Refreshed { changed: true }
        }
        Err(r) => {
            store::drop_entry(&c.root, &repo.key);
            let _ = shim::write_config(&c.root);
            skip(c, r)
        }
    }
}

async fn current_head(bin: &Path, home: &Path, allow: &str, dir: &Path) -> Option<String> {
    let mut cmd = git::command(bin, home, allow);
    cmd.arg("-C").arg(dir).args(["symbolic-ref", "HEAD"]);
    let r = git::run(cmd, Duration::from_secs(30)).await.ok()?;
    r.ok.then(|| r.stdout.trim().to_string())
}

/// Checks a mirror before a Space may use it: no loose objects (a Space
/// copies packs; they are packed first when `may_repack`), every pack with
/// its index and a matching trailer, `git fsck --connectivity-only`, and a
/// `HEAD` that resolves. The packs, with their sizes, are what the Space's
/// helper checks on each use.
pub(crate) async fn verify(
    c: &RepoCache,
    bin: &Path,
    dir: &Path,
    may_repack: bool,
) -> Result<(String, Vec<Pack>), Reason> {
    let home = store::home_dir(&c.root);
    let git_in = |args: &[&str]| {
        let mut cmd = git::command(bin, &home, c.policy.allow);
        cmd.arg("-C").arg(dir).args(args);
        cmd
    };
    let loose = || -> bool {
        std::fs::read_dir(dir.join("objects"))
            .map(|rd| {
                rd.flatten().any(|e| {
                    let n = e.file_name().to_string_lossy().into_owned();
                    n.len() == 2 && n.bytes().all(|b| b.is_ascii_hexdigit())
                })
            })
            .unwrap_or(true)
    };
    let pack_count = || packs_of(dir).len();
    if loose() || (may_repack && pack_count() > MAX_PACKS) {
        if !may_repack && loose() {
            return Err(Reason::VerifyFailed);
        }
        let r = git::run(git_in(&["repack", "-a", "-d", "-q"]), VERIFY_LIMIT).await;
        if !r.is_ok_and(|r| r.ok) || loose() {
            // `repack -a -d` leaves loose objects that are unreachable.
            let _ = git::run(git_in(&["prune-packed"]), VERIFY_LIMIT).await;
            if loose() {
                return Err(Reason::VerifyFailed);
            }
        }
    }
    let fsck = git::run(
        git_in(&[
            "fsck",
            "--connectivity-only",
            "--no-dangling",
            "--no-progress",
        ]),
        VERIFY_LIMIT,
    )
    .await;
    if !fsck.is_ok_and(|r| r.ok) {
        return Err(Reason::VerifyFailed);
    }
    let head = git::run(
        git_in(&["rev-parse", "--verify", "HEAD"]),
        Duration::from_secs(30),
    )
    .await
    .map_err(|_| Reason::VerifyFailed)?;
    if !head.ok {
        return Err(Reason::VerifyFailed);
    }
    let packs = packs_of(dir);
    if packs.is_empty() || !packs.iter().all(|p| pack_trailer_matches(dir, &p.name)) {
        return Err(Reason::VerifyFailed);
    }
    Ok((head.stdout.trim().to_string(), packs))
}

fn packs_of(dir: &Path) -> Vec<Pack> {
    let Ok(rd) = std::fs::read_dir(dir.join("objects/pack")) else {
        return Vec::new();
    };
    let mut v: Vec<Pack> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.ends_with(".pack").then(|| Pack {
                size: e.metadata().map_or(0, |m| m.len()),
                name,
            })
        })
        .collect();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

/// A pack's last 20 bytes (its checksum) are the first 20 of the last 40 of
/// its index; a truncated or swapped file does not match.
fn pack_trailer_matches(dir: &Path, pack: &str) -> bool {
    use std::io::{Read as _, Seek as _, SeekFrom};
    let base = pack.trim_end_matches(".pack");
    let read_tail = |p: PathBuf, n: i64| -> Option<Vec<u8>> {
        let mut f = std::fs::File::open(p).ok()?;
        f.seek(SeekFrom::End(-n)).ok()?;
        let mut b = vec![0u8; n as usize];
        f.read_exact(&mut b).ok()?;
        Some(b)
    };
    let d = dir.join("objects/pack");
    let (Some(pk), Some(ix)) = (
        read_tail(d.join(format!("{base}.pack")), 20),
        read_tail(d.join(format!("{base}.idx")), 40),
    ) else {
        return false;
    };
    pk == ix[..20]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ls_remote_output_parses_to_heads_tags_and_head() {
        let oid = |c: char| c.to_string().repeat(40);
        let out = format!(
            "ref: refs/heads/main\tHEAD\n{a}\tHEAD\n{a}\trefs/heads/main\n{b}\trefs/tags/v1\n{c}\trefs/tags/v1^{{}}\n",
            a = oid('a'),
            b = oid('b'),
            c = oid('c'),
        );
        let r = parse_ls_remote(&out);
        assert_eq!(r.head.as_deref(), Some("refs/heads/main"));
        assert_eq!(
            r.refs,
            vec![
                ("refs/heads/main".to_string(), oid('a')),
                ("refs/tags/v1".to_string(), oid('b')),
            ]
        );
    }
}
