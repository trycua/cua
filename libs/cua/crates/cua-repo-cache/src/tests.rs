//! The cache against real git: local repositories stand in for remotes, and
//! the helper runs as a Space runs it.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{Config, EVICTION_GRACE, Outcome, Reason, RepoCache, RepoUrl, shim, store};

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A repository with `n` commits, each adding a file of `kb` KiB of
/// pseudo-random bytes.
fn source(dir: &Path, name: &str, n: usize, kb: usize) -> RepoUrl {
    let path = dir.join(name);
    std::fs::create_dir_all(&path).unwrap();
    git(&path, &["init", "-q", "-b", "main"]);
    for i in 0..n {
        add_commit(&path, i, kb);
    }
    repo_url(&path)
}

fn add_commit(path: &Path, i: usize, kb: usize) {
    let mut x: u64 = 0x9e37_79b9_7f4a_7c15 ^ (i as u64 + 1).wrapping_mul(0xff51_afd7_ed55_8ccd);
    let bytes: Vec<u8> = (0..kb * 1024)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect();
    std::fs::write(path.join(format!("f{i}.bin")), bytes).unwrap();
    git(path, &["add", "."]);
    git(path, &["commit", "-q", "-m", &format!("c{i}")]);
}

fn repo_url(path: &Path) -> RepoUrl {
    let url = format!("file://{}", path.display());
    let key = hex::encode(&<sha2::Sha256 as sha2::Digest>::digest(url.as_bytes())[..8]);
    RepoUrl {
        url,
        key,
        host: "example.com".into(),
    }
}

/// Git writes packs read-only.
fn make_writable(p: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o644)).unwrap();
}

fn cache(root: &Path, cap: Option<u64>) -> RepoCache {
    RepoCache::new(
        root,
        Config {
            enabled: true,
            cap_bytes: cap,
        },
    )
    .allowing_local_repos()
}

fn dirs() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("src");
    let root = t.path().join("cache");
    std::fs::create_dir_all(&src).unwrap();
    (t, src, root)
}

#[tokio::test]
async fn the_first_sight_fills_a_verified_mirror_and_a_repeat_is_confirmed() {
    let (_t, src, root) = dirs();
    let repo = source(&src, "a", 3, 64);
    let c = cache(&root, Some(1 << 30));
    let out = c.ensure(&repo).await;
    let Outcome::Filled { bytes, .. } = out else {
        panic!("not filled: {out:?}");
    };
    assert!(bytes > 0);
    let e = c.entries();
    assert_eq!(e.len(), 1);
    assert_eq!(e[0].head, git(&src.join("a"), &["rev-parse", "HEAD"]));
    assert!(!e[0].packs.is_empty());
    // The Space-facing files are all there.
    for ext in ["ok", "url", "fresh", "json"] {
        assert!(store::side(&root, &repo.key, ext).is_file(), "{ext}");
    }
    assert!(
        std::fs::read_to_string(root.join("gitconfig"))
            .unwrap()
            .contains(&format!("cuarepo::{}::{}", repo.key, repo.url))
    );
    // A repeat within seconds is confirmed without asking the remote again.
    assert_eq!(c.ensure(&repo).await, Outcome::Refreshed { changed: false });
}

#[tokio::test]
async fn a_new_commit_on_the_remote_reaches_the_mirror_and_is_never_served_stale() {
    let (_t, src, root) = dirs();
    let repo = source(&src, "a", 2, 16);
    let c = cache(&root, Some(1 << 30));
    assert!(matches!(c.ensure(&repo).await, Outcome::Filled { .. }));
    let before = c.entries()[0].head.clone();
    add_commit(&src.join("a"), 99, 16);
    git(&src.join("a"), &["tag", "v1"]);
    // The lease stamp of the first fill is still fresh: age it, as time does.
    std::fs::write(store::side(&root, &repo.key, "fresh"), "1\n").unwrap();
    assert_eq!(c.ensure(&repo).await, Outcome::Refreshed { changed: true });
    let e = c.entries();
    assert_ne!(e[0].head, before);
    assert_eq!(e[0].head, git(&src.join("a"), &["rev-parse", "HEAD"]));
    // The mirror has the tag and the commit.
    let mirror = store::mirror_dir(&root, &repo.key);
    assert_eq!(
        git(&mirror, &["rev-parse", "v1"]),
        git(&src.join("a"), &["rev-parse", "v1"])
    );
    // No loose objects: a Space copies packs.
    assert!(
        std::fs::read_dir(mirror.join("objects"))
            .unwrap()
            .flatten()
            .all(|e| e.file_name().to_string_lossy().len() != 2),
        "loose objects in a mirror"
    );
}

#[tokio::test]
async fn a_mirror_that_stopped_verifying_is_dropped_and_not_served() {
    let (_t, src, root) = dirs();
    let repo = source(&src, "a", 2, 32);
    let c = cache(&root, Some(1 << 30));
    assert!(matches!(c.ensure(&repo).await, Outcome::Filled { .. }));
    // A pack loses its tail.
    let pack = &c.entries()[0].packs[0].name;
    let p = store::mirror_dir(&root, &repo.key)
        .join("objects/pack")
        .join(pack);
    let len = std::fs::metadata(&p).unwrap().len();
    make_writable(&p);
    std::fs::OpenOptions::new()
        .write(true)
        .open(&p)
        .unwrap()
        .set_len(len - 10)
        .unwrap();
    // The helper in a Space reports it skipped the mirror; the daemon checks
    // and drops it.
    c.ingest_guest_log(&format!("1700000000 skipped {} 0\n", repo.key));
    assert!(c.entries().is_empty());
    assert!(!store::mirror_dir(&root, &repo.key).exists());
    assert!(
        !std::fs::read_to_string(root.join("gitconfig"))
            .unwrap()
            .contains(&repo.key)
    );
    // A fill after that makes a new one.
    assert!(matches!(c.ensure(&repo).await, Outcome::Filled { .. }));
}

#[tokio::test]
async fn the_cache_stays_within_its_cap_under_churn_and_drops_the_least_recently_used() {
    let (_t, src, root) = dirs();
    let cap: u64 = 700 * 1024;
    let c = cache(&root, Some(cap));
    let mut repos = Vec::new();
    for i in 0..8 {
        repos.push(source(&src, &format!("r{i}"), 2, 100));
    }
    for r in &repos {
        let _ = c.ensure(r).await;
        // Pretend each was last used long ago, so eviction may take it.
        for e in c.entries() {
            let mut e = e;
            e.used_ms = e
                .used_ms
                .saturating_sub(EVICTION_GRACE.as_millis() as u64 * 2)
                .min(store::now_ms() - 1_000_000);
            store::publish(&root, &e, false).unwrap();
        }
        let on_disk = store::dir_size(&store::m_dir(&root));
        let held: u64 = c.entries().iter().map(|e| e.bytes).sum();
        assert!(held <= cap, "held {held} over cap {cap}");
        // Everything under m/ (mirrors plus their small side files).
        assert!(
            on_disk <= cap + 16 * 1024,
            "on disk {on_disk} over cap {cap}"
        );
    }
    assert!(!c.entries().is_empty());
    assert!(c.entries().len() < 8);
}

#[tokio::test]
async fn nothing_is_taken_in_while_the_cache_is_full_of_recently_used_mirrors() {
    let (_t, src, root) = dirs();
    let a = source(&src, "a", 2, 100);
    let b = source(&src, "b", 2, 100);
    let c = cache(&root, Some(300 * 1024));
    assert!(matches!(c.ensure(&a).await, Outcome::Filled { .. }));
    // `a` was just used: it is not evicted for `b`, and `b` is not taken.
    assert_eq!(c.ensure(&b).await, Outcome::Skipped(Reason::CacheFull));
    assert_eq!(c.entries().len(), 1);
    assert!(std::fs::read_dir(store::tmp_dir(&root)).map_or(true, |mut d| d.next().is_none()));
}

#[tokio::test]
async fn a_repository_bigger_than_the_cache_is_not_cached() {
    let (_t, src, root) = dirs();
    let a = source(&src, "a", 3, 200);
    let c = cache(&root, Some(100 * 1024));
    assert!(matches!(
        c.ensure(&a).await,
        Outcome::Skipped(Reason::TooBig | Reason::CacheFull)
    ));
    assert!(c.entries().is_empty());
    assert!(store::dir_size(&root.join("tmp")) == 0);
}

#[tokio::test]
async fn off_means_nothing_is_cached_and_clear_empties_the_cache() {
    let (_t, src, root) = dirs();
    let a = source(&src, "a", 2, 16);
    let off = RepoCache::new(
        &root,
        Config {
            enabled: false,
            cap_bytes: None,
        },
    )
    .allowing_local_repos();
    assert_eq!(off.ensure(&a).await, Outcome::Skipped(Reason::Disabled));
    assert!(off.mount_source().is_none());
    let c = cache(&root, Some(1 << 30));
    assert!(matches!(c.ensure(&a).await, Outcome::Filled { .. }));
    assert_eq!(c.status().repos, 1);
    c.clear();
    assert_eq!(c.status().repos, 0);
    assert!(c.entries().is_empty());
    assert_eq!(
        store::dir_size(&store::m_dir(&root)),
        std::fs::metadata(root.join("m/..")).map(|_| 0).unwrap()
    );
}

#[tokio::test]
async fn an_unreachable_remote_is_a_quiet_skip() {
    let (_t, _src, root) = dirs();
    let c = cache(&root, Some(1 << 30));
    let missing = repo_url(Path::new("/nonexistent/cua-repo-cache-test"));
    assert!(matches!(
        c.ensure(&missing).await,
        Outcome::Skipped(Reason::Unreachable | Reason::Private)
    ));
    assert!(c.entries().is_empty());
}

#[tokio::test]
async fn hits_reported_by_a_space_show_in_the_week() {
    let (_t, src, root) = dirs();
    let a = source(&src, "a", 2, 16);
    let c = cache(&root, Some(1 << 30));
    assert!(matches!(c.ensure(&a).await, Outcome::Filled { .. }));
    c.ingest_guest_log(&format!(
        "1700000000 hit_lease {k} 0\n1700000001 hit_checked {k} 1\n1700000002 failed {k} 0\n",
        k = a.key
    ));
    let s = c.status();
    assert_eq!((s.hits, s.misses, s.repos), (2, 1, 1));
    assert!(s.bytes_saved > 0 && s.bytes > 0);
}

/// A clone made the way a Space makes it: the cache's system git config, the
/// helper on PATH, the mirror under the cache root.
fn clone_like_a_space(root: &Path, url: &str, dest: &Path, extra: &[&str]) -> std::process::Output {
    Command::new("git")
        .args(["clone", "-q"])
        .args(extra)
        .arg(url)
        .arg(dest)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", root.join("gitconfig"))
        .env("CUA_REPO_CACHE_ROOT", root)
        .env("CUA_REPO_CACHE_LOG", root.join("guest.log"))
        .env(
            "PATH",
            format!(
                "{}:{}",
                root.join("bin").display(),
                std::env::var("PATH").unwrap()
            ),
        )
        .output()
        .unwrap()
}

#[tokio::test]
async fn a_clone_through_the_helper_is_complete_independent_and_matches_the_remote() {
    let (t, src, root) = dirs();
    let repo = source(&src, "a", 4, 64);
    let c = cache(&root, Some(1 << 30));
    assert!(matches!(c.ensure(&repo).await, Outcome::Filled { .. }));
    // Within the lease the helper serves from the mirror with no remote.
    let dest = t.path().join("clone");
    let out = clone_like_a_space(&root, &repo.url, &dest, &[]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        git(&dest, &["rev-parse", "HEAD"]),
        git(&src.join("a"), &["rev-parse", "HEAD"])
    );
    assert_eq!(git(&dest, &["fsck", "--connectivity-only"]), "");
    // Own objects: no alternate, and the packs are the clone's own files.
    assert!(!dest.join(".git/objects/info/alternates").exists());
    assert!(
        std::fs::read_dir(dest.join(".git/objects/pack"))
            .unwrap()
            .flatten()
            .count()
            >= 2
    );
    // The clone keeps the remote's own URL.
    assert_eq!(git(&dest, &["config", "remote.origin.url"]), repo.url);
    // The helper logged a lease hit that the daemon can read.
    let log = std::fs::read_to_string(root.join("guest.log")).unwrap();
    assert!(log.contains(&format!("hit_lease {}", repo.key)), "{log}");
    c.ingest_guest_log(&log);
    assert_eq!(c.status().hits, 1);
    // Removing the mirror afterwards leaves the clone whole.
    c.clear();
    assert_eq!(git(&dest, &["fsck", "--full"]), "");
    assert_eq!(git(&dest, &["log", "--oneline"]).lines().count(), 4);
}

#[tokio::test]
async fn a_clone_past_the_lease_without_a_remote_goes_the_plain_way_and_is_never_stale() {
    let (t, src, root) = dirs();
    let repo = source(&src, "a", 2, 16);
    let c = cache(&root, Some(1 << 30));
    assert!(matches!(c.ensure(&repo).await, Outcome::Filled { .. }));
    // New commit upstream, the mirror not yet refreshed, the lease over.
    add_commit(&src.join("a"), 50, 16);
    std::fs::write(store::side(&root, &repo.key, "fresh"), "1\n").unwrap();
    // The helper hands over to `git remote-https`, which cannot reach a
    // file remote: the clone fails the same way a plain clone of this URL
    // would, and nothing stale is served.
    let dest = t.path().join("clone");
    let out = clone_like_a_space(&root, &repo.url, &dest, &[]);
    let plain = Command::new("git")
        .args(["clone", "-q", &repo.url])
        .arg(t.path().join("plain"))
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .unwrap();
    assert!(plain.status.success());
    assert!(
        !out.status.success(),
        "a stale mirror must not be served past the lease"
    );
    assert!(!dest.join(".git/objects/info/alternates").exists());
}

#[tokio::test]
async fn a_mirror_that_fails_the_helper_checks_is_skipped_and_the_clone_is_plain() {
    let (t, src, root) = dirs();
    let repo = source(&src, "a", 2, 16);
    let c = cache(&root, Some(1 << 30));
    assert!(matches!(c.ensure(&repo).await, Outcome::Filled { .. }));
    // Tamper: the pack is no longer the size that was verified.
    let pack = &c.entries()[0].packs[0].name;
    let p = store::mirror_dir(&root, &repo.key)
        .join("objects/pack")
        .join(pack);
    let mut data = std::fs::read(&p).unwrap();
    make_writable(&p);
    data.push(0);
    std::fs::write(&p, data).unwrap();
    let dest = t.path().join("clone");
    // Plain path: `git remote-https` cannot take a file URL, so this clone
    // fails; what matters is that the helper did not use the mirror.
    let _ = clone_like_a_space(&root, &repo.url, &dest, &[]);
    let log = std::fs::read_to_string(root.join("guest.log")).unwrap_or_default();
    assert!(!log.contains("hit_"), "{log}");
    assert!(log.contains(&format!("skipped {}", repo.key)), "{log}");
}

#[tokio::test]
async fn a_missing_mount_means_no_config_and_a_plain_clone() {
    let (t, src, root) = dirs();
    let repo = source(&src, "a", 2, 16);
    let c = cache(&root, Some(1 << 30));
    assert!(matches!(c.ensure(&repo).await, Outcome::Filled { .. }));
    // The Space's system config includes a file that is gone.
    let sys = t.path().join("etc-gitconfig");
    std::fs::write(
        &sys,
        format!("[include]\n\tpath = {}/missing/gitconfig\n", root.display()),
    )
    .unwrap();
    let dest = t.path().join("clone");
    let out = Command::new("git")
        .args(["clone", "-q", &repo.url])
        .arg(&dest)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", &sys)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
}

#[test]
fn the_helper_text_is_written_executable_and_is_valid_shell() {
    let (_t, _src, root) = dirs();
    let c = cache(&root, None);
    c.prepare().unwrap();
    let p = root.join("bin").join(shim::HELPER_NAME);
    use std::os::unix::fs::PermissionsExt as _;
    assert_eq!(
        std::fs::metadata(&p).unwrap().permissions().mode() & 0o111,
        0o111
    );
    let out = Command::new("sh").arg("-n").arg(&p).output().unwrap();
    assert!(out.status.success(), "{out:?}");
}
