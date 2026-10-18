//! "Ensure X is installed" for harnesses and apps alike.
//!
//! One manifest ([`MANIFEST`], `installables.json`) lists every known
//! installable with a pinned version and a checksum from its publisher:
//!
//! - `archive`: a release archive per CPU, verified by sha256 before it is
//!   unpacked;
//! - `npm`: registry tarballs verified against their pinned sha512
//!   `integrity`, then installed with the sandbox's own npm (their
//!   dependencies are resolved by npm and checked against the registry's
//!   integrity metadata);
//! - `git-uv`: a repository at a pinned commit (checked with `git
//!   rev-parse`), installed with `uv sync --frozen`, so every dependency is
//!   checked against the project's own `uv.lock` hashes.
//!
//! [`script`] turns a set of ids (dependencies first) into one POSIX shell
//! script that runs inside the sandbox. It is idempotent: an item lands in
//! `$HOME/.cua/tools/<id>/<version>` behind a `.cua-installed` marker, a
//! marker under `/opt/cua/tools/<id>/<version>` (baked into an image) is used
//! as is, and its binaries are linked into `$HOME/.cua/bin`. It reports
//! progress as `CUA_PROGRESS {json}` lines on stdout and, when
//! `CUA_EVENTS_FILE` is set, as `install` events in that run's event log.
//!
//! An app's OS dependencies (`apt`: the shared-library packages its own
//! Debian package `Depends` on) are resolved alongside its download: through
//! apt as root or with passwordless sudo, else (a gVisor sandbox honours no
//! setuid bit, so `sudo` cannot work there) rootless, by fetching the
//! missing packages with apt into a private sysroot
//! (`$HOME/.cua/tools/.sysroot`) that the app's launcher puts on
//! `LD_LIBRARY_PATH`. Its `ldd` files are then checked, so a library that is
//! still missing fails the install by name instead of the launch.
//!
//! An archive already staged at `$HOME/.cua/tools/.incoming/<sha256>.<ext>`
//! (the SDK sends one from its host cache, see [`staged_archives`]) is used
//! instead of a download, and verified like one.

use crate::quote;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

/// The manifest, as shipped.
pub const MANIFEST: &str = include_str!("../installables.json");

/// Where tools are installed, under the guest user's home.
pub const TOOLS_DIR: &str = ".cua/tools";
/// Where their binaries are linked, under the guest user's home.
pub const BIN_DIR: &str = ".cua/bin";
/// A read-only cache an image can bake installs into.
pub const IMAGE_TOOLS_DIR: &str = "/opt/cua/tools";
/// Prefix of a progress line.
pub const PROGRESS_PREFIX: &str = "CUA_PROGRESS ";

/// One archive for one CPU.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Archive {
    /// Download URLs, tried in order (mirrors).
    pub urls: Vec<String>,
    /// Lowercase hex sha256 of the archive.
    pub sha256: String,
}

/// One pinned npm package.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NpmPackage {
    pub name: String,
    pub version: String,
    pub tarball: String,
    /// `sha512-<base64>` (the registry's `dist.integrity`).
    pub integrity: String,
}

/// How an item is installed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Source {
    Archive {
        /// `tar.gz`, `tar.xz`, `zip` or `deb` (its payload; `strip` does
        /// not apply).
        format: String,
        /// Leading path components to drop.
        #[serde(default)]
        strip: u32,
        /// By `uname -m` family: `aarch64`, `x86_64`.
        archives: BTreeMap<String, Archive>,
    },
    Npm {
        packages: Vec<NpmPackage>,
    },
    GitUv {
        repo: String,
        commit: String,
        #[serde(default)]
        extras: Vec<String>,
        python: String,
    },
}

/// One installable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Installable {
    /// Display name.
    pub name: String,
    pub version: String,
    /// SPDX or the publisher's terms, for the record.
    pub license: String,
    /// Where the pin and its checksum came from.
    pub source: String,
    #[serde(flatten)]
    pub how: Source,
    /// Binaries: name linked into `~/.cua/bin` -> path inside the install.
    #[serde(default)]
    pub bins: BTreeMap<String, String>,
    /// Other installables this one needs first (npm items always need
    /// `node`).
    #[serde(default)]
    pub needs: Vec<String>,
    /// Debian/Ubuntu packages of the shared libraries the app loads (for an
    /// app that ships a `.deb`, its `Depends` libraries). `a|b` accepts
    /// either (the first one apt has is installed). Checked with dpkg,
    /// installed through apt as root or with passwordless sudo, else into
    /// the rootless sysroot; reported, not fatal, where apt is unavailable
    /// (the `ldd` check is the gate).
    #[serde(default)]
    pub apt: Vec<String>,
    /// ELF files inside the install whose shared libraries must all resolve
    /// (`ldd`) once `apt` is in place; a missing one fails the install.
    #[serde(default)]
    pub ldd: Vec<String>,
}

#[derive(Deserialize)]
struct Manifest {
    items: BTreeMap<String, Installable>,
}

/// Every known installable, by id.
pub fn all() -> &'static BTreeMap<String, Installable> {
    static ALL: OnceLock<BTreeMap<String, Installable>> = OnceLock::new();
    ALL.get_or_init(|| {
        serde_json::from_str::<Manifest>(MANIFEST)
            .expect("installables.json is valid (checked by tests)")
            .items
    })
}

/// One installable by id.
pub fn get(id: &str) -> Option<&'static Installable> {
    all().get(id)
}

impl Installable {
    /// Ids this one depends on, npm's implicit `node` included.
    pub fn deps(&self) -> Vec<String> {
        let mut d = self.needs.clone();
        if matches!(self.how, Source::Npm { .. }) && !d.iter().any(|x| x == "node") {
            d.insert(0, "node".into());
        }
        d
    }

    /// CPUs it is published for (every CPU for npm and git sources).
    pub fn arches(&self) -> Vec<&str> {
        match &self.how {
            Source::Archive { archives, .. } => archives.keys().map(String::as_str).collect(),
            _ => vec!["aarch64", "x86_64"],
        }
    }
}

/// `ids` and everything they need, dependencies first, each once.
pub fn resolve(ids: &[&str]) -> crate::Result<Vec<String>> {
    fn visit(
        id: &str,
        seen: &mut BTreeSet<String>,
        stack: &mut Vec<String>,
        out: &mut Vec<String>,
    ) -> crate::Result<()> {
        if seen.contains(id) {
            return Ok(());
        }
        if stack.iter().any(|s| s == id) {
            return Err(crate::Error::Invalid(format!("installable cycle at {id}")));
        }
        let item = get(id).ok_or_else(|| {
            crate::Error::Invalid(format!(
                "unknown installable {id:?}; known: {}",
                all().keys().cloned().collect::<Vec<_>>().join(", ")
            ))
        })?;
        stack.push(id.into());
        for d in item.deps() {
            visit(&d, seen, stack, out)?;
        }
        stack.pop();
        seen.insert(id.into());
        out.push(id.into());
        Ok(())
    }
    let mut seen = BTreeSet::new();
    let mut out = vec![];
    for id in ids {
        visit(id, &mut seen, &mut vec![], &mut out)?;
    }
    Ok(out)
}

/// One progress report from the script.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    pub id: String,
    /// `check`, `cached`, `download`, `verify`, `install`, `deps` (OS
    /// packages), `done`, `error`.
    pub phase: String,
    #[serde(default)]
    pub detail: String,
}

impl Progress {
    /// Parses a `CUA_PROGRESS {json}` line.
    pub fn parse(line: &str) -> Option<Progress> {
        serde_json::from_str(line.trim().strip_prefix(PROGRESS_PREFIX)?).ok()
    }
}

const PRELUDE: &str = r#"set -u
CUA_TOOLS="$HOME/.cua/tools"; CUA_BIN="$HOME/.cua/bin"
mkdir -p "$CUA_TOOLS" "$CUA_BIN"
export PATH="$CUA_BIN:$PATH"
case "$(uname -m)" in aarch64|arm64) CUA_ARCH=aarch64 ;; x86_64|amd64) CUA_ARCH=x86_64 ;; *) CUA_ARCH="$(uname -m)" ;; esac
# Archive keys: the arch on Linux, `macos-<arch>` on macOS guests.
case "$(uname -s)" in Darwin) CUA_PLATFORM="macos-$CUA_ARCH" ;; *) CUA_PLATFORM="$CUA_ARCH" ;; esac
cua_progress() {
  # A detail can quote a tool's message: keep the JSON line valid.
  set -- "$1" "$2" "$(printf '%s' "$3" | tr -d '"\\' | tr '\n\t' '  ')"
  printf 'CUA_PROGRESS {"id":"%s","phase":"%s","detail":"%s"}\n' "$1" "$2" "$3"
  if [ -n "${CUA_EVENTS_FILE:-}" ]; then
    cua_n=$(( $(wc -l < "$CUA_EVENTS_FILE" 2>/dev/null || echo 0) + 1 ))
    printf '{"seq":%s,"ts":%s000,"turn":0,"type":"install","id":"%s","phase":"%s","detail":"%s"}\n' \
      "$cua_n" "$(date +%s)" "$1" "$2" "$3" >> "$CUA_EVENTS_FILE"
  fi
}
cua_fail() { cua_progress "$1" error "$2"; exit 1; }
cua_fetch() {
  if command -v curl >/dev/null 2>&1; then curl -fsSL --retry 3 --connect-timeout 20 -o "$2" "$1"
  elif command -v wget >/dev/null 2>&1; then wget -q -O "$2" "$1"
  else return 127; fi
}
cua_sha256() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1
  else shasum -a 256 "$1" | cut -d' ' -f1; fi
}
cua_lock() {
  i=0; while ! mkdir "$1.lock" 2>/dev/null; do
    i=$((i+1)); [ $i -gt 1800 ] && return 1; sleep 1
  done
}
cua_unlock() { rmdir "$1.lock" 2>/dev/null || true; }
CUA_SYSROOT="$CUA_TOOLS/.sysroot"; CUA_INCOMING="$CUA_TOOLS/.incoming"
# "" as root, "sudo -n" with passwordless sudo that works, else "none".
cua_root() {
  if [ "$(id -u)" = 0 ]; then echo ""; elif command -v sudo >/dev/null 2>&1 && sudo -n true 2>/dev/null; then echo "sudo -n"; else echo none; fi
}
# A system package through apt, as root or with passwordless sudo.
cua_apt() {
  cua_su="$(cua_root)"; [ "$cua_su" = none ] && return 1
  command -v apt-get >/dev/null 2>&1 || return 1
  cua_lock "$CUA_TOOLS/.apt-run" || return 1
  { $cua_su env DEBIAN_FRONTEND=noninteractive apt-get install -y -qq "$@" >/dev/null 2>&1 \
    || { $cua_su apt-get update -qq >/dev/null 2>&1 && $cua_su env DEBIAN_FRONTEND=noninteractive apt-get install -y -qq "$@" >/dev/null 2>&1; }; }
  cua_rc=$?; cua_unlock "$CUA_TOOLS/.apt-run"; return $cua_rc
}
# The rootless sysroot's library directories.
cua_libpath() {
  printf '%s' "$CUA_SYSROOT/usr/lib/$CUA_ARCH-linux-gnu:$CUA_SYSROOT/lib/$CUA_ARCH-linux-gnu:$CUA_SYSROOT/usr/lib"
}
# Installed package names and what they provide (dpkg and the sysroot), one
# per line; cua_pkg_have reads it.
cua_have_list() {
  dpkg-query -W -f='${db:Status-Abbrev}|${Package}|${Provides}\n' 2>/dev/null | sed -n 's/^ii *|//p' \
    | tr '|,' '\n\n' | sed 's/(.*//; s/:.*//; s/[[:space:]]//g' | grep -v '^$'
  ls "$CUA_SYSROOT/.pkgs" 2>/dev/null
  return 0
}
# Whether a package, or any of `a|b`, is in $cua_have.
cua_pkg_have() {
  cua_ifs=$IFS; IFS='|'; set -- $1; IFS=$cua_ifs
  for cua_p in "$@"; do
    printf '%s\n' "$cua_have" | grep -qxF "$cua_p" && return 0
  done
  return 1
}
# `a (>= 1), b | c` (a Debian Depends field) to one `a` / `b|c` per line.
cua_depends_groups() {
  tr ',' '\n' | sed -e 's/([^)]*)//g' -e 's/:any//g' -e 's/[[:space:]]//g' | grep -v '^$' | sort -u
  return 0
}
# The first of `a|b` apt can install (apt's lists are in $cua_aptopt).
cua_pkg_pick() {
  cua_ifs=$IFS; IFS='|'; set -- $1; IFS=$cua_ifs
  for cua_p in "$@"; do
    apt-cache $cua_aptopt policy "$cua_p" 2>/dev/null | grep -q 'Candidate: [^(]' && { echo "$cua_p"; return 0; }
  done
  echo "$1"
}
# An app's OS packages: the missing ones through apt as root or with sudo,
# else rootless into the sysroot. Not fatal (the ldd check is the gate).
cua_sysdeps() {
  cua_id=$1; shift
  cua_have="$(cua_have_list)"; cua_miss=""
  for cua_g in "$@"; do cua_pkg_have "$cua_g" || cua_miss="$cua_miss $cua_g"; done
  [ -n "$cua_miss" ] || { cua_progress "$cua_id" deps satisfied; return 0; }
  command -v apt-get >/dev/null 2>&1 || { cua_progress "$cua_id" deps "no apt-get; missing:$cua_miss"; return 0; }
  cua_lock "$CUA_TOOLS/.apt-run" || return 0
  cua_su="$(cua_root)"
  if [ "$cua_su" = none ]; then
    cua_apt_dir="$CUA_TOOLS/.apt"; mkdir -p "$cua_apt_dir/lists/partial" "$cua_apt_dir/archives/partial" "$CUA_SYSROOT/.pkgs"
    cua_aptopt="-o Dir::State::Lists=$cua_apt_dir/lists -o Dir::Cache=$cua_apt_dir -o Dir::Cache::archives=$cua_apt_dir/archives -o Dir::Cache::pkgcache= -o Dir::Cache::srcpkgcache= -o Debug::NoLocking=1 -o APT::Sandbox::User=$(id -un)"
    cua_lists="$cua_apt_dir/lists"
  else
    cua_aptopt=""; cua_lists=/var/lib/apt/lists
  fi
  if ! ls "$cua_lists"/*_Packages* >/dev/null 2>&1; then
    cua_progress "$cua_id" deps "apt-get update"
    cua_run="$cua_su"; [ "$cua_su" = none ] && cua_run=""
    cua_err="$($cua_run apt-get $cua_aptopt update -qq 2>&1 >/dev/null)" \
      || cua_progress "$cua_id" deps "apt-get update: $(printf '%s' "$cua_err" | grep -v '^rm: ' | tail -n 3)"
  fi
  cua_pkgs=""; for cua_g in $cua_miss; do cua_pkgs="$cua_pkgs $(cua_pkg_pick "$cua_g")"; done
  if [ "$cua_su" = none ]; then
    cua_progress "$cua_id" deps "rootless:$cua_pkgs"
    rm -f "$cua_apt_dir/archives/"*.deb
    if cua_err="$(apt-get $cua_aptopt install -y -qq --no-install-recommends --download-only $cua_pkgs 2>&1 >/dev/null)"; then
      for cua_f in "$cua_apt_dir/archives/"*.deb; do
        [ -f "$cua_f" ] || continue
        dpkg-deb -x "$cua_f" "$CUA_SYSROOT" || continue
        for cua_n in $(dpkg-deb -f "$cua_f" Package; dpkg-deb -f "$cua_f" Provides | cua_depends_groups); do
          : > "$CUA_SYSROOT/.pkgs/$cua_n"
        done
      done
    else
      cua_progress "$cua_id" deps "apt download failed:$cua_pkgs: $(printf '%s' "$cua_err" | grep '^E:' | tail -n 2)"
    fi
  else
    cua_progress "$cua_id" deps "apt-get install$cua_pkgs"
    $cua_su env DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends $cua_pkgs >/dev/null 2>&1 \
      || cua_progress "$cua_id" deps "apt-get install failed:$cua_pkgs"
  fi
  cua_unlock "$CUA_TOOLS/.apt-run"
}
# Fails the install when any of the ELF files (relative to $CUA_DIR) has a
# shared library that does not resolve. Skipped where there is no ldd.
cua_libcheck() {
  cua_id=$1; shift
  command -v ldd >/dev/null 2>&1 || return 0
  cua_nf=""
  for cua_e in "$@"; do
    [ -f "$CUA_DIR/$cua_e" ] || continue
    cua_nf="$cua_nf $(LD_LIBRARY_PATH="$(cua_libpath)${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" ldd "$CUA_DIR/$cua_e" 2>/dev/null \
      | sed -n 's/^[[:space:]]*\([^[:space:]]*\) => not found.*/\1/p' | sort -u | tr '\n' ' ')"
  done
  cua_nf="$(echo $cua_nf)"
  [ -z "$cua_nf" ] || cua_fail "$cua_id" "missing shared libraries: $cua_nf"
}
cua_have() {
  if [ -f "/opt/cua/tools/$1/$2/.cua-installed" ]; then CUA_DIR="/opt/cua/tools/$1/$2"; return 0; fi
  CUA_DIR="$CUA_TOOLS/$1/$2"; [ -f "$CUA_DIR/.cua-installed" ]
}
cua_link() { ln -sfn "$CUA_DIR/$2" "$CUA_BIN/$1"; }
# A launcher that puts the rootless sysroot on LD_LIBRARY_PATH (a plain link
# while the sysroot is empty).
cua_link_sysroot() {
  if [ -n "$(ls "$CUA_SYSROOT/.pkgs" 2>/dev/null)" ]; then
    rm -f "$CUA_BIN/$1.tmp"
    printf '#!/bin/sh\n# cua launcher: %s with the rootless sysroot\nLD_LIBRARY_PATH="%s${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"\nexport LD_LIBRARY_PATH\nexec "%s" "$@"\n' \
      "$1" "$(cua_libpath)" "$CUA_DIR/$2" > "$CUA_BIN/$1.tmp" && chmod 755 "$CUA_BIN/$1.tmp" && mv -f "$CUA_BIN/$1.tmp" "$CUA_BIN/$1"
  else
    cua_link "$1" "$2"
  fi
}
# Downloads $1 to $2, reporting the bytes so far every few seconds.
cua_fetch_progress() {
  cua_fetch "$1" "$2" &
  cua_fp=$!
  while kill -0 "$cua_fp" 2>/dev/null; do
    sleep 1
    [ -f "$2" ] && cua_progress "$3" download "$(wc -c < "$2" | tr -d ' ') bytes"
  done
  wait "$cua_fp"
}
"#;

fn step(id: &str, it: &Installable) -> String {
    let v = quote(&it.version);
    let qid = quote(id);
    let mut s = format!(
        "# ---- {id} {version}\n\
         if cua_have {qid} {v}; then cua_progress {qid} cached \"$CUA_DIR\"; else\n\
         cua_progress {qid} check {v}\n\
         cua_lock \"$CUA_TOOLS/{id}-{version}\" || cua_fail {qid} 'install lock timed out'\n\
         if ! cua_have {qid} {v}; then\n\
         CUA_STAGE=\"$CUA_TOOLS/.stage-{id}-$$\"; rm -rf \"$CUA_STAGE\"; mkdir -p \"$CUA_STAGE\"\n",
        version = it.version,
    );
    match &it.how {
        Source::Archive {
            format,
            strip,
            archives,
        } => {
            s.push_str("case \"$CUA_PLATFORM\" in\n");
            for (arch, a) in archives {
                s.push_str(&format!(
                    "  {arch}) CUA_URLS={urls_q}; CUA_SUM={sum} ;;\n",
                    urls_q = quote(&a.urls.join(" ")),
                    sum = quote(&a.sha256)
                ));
            }
            s.push_str(&format!(
                "  *) cua_unlock \"$CUA_TOOLS/{id}-{version}\"; cua_fail {qid} \"not published for $CUA_PLATFORM\" ;;\nesac\n",
                version = it.version
            ));
            let ext = format.clone();
            s.push_str(&format!(
                "CUA_OK=\n\
                 if [ -f \"$CUA_INCOMING/$CUA_SUM.{ext}\" ]; then cua_progress {qid} download staged; \
                 mv -f \"$CUA_INCOMING/$CUA_SUM.{ext}\" \"$CUA_STAGE/a.{ext}\" && CUA_OK=1; fi\n\
                 [ -n \"$CUA_OK\" ] || for u in $CUA_URLS; do cua_progress {qid} download \"$u\"; \
                 if cua_fetch_progress \"$u\" \"$CUA_STAGE/a.{ext}\" {qid}; then CUA_OK=1; break; fi; done\n\
                 [ -n \"$CUA_OK\" ] || {{ rm -rf \"$CUA_STAGE\"; cua_unlock \"$CUA_TOOLS/{id}-{version}\"; cua_fail {qid} 'download failed'; }}\n\
                 cua_progress {qid} verify sha256\n\
                 [ \"$(cua_sha256 \"$CUA_STAGE/a.{ext}\")\" = \"$CUA_SUM\" ] || {{ rm -rf \"$CUA_STAGE\"; cua_unlock \"$CUA_TOOLS/{id}-{version}\"; cua_fail {qid} 'sha256 mismatch'; }}\n\
                 cua_progress {qid} install unpack\n\
                 mkdir -p \"$CUA_STAGE/out\"\n",
                version = it.version
            ));
            let unpack = match format.as_str() {
                "zip" => "{ command -v unzip >/dev/null 2>&1 || cua_apt unzip || true; } && (cd \"$CUA_STAGE/out\" && (command -v unzip >/dev/null 2>&1 && unzip -q ../a.zip || python3 -m zipfile -e ../a.zip .)) \
                     && chmod -R u+rwX \"$CUA_STAGE/out\" && find \"$CUA_STAGE/out\" -maxdepth 1 -type f -exec chmod +x {} +"
                    .to_string(),
                // A Debian package's payload, unpacked without running its
                // maintainer scripts (no root needed); its own `Depends` is
                // kept for `finish`.
                "deb" => "{ if command -v dpkg-deb >/dev/null 2>&1; then dpkg-deb -x \"$CUA_STAGE/a.deb\" \"$CUA_STAGE/out\" \
                     && dpkg-deb -f \"$CUA_STAGE/a.deb\" Depends > \"$CUA_STAGE/out/.cua-depends\"; \
                     else (cd \"$CUA_STAGE\" && ar x a.deb && tar -xf data.tar.* -C out); fi; }"
                    .to_string(),
                "tar.xz" => format!(
                    "tar -xJf \"$CUA_STAGE/a.tar.xz\" -C \"$CUA_STAGE/out\" --strip-components={strip}"
                ),
                _ => format!(
                    "tar -xzf \"$CUA_STAGE/a.{ext}\" -C \"$CUA_STAGE/out\" --strip-components={strip}"
                ),
            };
            s.push_str(&format!(
                "{unpack} || {{ rm -rf \"$CUA_STAGE\"; cua_unlock \"$CUA_TOOLS/{id}-{version}\"; cua_fail {qid} 'unpack failed'; }}\n\
                 mkdir -p \"$CUA_TOOLS/{id}\"; rm -rf \"$CUA_TOOLS/{id}/{version}\"; mv \"$CUA_STAGE/out\" \"$CUA_TOOLS/{id}/{version}\"\n",
                version = it.version
            ));
        }
        Source::Npm { packages } => {
            s.push_str(
                "command -v npm >/dev/null 2>&1 || { cua_unlock \"$CUA_TOOLS/ID-VER\"; cua_fail ID 'npm is not available'; }\n"
                    .replace("ID-VER", &format!("{id}-{}", it.version))
                    .replace(" ID ", &format!(" {qid} "))
                    .as_str(),
            );
            let mut files = vec![];
            for (i, p) in packages.iter().enumerate() {
                let f = format!("\"$CUA_STAGE/p{i}.tgz\"");
                s.push_str(&format!(
                    "cua_progress {qid} download {pkg}\n\
                     cua_fetch {url} {f} || {{ rm -rf \"$CUA_STAGE\"; cua_unlock \"$CUA_TOOLS/{id}-{version}\"; cua_fail {qid} 'download failed'; }}\n\
                     cua_progress {qid} verify {integ_kind}\n\
                     [ \"$(node -e 'process.stdout.write(\"sha512-\"+require(\"crypto\").createHash(\"sha512\").update(require(\"fs\").readFileSync(process.argv[1])).digest(\"base64\"))' {f})\" = {integ} ] \
                     || {{ rm -rf \"$CUA_STAGE\"; cua_unlock \"$CUA_TOOLS/{id}-{version}\"; cua_fail {qid} 'integrity mismatch for {name}'; }}\n",
                    pkg = quote(&format!("{}@{}", p.name, p.version)),
                    url = quote(&p.tarball),
                    integ_kind = quote("sha512 integrity"),
                    integ = quote(&p.integrity),
                    name = p.name,
                    version = it.version,
                ));
                files.push(f);
            }
            s.push_str(&format!(
                "cua_progress {qid} install npm\n\
                 mkdir -p \"$CUA_STAGE/out\" && (cd \"$CUA_STAGE/out\" && npm install --no-audit --no-fund --loglevel=error --omit=dev {files} >\"$CUA_STAGE/npm.log\" 2>&1) \
                 || {{ tail -5 \"$CUA_STAGE/npm.log\" >&2; rm -rf \"$CUA_STAGE\"; cua_unlock \"$CUA_TOOLS/{id}-{version}\"; cua_fail {qid} 'npm install failed'; }}\n\
                 mkdir -p \"$CUA_TOOLS/{id}\"; rm -rf \"$CUA_TOOLS/{id}/{version}\"; mv \"$CUA_STAGE/out\" \"$CUA_TOOLS/{id}/{version}\"\n",
                files = files.join(" "),
                version = it.version,
            ));
        }
        Source::GitUv {
            repo,
            commit,
            extras,
            python,
        } => {
            let extras = extras
                .iter()
                .map(|e| format!("--extra {}", quote(e)))
                .collect::<Vec<_>>()
                .join(" ");
            s.push_str(&format!(
                "command -v git >/dev/null 2>&1 || {{ cua_progress {qid} install 'git (apt)'; cua_apt git; }}\n\
                 command -v git >/dev/null 2>&1 || {{ cua_unlock \"$CUA_TOOLS/{id}-{version}\"; cua_fail {qid} 'git is not available'; }}\n\
                 cua_progress {qid} download {commit_q}\n\
                 CUA_DEST=\"$CUA_TOOLS/{id}/{version}\"; rm -rf \"$CUA_DEST\"; mkdir -p \"$CUA_TOOLS/{id}\"\n\
                 (git init -q \"$CUA_DEST\" && cd \"$CUA_DEST\" && git fetch -q --depth 1 {repo} {commit_q} && git checkout -q FETCH_HEAD) \
                 || {{ rm -rf \"$CUA_STAGE\" \"$CUA_DEST\"; cua_unlock \"$CUA_TOOLS/{id}-{version}\"; cua_fail {qid} 'git fetch failed'; }}\n\
                 cua_progress {qid} verify commit\n\
                 [ \"$(cd \"$CUA_DEST\" && git rev-parse HEAD)\" = {commit_q} ] || {{ rm -rf \"$CUA_STAGE\" \"$CUA_DEST\"; cua_unlock \"$CUA_TOOLS/{id}-{version}\"; cua_fail {qid} 'commit mismatch'; }}\n\
                 cua_progress {qid} install 'uv sync --frozen'\n\
                 (cd \"$CUA_DEST\" && UV_PYTHON_INSTALL_DIR=\"$CUA_TOOLS/python\" uv sync --quiet --frozen --no-dev {extras} --python {python} >\"$CUA_STAGE/uv.log\" 2>&1) \
                 || {{ tail -5 \"$CUA_STAGE/uv.log\" >&2; rm -rf \"$CUA_STAGE\" \"$CUA_DEST\"; cua_unlock \"$CUA_TOOLS/{id}-{version}\"; cua_fail {qid} 'uv sync failed'; }}\n",
                repo = quote(repo),
                commit_q = quote(commit),
                python = quote(python),
                version = it.version,
            ));
        }
    }
    // An app with OS dependencies is finished (checked, linked, `done`)
    // once they are in place: see [`finish`].
    let done = if it.apt.is_empty() {
        format!("cua_progress {qid} done \"$CUA_DIR\"\n")
    } else {
        String::new()
    };
    s.push_str(&format!(
        "rm -rf \"$CUA_STAGE\"; printf %s {v} > \"$CUA_TOOLS/{id}/{version}/.cua-installed\"\n\
         fi\n\
         cua_unlock \"$CUA_TOOLS/{id}-{version}\"\n\
         cua_have {qid} {v}\n\
         {done}\
         fi\n",
        version = it.version
    ));
    if !it.apt.is_empty() {
        return s;
    }
    for (bin, rel) in &it.bins {
        s.push_str(&format!("cua_link {} {}\n", quote(bin), quote(rel)));
    }
    // A runtime's own bin dir first, so npm finds this node.
    if let Some(rel) = it.bins.get("node") {
        let dir = rel.rsplit_once('/').map(|(d, _)| d).unwrap_or(".");
        s.push_str(&format!("export PATH=\"$CUA_DIR/{dir}:$PATH\"\n"));
    }
    s
}

/// The line that puts `it`'s OS packages in place (empty when it has none).
fn sysdeps(id: &str, it: &Installable) -> String {
    if it.apt.is_empty() {
        return String::new();
    }
    let pkgs = it
        .apt
        .iter()
        .map(|p| quote(p))
        .collect::<Vec<_>>()
        .join(" ");
    format!("cua_sysdeps {} {pkgs}\n", quote(id))
}

/// After its OS packages: the `ldd` check, the launchers and `done`.
fn finish(id: &str, it: &Installable) -> String {
    let qid = quote(id);
    let mut s = format!("cua_have {qid} {}\n", quote(&it.version));
    if matches!(&it.how, Source::Archive { format, .. } if format == "deb") {
        // The package's own Depends, beyond what the manifest declared.
        s.push_str(&format!(
            "[ -f \"$CUA_DIR/.cua-depends\" ] && cua_sysdeps {qid} $(cua_depends_groups < \"$CUA_DIR/.cua-depends\")\n"
        ));
    }
    if !it.ldd.is_empty() {
        let files = it
            .ldd
            .iter()
            .map(|f| quote(f))
            .collect::<Vec<_>>()
            .join(" ");
        s.push_str(&format!("cua_libcheck {qid} {files}\n"));
    }
    for (bin, rel) in &it.bins {
        s.push_str(&format!("cua_link_sysroot {} {}\n", quote(bin), quote(rel)));
    }
    s.push_str(&format!("cua_progress {qid} done \"$CUA_DIR\"\n"));
    s
}

/// The install script for `ids` and their dependencies. OS packages are
/// resolved in the background while the archives download and unpack.
pub fn script(ids: &[&str]) -> crate::Result<String> {
    let order = resolve(ids)?;
    let with_deps: Vec<&String> = order
        .iter()
        .filter(|id| !get(id).expect("resolved").apt.is_empty())
        .collect();
    let mut s = String::from(PRELUDE);
    if !with_deps.is_empty() {
        s.push_str("(\n");
        for id in &with_deps {
            s.push_str(&sysdeps(id, get(id).expect("resolved")));
        }
        s.push_str(") &\nCUA_DEPS_PID=$!\n");
    }
    for id in &order {
        s.push_str(&step(id, get(id).expect("resolved")));
    }
    if !with_deps.is_empty() {
        s.push_str("wait \"$CUA_DEPS_PID\"\n");
        for id in &with_deps {
            s.push_str(&finish(id, get(id).expect("resolved")));
        }
        // The rootless apt lists (kept across the passes above).
        s.push_str("rm -rf \"$CUA_TOOLS/.apt\"\n");
    }
    Ok(s)
}

/// Only the OS packages of `ids` (and their dependencies), for a caller
/// that stages the archives itself meanwhile (the SDK's host cache).
/// [`script`] then finds them in place.
pub fn deps_script(ids: &[&str]) -> crate::Result<String> {
    let order = resolve(ids)?;
    let mut s = String::from(PRELUDE);
    for id in &order {
        s.push_str(&sysdeps(id, get(id).expect("resolved")));
    }
    Ok(s)
}

/// Where the SDK stages archives in the guest, under the guest user's home.
pub const INCOMING_DIR: &str = ".cua/tools/.incoming";

/// An archive the SDK can fetch once on the host and send to a sandbox
/// instead of the sandbox downloading it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagedArchive {
    pub id: String,
    pub version: String,
    /// Mirrors, in order.
    pub urls: Vec<String>,
    /// Lowercase hex sha256.
    pub sha256: String,
    /// `tar.gz`, `tar.xz` or `zip`.
    pub format: String,
}

impl StagedArchive {
    /// The file name the install script looks for in [`INCOMING_DIR`].
    pub fn file_name(&self) -> String {
        format!("{}.{}", self.sha256, self.format)
    }

    /// The guest path for a guest home.
    pub fn guest_path(&self, home: &str) -> String {
        format!(
            "{}/{INCOMING_DIR}/{}",
            home.trim_end_matches('/'),
            self.file_name()
        )
    }

    /// The guest's install marker (the item is already installed when
    /// either exists, so nothing needs staging).
    pub fn markers(&self, home: &str) -> [String; 2] {
        [
            format!(
                "{IMAGE_TOOLS_DIR}/{}/{}/.cua-installed",
                self.id, self.version
            ),
            format!(
                "{}/{TOOLS_DIR}/{}/{}/.cua-installed",
                home.trim_end_matches('/'),
                self.id,
                self.version
            ),
        ]
    }
}

/// The archives `ids` (and their dependencies) download for `arch`
/// (`aarch64`, `x86_64`), in install order.
pub fn staged_archives(ids: &[&str], arch: &str) -> crate::Result<Vec<StagedArchive>> {
    let mut out = vec![];
    for id in resolve(ids)? {
        let it = get(&id).expect("resolved");
        if let Source::Archive {
            format, archives, ..
        } = &it.how
            && let Some(a) = archives.get(arch)
        {
            out.push(StagedArchive {
                id: id.clone(),
                version: it.version.clone(),
                urls: a.urls.clone(),
                sha256: a.sha256.clone(),
                format: format.clone(),
            });
        }
    }
    Ok(out)
}

/// The directory an installed item lives in (for paths the SDK writes,
/// such as the runner's `node_modules` link).
pub fn install_dir(home: &str, id: &str) -> Option<String> {
    let it = get(id)?;
    Some(format!(
        "{}/{TOOLS_DIR}/{id}/{}",
        home.trim_end_matches('/'),
        it.version
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_manifest_parses_and_every_pin_is_well_formed() {
        assert!(all().len() >= 10);
        for (id, it) in all() {
            match &it.how {
                Source::Archive {
                    archives, format, ..
                } => {
                    assert!(
                        ["tar.gz", "tar.xz", "zip", "deb"].contains(&format.as_str()),
                        "{id}"
                    );
                    assert!(!archives.is_empty(), "{id}");
                    for a in archives.values() {
                        assert_eq!(a.sha256.len(), 64, "{id}");
                        assert!(a.sha256.bytes().all(|b| b.is_ascii_hexdigit()), "{id}");
                        assert!(a.urls.iter().all(|u| u.starts_with("https://")), "{id}");
                    }
                }
                Source::Npm { packages } => {
                    for p in packages {
                        assert!(p.integrity.starts_with("sha512-"), "{id}");
                        assert!(p.tarball.starts_with("https://registry.npmjs.org/"), "{id}");
                        assert!(p.tarball.ends_with(&format!("-{}.tgz", p.version)), "{id}");
                    }
                }
                Source::GitUv { commit, repo, .. } => {
                    assert_eq!(commit.len(), 40, "{id}");
                    assert!(repo.starts_with("https://"), "{id}");
                }
            }
            for d in it.deps() {
                assert!(get(&d).is_some(), "{id} needs unknown {d}");
            }
        }
    }

    #[test]
    fn resolve_orders_dependencies_first_once() {
        let order = resolve(&["pi", "claude-agent-acp", "hermes"]).unwrap();
        assert_eq!(
            order,
            vec!["node", "pi", "claude-agent-acp", "uv", "hermes"]
        );
        assert!(resolve(&["nope"]).is_err());
    }

    #[test]
    fn progress_lines_round_trip() {
        let p = Progress::parse(r#"CUA_PROGRESS {"id":"node","phase":"cached","detail":"/x"}"#);
        assert_eq!(p.unwrap().phase, "cached");
        assert!(Progress::parse("hello").is_none());
    }

    /// The script is valid POSIX shell (`sh -n`) for every item.
    #[test]
    fn scripts_parse() {
        let ids: Vec<&str> = all().keys().map(String::as_str).collect();
        let s = script(&ids).unwrap();
        let out = std::process::Command::new("/bin/sh")
            .arg("-n")
            .arg("-c")
            .arg(&s)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Against a temp HOME with a fake cached install: nothing is fetched,
    /// the bin is linked and progress says `cached`.
    #[test]
    fn a_cached_install_is_reused_and_linked() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join(".cua/tools/goose/1.52.0");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("goose"), "#!/bin/sh\necho goose\n").unwrap();
        std::fs::write(dir.join(".cua-installed"), "1.52.0").unwrap();
        let out = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(script(&["goose"]).unwrap())
            .env("HOME", home.path())
            .env("PATH", "/usr/bin:/bin")
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success(),
            "{stdout}{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let p: Vec<Progress> = stdout.lines().filter_map(Progress::parse).collect();
        assert_eq!(p[0].phase, "cached");
        let link = std::fs::read_link(home.path().join(".cua/bin/goose")).unwrap();
        assert_eq!(link, dir.join("goose"));
    }

    // ---- OS dependencies, against stub dpkg/apt/ldd (hermetic: a temp
    // HOME, a PATH whose first entry holds the stubs; `sudo` is always a
    // stub, so the host's own sudo is never run).

    /// The VS Code package's `Depends`, as the pinned .deb carries it.
    const VSCODE_DEPENDS: &str = "ca-certificates, libasound2 (>= 1.0.17), libatk-bridge2.0-0 (>= 2.5.3), libatk1.0-0 (>= 2.11.90), libatspi2.0-0 (>= 2.9.90), libc6 (>= 2.17), libc6 (>= 2.25), libc6 (>= 2.28), libcairo2 (>= 1.6.0), libcups2 (>= 1.6.0), libcurl3-gnutls | libcurl3-nss | libcurl4 | libcurl3, libdbus-1-3 (>= 1.9.14), libexpat1 (>= 2.1~beta3), libgbm1 (>= 17.1.0~rc2), libglib2.0-0 (>= 2.39.4), libgtk-3-0 (>= 3.9.10), libgtk-3-0 (>= 3.9.10) | libgtk-4-1, libnspr4 (>= 2:4.9-2~), libnss3 (>= 2:3.30), libnss3 (>= 3.26), libpango-1.0-0 (>= 1.14.0), libstdc++6 (>= 4.1.1), libstdc++6 (>= 5), libstdc++6 (>= 5.2), libstdc++6 (>= 6), libstdc++6 (>= 9), libudev1 (>= 183), libx11-6, libx11-6 (>= 2:1.4.99.1), libxcb1 (>= 1.9.2), libxcomposite1 (>= 1:0.4.4-1), libxdamage1 (>= 1:1.1), libxext6, libxfixes3, libxkbcommon0 (>= 0.5.0), libxkbfile1 (>= 1:1.1.0), libxrandr2, xdg-utils (>= 1.0.2)";

    struct Fake {
        home: tempfile::TempDir,
        dir: std::path::PathBuf,
    }

    const STUBS: &[(&str, &str)] = &[
        // `${db:Status-Abbrev}|${Package}|${Provides}` rows from dpkg.txt.
        (
            "dpkg-query",
            "#!/bin/sh\ncat \"$FAKE/dpkg.txt\" 2>/dev/null\n",
        ),
        (
            "apt-get",
            r#"#!/bin/sh
echo "apt-get $*" >> "$FAKE/calls.log"
lists=""; archives=""; download=""; op=""; pkgs=""
while [ $# -gt 0 ]; do
  case "$1" in
    -o) case "$2" in Dir::State::Lists=*) lists="${2#*=}" ;; Dir::Cache::archives=*) archives="${2#*=}" ;; esac; shift 2; continue ;;
    --download-only) download=1 ;;
    update|install) op="$1" ;;
    -*) ;;
    *) pkgs="$pkgs $1" ;;
  esac; shift
done
[ -f "$FAKE/apt-fails" ] && exit 100
if [ "$op" = update ] && [ -n "$lists" ]; then mkdir -p "$lists" && : > "$lists/fake_Packages"; fi
if [ "$op" = install ] && [ -n "$download" ]; then for p in $pkgs; do : > "$archives/$p.deb"; done; fi
if [ "$op" = install ] && [ -z "$download" ]; then for p in $pkgs; do echo "ii |$p|" >> "$FAKE/dpkg.txt"; done; fi
exit 0
"#,
        ),
        (
            "apt-cache",
            "#!/bin/sh\nfor a; do p=$a; done\nif grep -qxF \"$p\" \"$FAKE/avail.txt\" 2>/dev/null; then echo \"  Candidate: 1.0\"; else echo \"  Candidate: (none)\"; fi\n",
        ),
        (
            "dpkg-deb",
            r#"#!/bin/sh
echo "dpkg-deb $*" >> "$FAKE/calls.log"
case "$1" in
  -x) p=$(basename "$2" .deb); mkdir -p "$3/usr/lib/$(uname -m | sed 's/arm64/aarch64/')-linux-gnu" && : > "$3/usr/lib/$(uname -m | sed 's/arm64/aarch64/')-linux-gnu/$p.so" ;;
  -f) [ "$3" = Package ] && basename "$2" .deb ;;
esac
exit 0
"#,
        ),
        (
            "id",
            "#!/bin/sh\ncase \"$1\" in -u) echo \"${FAKE_UID:-1000}\" ;; -un) echo cua ;; *) echo cua ;; esac\n",
        ),
        (
            "sudo",
            "#!/bin/sh\n[ -f \"$FAKE/sudo-ok\" ] || exit 1\n[ \"$1\" = -n ] && shift\nexec \"$@\"\n",
        ),
        // Each library in needs.txt resolves from LD_LIBRARY_PATH or the
        // system libs in syslib.txt, else is `not found`.
        (
            "ldd",
            r#"#!/bin/sh
while read -r lib; do
  [ -n "$lib" ] || continue
  found=""
  grep -qxF "$lib" "$FAKE/syslib.txt" 2>/dev/null && found=/usr/lib/$lib
  old=$IFS; IFS=:; for d in ${LD_LIBRARY_PATH:-}; do [ -f "$d/$lib" ] && found="$d/$lib"; done; IFS=$old
  if [ -n "$found" ]; then echo "	$lib => $found (0x0)"; else echo "	$lib => not found"; fi
done < "$FAKE/needs.txt"
"#,
        ),
    ];

    impl Fake {
        fn new() -> Fake {
            let home = tempfile::tempdir().unwrap();
            let dir = home.path().join("fake");
            let bin = dir.join("bin");
            std::fs::create_dir_all(&bin).unwrap();
            for (name, body) in STUBS {
                let p = bin.join(name);
                std::fs::write(&p, body).unwrap();
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            Fake { home, dir }
        }

        fn write(&self, name: &str, body: &str) {
            std::fs::write(self.dir.join(name), body).unwrap();
        }

        fn calls(&self) -> String {
            std::fs::read_to_string(self.dir.join("calls.log")).unwrap_or_default()
        }

        /// A fake installed `id` (so nothing downloads), with `.cua-depends`.
        fn installed(&self, id: &str, depends: &str) -> std::path::PathBuf {
            let it = get(id).unwrap();
            let d = self
                .home
                .path()
                .join(format!(".cua/tools/{id}/{}", it.version));
            std::fs::create_dir_all(&d).unwrap();
            for rel in it.bins.values().chain(it.ldd.iter()) {
                let p = d.join(rel);
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(&p, "#!/bin/sh\n").unwrap();
            }
            std::fs::write(d.join(".cua-depends"), depends).unwrap();
            std::fs::write(d.join(".cua-installed"), &it.version).unwrap();
            d
        }

        fn run(&self, script: &str, uid: u32) -> (bool, Vec<Progress>, String) {
            let out = std::process::Command::new("/bin/sh")
                .arg("-c")
                .arg(script)
                .env_clear()
                .env("HOME", self.home.path())
                .env("FAKE", &self.dir)
                .env("FAKE_UID", uid.to_string())
                .env(
                    "PATH",
                    format!("{}:/usr/bin:/bin", self.dir.join("bin").display()),
                )
                .output()
                .unwrap();
            let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
            let p = stdout.lines().filter_map(Progress::parse).collect();
            (
                out.status.success(),
                p,
                format!("{stdout}{}", String::from_utf8_lossy(&out.stderr)),
            )
        }
    }

    fn details(p: &[Progress], phase: &str) -> Vec<String> {
        p.iter()
            .filter(|p| p.phase == phase)
            .map(|p| p.detail.clone())
            .collect()
    }

    /// Every system row a stock Ubuntu 24.04 desktop has for VS Code except
    /// NSS: `ii |pkg|provides`.
    fn stock_dpkg(except: &[&str]) -> String {
        let mut rows = String::new();
        for g in &get("vscode").unwrap().apt {
            let mut alts = g.split('|');
            let p = alts.next().unwrap();
            if !except.contains(&p) {
                // 24.04's t64 packages provide the old names (as dpkg
                // reports them: `name (= version), ...`).
                let provides: Vec<String> = alts
                    .filter(|a| !a.contains("curl4"))
                    .map(|a| format!("{a} (= 1.0)"))
                    .collect();
                rows.push_str(&format!("ii |{p}|{}\n", provides.join(", ")));
            }
        }
        // Removed but configured: not installed.
        rows.push_str("ii |libc6|\nrc |libnss3|\n");
        rows
    }

    #[test]
    fn a_debian_depends_field_becomes_one_group_per_line() {
        let out = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(format!(
                "{PRELUDE}\nprintf '%s' \"$1\" | cua_depends_groups | sort -u"
            ))
            .arg("sh")
            .arg(VSCODE_DEPENDS)
            .env("HOME", tempfile::tempdir().unwrap().path())
            .output()
            .unwrap();
        let groups: Vec<String> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::to_string)
            .collect();
        assert!(groups.contains(&"libnss3".to_string()));
        assert!(groups.contains(&"libnspr4".to_string()));
        assert!(groups.contains(&"libcurl3-gnutls|libcurl3-nss|libcurl4|libcurl3".to_string()));
        assert!(groups.contains(&"libgtk-3-0|libgtk-4-1".to_string()));
        assert!(
            groups.iter().all(|g| !g.contains(['(', ' ', ','])),
            "{groups:?}"
        );
    }

    /// The manifest's declared list covers the pinned package's own
    /// Depends (so the parallel pass leaves nothing for `finish`).
    #[test]
    fn vscode_declares_every_depends_of_its_package() {
        let declared: Vec<Vec<&str>> = get("vscode")
            .unwrap()
            .apt
            .iter()
            .map(|g| g.split('|').collect())
            .collect();
        for group in VSCODE_DEPENDS.split(',') {
            let alts: Vec<String> = group
                .split('|')
                .map(|a| a.split('(').next().unwrap().trim().to_string())
                .collect();
            if alts.iter().any(|a| a == "libc6") {
                continue; // always present
            }
            let covered = declared.iter().any(|d| {
                d.iter().any(|x| {
                    alts.iter().any(|a| {
                        x == a || x.strip_suffix("t64") == Some(a) || x.replace("t64-", "-") == *a
                    })
                })
            });
            assert!(covered, "vscode's apt list misses {group:?}");
        }
    }

    /// Every GUI app the teleport catalog installs declares its OS
    /// libraries and an `ldd` gate.
    #[test]
    fn gui_archive_apps_declare_os_libraries() {
        for id in ["vscode", "blender"] {
            let it = get(id).unwrap();
            assert!(!it.apt.is_empty(), "{id} declares its OS packages");
            assert!(!it.ldd.is_empty(), "{id} has an ldd check");
        }
    }

    #[test]
    fn deps_already_present_touch_nothing() {
        let f = Fake::new();
        f.write("dpkg.txt", &stock_dpkg(&[]));
        f.write("needs.txt", "libnss3.so\n");
        f.write("syslib.txt", "libnss3.so\n");
        let dir = f.installed("vscode", VSCODE_DEPENDS);
        let (ok, p, log) = f.run(&script(&["vscode"]).unwrap(), 1000);
        assert!(ok, "{log}");
        assert_eq!(details(&p, "deps"), ["satisfied", "satisfied"], "{log}");
        assert_eq!(f.calls(), "", "no apt at all");
        // No sysroot: a plain link.
        let link = f.home.path().join(".cua/bin/code");
        assert_eq!(
            std::fs::read_link(&link).unwrap(),
            dir.join("usr/share/code/bin/code")
        );
        assert_eq!(p.last().unwrap().phase, "done");
    }

    #[test]
    fn as_root_missing_packages_come_from_apt_with_one_update() {
        let f = Fake::new();
        f.write(
            "dpkg.txt",
            &stock_dpkg(&["libnss3", "libnspr4", "libsecret-1-0"]),
        );
        f.write("avail.txt", "libnss3\nlibnspr4\nlibsecret-1-0\n");
        f.write("needs.txt", "libnss3.so\n");
        f.write("syslib.txt", "libnss3.so\n");
        f.installed("vscode", VSCODE_DEPENDS);
        let (ok, p, log) = f.run(&script(&["vscode"]).unwrap(), 0);
        assert!(ok, "{log}");
        let calls = f.calls();
        assert_eq!(calls.matches("apt-get update").count(), 1, "{calls}");
        assert!(
            calls.contains("install -y -qq --no-install-recommends libnspr4 libnss3 libsecret-1-0"),
            "{calls}"
        );
        assert!(!calls.contains("--download-only"), "{calls}");
        // The second pass (the package's Depends) finds them installed.
        assert_eq!(details(&p, "deps").last().unwrap(), "satisfied", "{log}");
    }

    #[test]
    fn passwordless_sudo_is_used_when_not_root() {
        let f = Fake::new();
        f.write("sudo-ok", "");
        f.write("dpkg.txt", &stock_dpkg(&["libnss3"]));
        f.write("avail.txt", "libnss3\n");
        f.write("needs.txt", "");
        f.installed("vscode", VSCODE_DEPENDS);
        let (ok, _, log) = f.run(&script(&["vscode"]).unwrap(), 1000);
        assert!(ok, "{log}");
        let calls = f.calls();
        assert!(calls.contains("--no-install-recommends libnss3"), "{calls}");
        assert!(
            !calls.contains("Dir::State::Lists"),
            "system apt, not rootless: {calls}"
        );
    }

    /// No root and no sudo (a gVisor sandbox): the packages land in the
    /// rootless sysroot and the launcher puts it on LD_LIBRARY_PATH.
    #[test]
    fn without_root_packages_land_in_the_sysroot_and_the_launcher_uses_it() {
        let f = Fake::new();
        f.write("dpkg.txt", &stock_dpkg(&["libnss3", "libnspr4"]));
        f.write("avail.txt", "libnss3\nlibnspr4\n");
        // `libnss3.so` only exists once the sysroot has it.
        f.write("needs.txt", "libnss3.so\n");
        let dir = f.installed("vscode", VSCODE_DEPENDS);
        let (ok, p, log) = f.run(&script(&["vscode"]).unwrap(), 1000);
        assert!(ok, "{log}");
        let calls = f.calls();
        assert!(calls.contains("Dir::State::Lists="), "{calls}");
        // One private `apt-get update`, then the download.
        assert_eq!(
            calls
                .lines()
                .filter(|l| l.contains("Dir::State::Lists=") && l.ends_with("update -qq"))
                .count(),
            1,
            "{calls}"
        );
        assert!(
            calls.contains("--download-only libnspr4 libnss3"),
            "{calls}"
        );
        assert!(!log.contains("not found"), "{log}");
        assert!(calls.contains("dpkg-deb -x"), "{calls}");
        let sysroot = f.home.path().join(".cua/tools/.sysroot");
        assert!(sysroot.join(".pkgs/libnss3").exists());
        assert!(
            !f.home.path().join(".cua/tools/.apt").exists(),
            "apt scratch removed"
        );
        let launcher = std::fs::read_to_string(f.home.path().join(".cua/bin/code")).unwrap();
        assert!(
            launcher.contains("LD_LIBRARY_PATH=\"") && launcher.contains(".sysroot/usr/lib/"),
            "{launcher}"
        );
        assert!(
            launcher.contains(&format!(
                "exec \"{}\"",
                dir.join("usr/share/code/bin/code").display()
            )),
            "{launcher}"
        );
        assert!(
            details(&p, "deps")
                .iter()
                .any(|d| d.starts_with("rootless:")),
            "{log}"
        );
        assert_eq!(p.last().unwrap().phase, "done");
    }

    /// A library nothing provides fails the install by name (not the launch
    /// with exit 127).
    #[test]
    fn an_unresolved_library_fails_the_install_by_name() {
        let f = Fake::new();
        f.write("dpkg.txt", &stock_dpkg(&["libnss3"]));
        f.write("apt-fails", "");
        f.write("needs.txt", "libnss3.so\nlibc.so.6\n");
        f.write("syslib.txt", "libc.so.6\n");
        f.installed("vscode", VSCODE_DEPENDS);
        let (ok, p, log) = f.run(&script(&["vscode"]).unwrap(), 0);
        assert!(!ok, "{log}");
        let err = p.iter().find(|p| p.phase == "error").unwrap();
        assert_eq!(err.detail, "missing shared libraries: libnss3.so", "{log}");
    }

    /// The deps-only script resolves the same packages and nothing else.
    #[test]
    fn the_deps_script_only_resolves_os_packages() {
        let s = deps_script(&["vscode"]).unwrap();
        assert!(s.contains("cua_sysdeps vscode"));
        assert!(!s.contains("cua_fetch_progress \"$u\""));
        let f = Fake::new();
        f.write("dpkg.txt", &stock_dpkg(&[]));
        let (ok, p, log) = f.run(&s, 1000);
        assert!(ok, "{log}");
        assert_eq!(details(&p, "deps"), ["satisfied"]);
    }

    #[test]
    fn macos_guests_pick_macos_archives() {
        let script = script(&["node"]).unwrap();
        assert!(script.contains(r#"Darwin) CUA_PLATFORM="macos-$CUA_ARCH""#));
        assert!(script.contains(r#"case "$CUA_PLATFORM" in"#));
        assert!(script.contains("node-v24.21.0-darwin-arm64.tar.xz"));
        assert!(script.contains("node-v24.21.0-linux-arm64.tar.xz"));
        let node = get("node").unwrap();
        let Source::Archive { archives, .. } = &node.how else {
            panic!("node is an archive")
        };
        for key in ["aarch64", "x86_64", "macos-aarch64", "macos-x86_64"] {
            assert_eq!(archives[key].sha256.len(), 64, "{key}");
        }
    }

    #[test]
    fn staged_archives_name_the_file_the_script_looks_for() {
        let a = staged_archives(&["vscode"], "aarch64").unwrap();
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].format, "deb");
        assert_eq!(
            a[0].guest_path("/home/cua/"),
            format!("/home/cua/.cua/tools/.incoming/{}.deb", a[0].sha256)
        );
        assert!(
            a[0].markers("/home/cua")[1].ends_with("/.cua/tools/vscode/1.139.0/.cua-installed")
        );
        // Blender is published for x86_64 only; npm items stage nothing.
        assert!(staged_archives(&["blender"], "aarch64").unwrap().is_empty());
        assert!(
            staged_archives(&["claude-code"], "aarch64")
                .unwrap()
                .iter()
                .all(|a| a.id == "node")
        );
        // The script uses a staged file before any download.
        let s = script(&["vscode"]).unwrap();
        assert!(s.contains("\"$CUA_INCOMING/$CUA_SUM.deb\""));
    }
}
