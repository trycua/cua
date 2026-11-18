//! A fake Apple `container` CLI for the hermetic Containerization tests: a
//! shell script keeping containers as JSON files in a state directory and
//! logging every call.

use std::path::{Path, PathBuf};

use cua_vmm::RootfsResolver;
use cua_vmm::containerization::{ContainerizationConfig, ContainerizationRuntime};

const SCRIPT: &str = r#"
log() { printf '%s\n' "$*" >> "$S/calls"; }
log "$* | APP_ROOT=$CONTAINER_APP_ROOT"
json() { # name state
  n="$1"; st="$2"
  ports=$(cat "$S/c/$n.ports" 2>/dev/null)
  labels=$(cat "$S/c/$n.labels" 2>/dev/null)
  printf '{"configuration":{"id":"%s","image":{"reference":"%s"},"labels":{%s},"publishedPorts":[%s]},"status":{"state":"%s","networks":[{"network":"default","hostname":"%s","ipv4Address":"192.168.64.9/24","ipv4Gateway":"192.168.64.1"}]}}' \
    "$n" "$(cat "$S/c/$n.image")" "$labels" "$ports" "$st" "$n"
}
case "$1 $2" in
  "system status")
    if [ -f "$S/services" ]; then
      printf '{"status":"running","paths":{"appRoot":"%s/","installRoot":"/fake/install/"}}\n' "$(cat "$S/services")"
    else
      echo '{"status":"not running"}'; exit 1
    fi
    exit 0 ;;
  "system start")
    shift 2
    while [ $# -gt 0 ]; do
      case "$1" in --app-root) app="$2"; shift ;; esac; shift
    done
    # launchd keeps another install's service: start "succeeds" anyway.
    if [ -f "$S/hijack" ]; then cp "$S/hijack" "$S/services"; else printf '%s' "$app" > "$S/services"; fi
    exit 0 ;;
  "system stop") log "SYSTEM STOP"; rm -f "$S/services"; exit 0 ;;
esac
cmd="$1"; shift
mkdir -p "$S/c"
case "$cmd" in
  run)
    ports=""; labels=""; name=""; envf=""; ep=""; dns=""
    while [ $# -gt 0 ]; do
      case "$1" in
        --detach|--progress|--name|--cpus|--memory|--shm-size|--cap-add|--label|--env-file|--publish|--entrypoint|--arch|--dns) ;;
        *) break ;;
      esac
      case "$1" in
        --detach) shift; continue ;;
        --name) name="$2" ;;
        --env-file) envf="$2" ;;
        --dns) dns="${dns:+$dns,}$2" ;;
        --entrypoint) ep="$2" ;;
        --label) k="${2%%=*}"; v="${2#*=}"; labels="${labels:+$labels,}\"$k\":\"$v\"" ;;
        --publish) spec="${2%/tcp}"; rest="${spec#*:}"; h="${rest%%:*}"; g="${rest#*:}"
          ports="${ports:+$ports,}{\"hostAddress\":\"127.0.0.1\",\"hostPort\":$h,\"containerPort\":$g,\"proto\":\"tcp\",\"count\":1}" ;;
      esac
      shift 2
    done
    image="$1"
    # `container run` pulls a missing image by itself, anonymously.
    k=$(printf '%s' "$image" | tr '/:@' '___')
    if [ -f "$S/private/$k" ] && [ ! -f "$S/img/$k" ]; then
      echo "Error: refusing insecure credential exchange: authorization server https://ghcr.io/token issued its own authentication challenge" >&2; exit 1
    fi
    if [ -f "$S/c/$name.image" ]; then echo "Error: container $name already exists" >&2; exit 1; fi
    printf '%s' "$dns" > "$S/c/$name.dns"
    printf '%s' "$image" > "$S/c/$name.image"
    printf '%s' "$ports" > "$S/c/$name.ports"
    printf '%s' "$labels" > "$S/c/$name.labels"
    if [ -n "$envf" ]; then
      cp "$envf" "$S/c/$name.env"
      (stat -c %a "$envf" 2>/dev/null || stat -f %Lp "$envf") > "$S/c/$name.envmode"
    fi
    [ -f "$S/boot_fails" ] && st=stopped || st=running
    printf '%s' "$st" > "$S/c/$name.state"
    echo "$name" ;;
  inspect)
    n="$1"
    [ -f "$S/c/$n.image" ] || { echo "Error: container not found: $n" >&2; exit 1; }
    printf '[%s]\n' "$(json "$n" "$(cat "$S/c/$n.state")")" ;;
  list)
    out=""
    for f in "$S"/c/*.image; do
      [ -f "$f" ] || continue
      n=$(basename "$f" .image)
      out="${out:+$out,}$(json "$n" "$(cat "$S/c/$n.state")")"
    done
    printf '[%s]\n' "$out" ;;
  stop) [ "$1" = "--time" ] && shift 2; printf stopped > "$S/c/$1.state" ;;
  start) [ -f "$S/c/$1.image" ] || exit 1; printf running > "$S/c/$1.state" ;;
  delete) [ "$1" = "--force" ] && shift; rm -f "$S/c/$1".* ;;
  image)
    sub="$1"; shift
    key() { printf '%s' "$1" | tr '/:@' '___'; }
    mkdir -p "$S/img"
    case "$sub" in
      pull)
        while [ $# -gt 0 ]; do case "$1" in --progress|--arch|--scheme) shift 2 ;; *) break ;; esac; done
        image="$1"
        if [ -f "$S/private/$(key "$image")" ] && [ ! -f "$S/img/$(key "$image")" ]; then
          echo "Error: refusing insecure credential exchange: authorization server https://ghcr.io/token issued its own authentication challenge" >&2; exit 1
        fi
        [ -f "$S/img/$(key "$image")" ] || printf 'sha256:pulled' > "$S/img/$(key "$image")"
        echo "$image" ;;
      load)
        [ "$1" = "--input" ] || { echo "load: no --input" >&2; exit 2; }
        # The fake archive: IMAGE=<ref> and DIGEST=<d> lines.
        ref=$(sed -n 's/^IMAGE=//p' "$2"); dg=$(sed -n 's/^DIGEST=//p' "$2")
        [ -n "$ref" ] || { echo "Error: not an OCI archive" >&2; exit 1; }
        printf '%s' "$dg" > "$S/img/$(key "$ref")"
        echo "$ref" ;;
      inspect)
        [ -f "$S/img/$(key "$1")" ] || { echo "Error: image not found: $1" >&2; exit 1; }
        printf '[{"id":"i","variants":[{"digest":"%s","platform":{"architecture":"arm64","os":"linux"},"config":{"architecture":"arm64","config":{"Cmd":["/bin/true"],"User":"cua"}}}]}]\n' "$(cat "$S/img/$(key "$1")")" ;;
      *) echo "fake container: unknown image command $sub" >&2; exit 2 ;;
    esac ;;
  logs) echo "boot failed: no init" ;;
  exec)
    [ "$1" = "--interactive" ] && shift
    n="$1"; shift
    [ "$(cat "$S/c/$n.state" 2>/dev/null)" = running ] || { echo "Error: container $n is not running" >&2; exit 1; }
    # Never run a guest's hostname change on the host: record it.
    case "$*" in *"hostname "*) printf '%s' "$3" > "$S/c/$n.hostname"; exit 0 ;; esac
    if [ "$1" = cat ]; then
      echo "  sl  local_address rem_address   st"
      [ -f "$S/c/$n.listen" ] && echo "   0: 00000000:0C8B 00000000:0000 0A"
      exit 0
    fi
    "$@" ;;
  *) echo "fake container: unknown command $cmd" >&2; exit 2 ;;
esac
"#;

/// A fake `container` with its own state directory.
pub struct FakeContainer {
    pub dir: tempfile::TempDir,
}

impl FakeContainer {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        std::fs::create_dir_all(state.join("c")).unwrap();
        let bin = dir.path().join("container");
        std::fs::write(
            &bin,
            format!("#!/bin/bash\nS='{}'\n{SCRIPT}", state.display()),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        Self { dir }
    }

    pub fn state(&self) -> PathBuf {
        self.dir.path().join("state")
    }

    pub fn root(&self) -> PathBuf {
        self.dir.path().join("root")
    }

    pub fn runtime(&self) -> ContainerizationRuntime {
        ContainerizationRuntime::new(ContainerizationConfig {
            root: self.root(),
            bin: Some(self.dir.path().join("container")),
            auto_install: false,
            graceful_stop: std::time::Duration::from_secs(1),
            resolver: None,
        })
    }

    /// [`Self::runtime`] with a resolver (images are pulled through it).
    pub fn runtime_with(
        &self,
        resolver: std::sync::Arc<dyn RootfsResolver>,
    ) -> ContainerizationRuntime {
        ContainerizationRuntime::new(ContainerizationConfig {
            root: self.root(),
            bin: Some(self.dir.path().join("container")),
            auto_install: false,
            graceful_stop: std::time::Duration::from_secs(1),
            resolver: Some(resolver),
        })
    }

    /// Makes `image` private: `container` cannot pull it anonymously.
    pub fn make_private(&self, image: &str) {
        std::fs::create_dir_all(self.state().join("private")).unwrap();
        let key: String = image
            .chars()
            .map(|c| if matches!(c, '/' | ':' | '@') { '_' } else { c })
            .collect();
        std::fs::write(self.state().join("private").join(key), "").unwrap();
    }

    /// Every call, one line each (`args | APP_ROOT=...`).
    pub fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.state().join("calls"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    pub fn file(&self, rel: &str) -> Option<String> {
        std::fs::read_to_string(self.state().join(rel)).ok()
    }

    pub fn touch(&self, rel: &str, contents: &str) {
        std::fs::write(self.state().join(rel), contents).unwrap();
    }

    pub fn exists(&self, rel: &str) -> bool {
        Path::new(&self.state().join(rel)).exists()
    }
}
