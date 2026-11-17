//! Execute the generated installer against isolated caches, with only the
//! network-facing git/uv commands replaced. No host tools or homes are changed.

use super::*;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Cache {
    home: tempfile::TempDir,
    image: PathBuf,
    bin: PathBuf,
}

impl Cache {
    fn new() -> Self {
        let home = tempfile::Builder::new()
            .prefix("cua cache '")
            .tempdir()
            .unwrap();
        let image = home.path().join("image");
        let bin = home.path().join("stubs");
        std::fs::create_dir_all(&bin).unwrap();
        executable(
            &bin.join("git"),
            r#"#!/bin/sh
set -eu
case "$1" in
  init) mkdir -p "$3" ;;
  fetch) printf '%s\n' "$5" > .repo; printf '%s\n' "$6" > .commit ;;
  checkout) test "$3" = FETCH_HEAD ;;
  rev-parse) cat .commit ;;
  *) exit 91 ;;
esac
"#,
        );
        executable(
            &bin.join("uv"),
            r#"#!/bin/sh
set -eu
test "$1" = sync
[ ! -f "$HOME/fail-sync" ] || exit 42
mkdir -p .venv/bin
{
  printf '%s\n' '#!/bin/sh' "cat <<'REQUIREMENTS'"
  cat .repo .commit
  printf '%s\n' "$*" REQUIREMENTS
} > .venv/bin/hermes
chmod +x .venv/bin/hermes
cp .venv/bin/hermes .venv/bin/hermes-acp
printf 'sync\n' >> "$HOME/syncs"
"#,
        );
        // A missing stub must never turn into a network call or privilege request.
        for name in ["curl", "wget", "sudo", "apt-get"] {
            executable(&bin.join(name), "#!/bin/sh\nexit 92\n");
        }
        Self { home, image, bin }
    }

    fn dir(&self, it: &Installable, baked: bool) -> PathBuf {
        let base = if baked {
            self.image.clone()
        } else {
            self.home.path().join(TOOLS_DIR)
        };
        base.join("hermes").join(&it.version)
    }

    fn legacy(&self, it: &Installable, baked: bool) {
        let dir = self.dir(it, baked);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".cua-installed"), &it.version).unwrap();
        executable(&dir.join(".venv/bin/hermes"), "#!/bin/sh\necho stale\n");
    }

    fn shell(&self, script: &str) -> Output {
        Command::new("/bin/sh")
            .arg("-c")
            .arg(script.replace(IMAGE_TOOLS_DIR, self.image.to_str().unwrap()))
            .env_clear()
            .env("HOME", self.home.path())
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()))
            .output()
            .unwrap()
    }

    fn install(&self, it: &Installable) -> Output {
        self.shell(&format!("{PRELUDE}{}", step("hermes", it)))
    }

    fn installed(&self, it: &Installable, phase: &str) {
        let out = self.install(it);
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success(),
            "{stdout}{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            stdout
                .lines()
                .filter_map(Progress::parse)
                .any(|p| p.phase == phase),
            "{stdout}"
        );
    }

    fn output(&self) -> String {
        let link = std::fs::read_link(self.home.path().join(BIN_DIR).join("hermes")).unwrap();
        let out = Command::new(link).output().unwrap();
        assert!(out.status.success());
        String::from_utf8(out.stdout).unwrap()
    }

    fn syncs(&self) -> usize {
        std::fs::read_to_string(self.home.path().join("syncs"))
            .unwrap_or_default()
            .lines()
            .count()
    }
}

fn executable(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn item() -> Installable {
    let mut it = get("hermes").unwrap().clone();
    it.how = Source::GitUv {
        repo: "https://example.invalid/agent".into(),
        commit: "1111111111111111111111111111111111111111".into(),
        extras: vec!["acp".into()],
        python: "3.12".into(),
    };
    it.bins = BTreeMap::from([("hermes".into(), ".venv/bin/hermes".into())]);
    it
}

#[test]
fn legacy_git_uv_caches_are_repaired_once_without_changing_the_image() {
    for baked in [false, true] {
        let cache = Cache::new();
        let it = item();
        cache.legacy(&it, baked);
        cache.installed(&it, "done");
        assert!(
            cache
                .output()
                .contains("sync --quiet --frozen --no-dev --extra acp --python 3.12")
        );
        assert_eq!(cache.syncs(), 1);
        cache.installed(&it, "cached");
        assert_eq!(cache.syncs(), 1);
        if baked {
            assert_eq!(
                std::fs::read_to_string(cache.dir(&it, true).join(".cua-installed")).unwrap(),
                it.version
            );
            assert!(
                std::fs::read_to_string(cache.dir(&it, true).join(".venv/bin/hermes"))
                    .unwrap()
                    .contains("stale")
            );
        }
    }
}

#[test]
fn same_version_git_uv_requirement_changes_reinstall() {
    let original = item();
    for (field, value) in [
        ("repo", "https://example.invalid/other"),
        ("commit", "2222222222222222222222222222222222222222"),
        ("python", "3.13"),
        ("extra", "mcp"),
    ] {
        let cache = Cache::new();
        cache.installed(&original, "done");
        let mut changed = original.clone();
        let Source::GitUv {
            repo,
            commit,
            python,
            extras,
        } = &mut changed.how
        else {
            unreachable!()
        };
        match field {
            "repo" => *repo = value.into(),
            "commit" => *commit = value.into(),
            "python" => *python = value.into(),
            _ => extras.push(value.into()),
        }
        cache.installed(&changed, "done");
        assert!(
            cache.output().contains(value),
            "{field}: {}",
            cache.output()
        );
        assert_eq!(cache.syncs(), 2, "{field}");
        cache.installed(&changed, "cached");
        assert_eq!(cache.syncs(), 2, "{field}");
    }
}

#[test]
fn matching_baked_git_uv_cache_is_reused_and_stale_one_is_not_modified() {
    let cache = Cache::new();
    let mut it = item();
    cache.installed(&it, "done");
    let baked = cache.dir(&it, true);
    std::fs::create_dir_all(baked.parent().unwrap()).unwrap();
    std::fs::rename(cache.dir(&it, false), &baked).unwrap();
    cache.installed(&it, "cached");
    assert_eq!(
        std::fs::read_link(cache.home.path().join(BIN_DIR).join("hermes")).unwrap(),
        baked.join(".venv/bin/hermes")
    );
    assert_eq!(cache.syncs(), 1);
    let before = std::fs::read(baked.join(".venv/bin/hermes")).unwrap();
    if let Source::GitUv { extras, .. } = &mut it.how {
        extras.push("mcp".into());
    }
    cache.installed(&it, "done");
    assert!(cache.output().contains("--extra mcp"));
    assert_eq!(cache.syncs(), 2);
    assert_eq!(
        std::fs::read(baked.join(".venv/bin/hermes")).unwrap(),
        before
    );
}

#[test]
fn failed_git_uv_cache_repair_is_not_marked_complete() {
    let cache = Cache::new();
    let it = item();
    cache.legacy(&it, false);
    std::fs::write(cache.home.path().join("fail-sync"), "").unwrap();
    let out = cache.install(&it);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(Progress::parse)
            .any(|p| p.phase == "error" && p.detail == "uv sync failed")
    );
    assert!(!cache.dir(&it, false).exists());
    std::fs::remove_file(cache.home.path().join("fail-sync")).unwrap();
    cache.installed(&it, "done");
    cache.installed(&it, "cached");
    assert_eq!(cache.syncs(), 1);
}

#[test]
fn launch_shortcut_requires_the_current_git_uv_installation() {
    let cache = Cache::new();
    let h = crate::harness::harness("hermes").unwrap();
    let check = crate::runs::tools_in_place_script(cache.home.path().to_str().unwrap(), h).unwrap();
    let it = get("hermes").unwrap();
    for id in resolve(&["acp-sdk", "mcp-remote", "hermes"]).unwrap() {
        if id == "hermes" {
            continue;
        }
        let dir = cache
            .home
            .path()
            .join(TOOLS_DIR)
            .join(&id)
            .join(&get(&id).unwrap().version);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".cua-installed"), "installed").unwrap();
    }
    assert!(!cache.shell(&check).status.success(), "missing Hermes");
    cache.legacy(it, false);
    assert!(
        !cache.shell(&check).status.success(),
        "legacy cache must not bypass repair"
    );
    cache.installed(it, "done");
    assert_eq!(cache.shell(&check).stdout, b"in-place\n");
    let mut different = it.clone();
    if let Source::GitUv { python, .. } = &mut different.how {
        *python = "3.13".into();
    }
    cache.installed(&different, "done");
    assert!(
        !cache.shell(&check).status.success(),
        "different requirements at the same version"
    );
    cache.installed(it, "done");
    assert_eq!(cache.shell(&check).stdout, b"in-place\n");
    // An older client can relink a baked install without removing the
    // current writable receipt. The shortcut must check what would run.
    cache.legacy(it, true);
    for (bin, rel) in &it.bins {
        let link = cache.home.path().join(BIN_DIR).join(bin);
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(cache.dir(it, true).join(rel), &link).unwrap();
        assert!(
            !cache.shell(&check).status.success(),
            "stale baked link: {bin}"
        );
        let syncs = cache.syncs();
        cache.installed(it, "cached");
        assert_eq!(cache.syncs(), syncs, "relinking does not reinstall");
        assert_eq!(cache.shell(&check).stdout, b"in-place\n");
    }
}

#[test]
fn archive_and_npm_caches_keep_reusing_existing_installs() {
    for id in ["goose", "pi"] {
        for baked in [false, true] {
            let cache = Cache::new();
            let it = get(id).unwrap();
            let base = if baked {
                cache.image.clone()
            } else {
                cache.home.path().join(TOOLS_DIR)
            };
            let dir = base.join(id).join(&it.version);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(".cua-installed"), &it.version).unwrap();
            for rel in it.bins.values() {
                executable(&dir.join(rel), "#!/bin/sh\necho cached\n");
            }
            let out = cache.shell(&format!("{PRELUDE}{}", step(id, it)));
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            let phases: Vec<_> = String::from_utf8_lossy(&out.stdout)
                .lines()
                .filter_map(Progress::parse)
                .map(|p| p.phase)
                .collect();
            assert_eq!(phases, ["cached"]);
            for (bin, rel) in &it.bins {
                assert_eq!(
                    std::fs::read_link(cache.home.path().join(BIN_DIR).join(bin)).unwrap(),
                    dir.join(rel)
                );
            }
        }
    }
}
