#!/usr/bin/env python3
"""Detect Hyprland builds that need plugin requalification, and plan the matrix.

A build is keyed by every tracked package's version and package SHA-256, so a
same-version Arch rebuild (a pkgrel bump, or a rebuilt file under the same
version) is a new build. A matching Hyprland version string alone is not
sufficient (docs/omarchy-edge-20261004-validation.md).

This only reads public package databases and the Hyprland release API. It
never publishes anything.
"""

import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tarfile
import time
import urllib.error
import urllib.request

# The plugin's exact ABI pins as Omarchy packages them (omacom/omarchy-pkgs#772),
# plus the compiler. A change to any of them can break the module ABI or make
# the pinned package uninstallable.
TRACKED = (
    "hyprland", "aquamarine", "hyprutils", "hyprlang", "hyprcursor", "hyprgraphics",
    "glibc", "gcc", "libgcc", "libstdc++", "libxkbcommon", "wayland",
)
ARCH_MIRROR = "https://geo.mirror.pkgbuild.com/$repo/os/$arch"

# Repository order matters: rc and stable list [omarchy] first, edge last
# (omacom/omarchy default/pacman/pacman-*.conf).
CHANNELS = {
    "arch": {
        "description": "Arch Linux core/extra (live)",
        "repos": [("core", ARCH_MIRROR), ("extra", ARCH_MIRROR)],
    },
    "omarchy-edge": {
        "description": "Omarchy edge (mirror.omarchy.org + pkgs.omarchy.org/edge)",
        "repos": [("core", "https://mirror.omarchy.org/$repo/os/$arch"),
                  ("extra", "https://mirror.omarchy.org/$repo/os/$arch"),
                  ("omarchy", "https://pkgs.omarchy.org/edge/$arch")],
    },
    "omarchy-rc": {
        "description": "Omarchy rc (rc-mirror.omarchy.org + pkgs.omarchy.org/rc)",
        "repos": [("omarchy", "https://pkgs.omarchy.org/rc/$arch"),
                  ("core", "https://rc-mirror.omarchy.org/$repo/os/$arch"),
                  ("extra", "https://rc-mirror.omarchy.org/$repo/os/$arch")],
    },
    "omarchy-stable": {
        "description": "Omarchy stable (stable-mirror.omarchy.org + pkgs.omarchy.org/stable)",
        "repos": [("omarchy", "https://pkgs.omarchy.org/stable/$arch"),
                  ("core", "https://stable-mirror.omarchy.org/$repo/os/$arch"),
                  ("extra", "https://stable-mirror.omarchy.org/$repo/os/$arch")],
    },
}
PLUGIN_PACKAGE = "cua-hyprland-plugin"
DRIVER_PACKAGE = "cua-driver-bin"
HYPRLAND_RELEASES = "https://api.github.com/repos/hyprwm/Hyprland/releases?per_page=10"
STABLE_TAG = re.compile(r"v?([0-9]+\.[0-9]+\.[0-9]+)")


def repo_url(server, repo, arch="x86_64"):
    return server.replace("$repo", repo).replace("$arch", arch)


def fetch(url, attempts=5, delay=2.0, opener=urllib.request.urlopen, sleep=time.sleep, headers=None):
    """GET with exponential backoff on network errors, 429 and 5xx."""
    request = urllib.request.Request(url, headers={"User-Agent": "cua-hyprland-requalify", **(headers or {})})
    for attempt in range(attempts):
        try:
            with opener(request, timeout=60) as response:
                return response.read()
        except urllib.error.HTTPError as error:
            retryable = error.code in (403, 429) or error.code >= 500
            if not retryable or attempt == attempts - 1:
                raise
        except (urllib.error.URLError, TimeoutError, ConnectionError):
            if attempt == attempts - 1:
                raise
        sleep(delay * (2 ** attempt))
    raise RuntimeError("unreachable")


ZSTD_MAGIC = b"\x28\xb5\x2f\xfd"


def decompress(data):
    """Pacman databases are gzip (Arch) or zstd (Omarchy); tarfile reads only the former."""
    if not data.startswith(ZSTD_MAGIC):
        return data
    try:
        from compression import zstd  # Python 3.14+
        return zstd.decompress(data)
    except ImportError:
        return subprocess.run(["zstd", "-dc"], input=data, capture_output=True, check=True).stdout


def parse_db(data):
    """Return {name: {version, sha256, builddate, depends}} from a pacman sync database."""
    data = decompress(data)
    packages = {}
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:*") as archive:
        for member in archive:
            if not member.isfile() or not member.name.endswith("/desc"):
                continue
            fields, key = {}, None
            for line in archive.extractfile(member).read().decode().splitlines():
                if line.startswith("%") and line.endswith("%"):
                    key = line.strip("%")
                    fields[key] = []
                elif line and key:
                    fields[key].append(line)
            name = fields.get("NAME", [None])[0]
            if not name:
                continue
            packages[name] = {
                "version": fields["VERSION"][0],
                "sha256": fields.get("SHA256SUM", [""])[0],
                "builddate": int(fields.get("BUILDDATE", ["0"])[0]),
                "depends": fields.get("DEPENDS", []),
                "filename": fields.get("FILENAME", [""])[0],
            }
    return packages


def exact_pins(depends):
    """Exact name=version pins from a DEPENDS list (no ranges, no soname provides)."""
    pins = {}
    for entry in depends:
        match = re.fullmatch(r"([^<>=]+)=([^<>=]+)", entry)
        if match and ".so" not in match[1]:
            pins[match[1]] = match[2]
    return pins


def resolve_channel(name, databases):
    """First repository in pacman order wins, as pacman resolves it."""
    resolved = {}
    for repo, _ in CHANNELS[name]["repos"]:
        for package, info in databases[repo].items():
            if package in TRACKED and package not in resolved:
                resolved[package] = dict(version=info["version"], sha256=info["sha256"], repo=repo)
    missing = sorted(set(TRACKED) - set(resolved))
    if missing:
        raise ValueError(f"{name}: tracked packages missing: {', '.join(missing)}")
    entry = {"packages": {package: resolved[package] for package in TRACKED}}
    omarchy = databases.get("omarchy", {})
    if PLUGIN_PACKAGE in omarchy:
        plugin = omarchy[PLUGIN_PACKAGE]
        pins = exact_pins(plugin["depends"])
        # Pins the channel no longer satisfies make `pacman -Syu` refuse the update.
        broken = {name: {"pinned": version, "channel": resolved[name]["version"]}
                  for name, version in sorted(pins.items())
                  if name in resolved and resolved[name]["version"] != version}
        entry["omarchy_plugin"] = {"version": plugin["version"], "pins": pins,
                                   "installable": not broken, "broken_pins": broken}
    if DRIVER_PACKAGE in omarchy:
        entry["omarchy_driver"] = {"version": omarchy[DRIVER_PACKAGE]["version"]}
    # An [omarchy] override of a tracked package needs that repository in the build.
    entry["needs_omarchy_repo"] = any(info["repo"] == "omarchy" for info in resolved.values())
    entry["abi_key"] = abi_key(entry["packages"])
    return entry


def abi_key(packages):
    identity = {name: [info["version"], info["sha256"]] for name, info in sorted(packages.items())}
    return hashlib.sha256(json.dumps(identity, sort_keys=True).encode()).hexdigest()[:16]


def header_version(package_version):
    """'0.56.2-4' -> '0.56.2' (epoch and pkgrel removed)."""
    return package_version.split(":", 1)[-1].rsplit("-", 1)[0]


def version_tuple(value):
    return tuple(int(part) for part in value.split("."))


def detect(channels=None, fetcher=fetch, token=None):
    snapshot = {"schema": 1, "detected_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
                "tracked": list(TRACKED), "channels": {}, "errors": {}}
    for name in channels or CHANNELS:
        try:
            databases = {repo: parse_db(fetcher(repo_url(server, repo) + f"/{repo}.db"))
                         for repo, server in CHANNELS[name]["repos"]}
            snapshot["channels"][name] = resolve_channel(name, databases)
        except Exception as error:  # A down mirror is recorded, not fatal for other channels.
            snapshot["errors"][name] = f"{type(error).__name__}: {error}"
    try:
        headers = {"Authorization": f"Bearer {token}"} if token else None
        releases = json.loads(fetcher(HYPRLAND_RELEASES, headers=headers) if headers else fetcher(HYPRLAND_RELEASES))
        stable = [r for r in releases if not r.get("draft") and not r.get("prerelease")
                  and STABLE_TAG.fullmatch(r.get("tag_name", ""))]
        if stable:
            latest = max(stable, key=lambda r: version_tuple(STABLE_TAG.fullmatch(r["tag_name"])[1]))
            snapshot["upstream"] = {"tag": latest["tag_name"], "version": STABLE_TAG.fullmatch(latest["tag_name"])[1],
                                    "published_at": latest.get("published_at"), "url": latest.get("html_url")}
    except Exception as error:
        snapshot["errors"]["hyprland-github"] = f"{type(error).__name__}: {error}"
    return snapshot


def versions(channel_info):
    return {name: info["version"] for name, info in channel_info["packages"].items()}


def plan(snapshot, sources, manifest=None, force=False):
    """Build matrix: one job per (distinct ABI key, distinct plugin tree).

    A combination that already passed is skipped unless forced. build-only and
    fail are retried, so a persistent failure is reported on every run and a
    transient one clears itself. Channels sharing an ABI key share one build.
    """
    done = {}
    for entry in (manifest or {}).get("entries", []):
        if entry.get("status") == "pass":
            done[(entry["abi_key"], entry["plugin"]["tree"])] = entry["status"]
    builds = {}
    for channel, info in sorted(snapshot["channels"].items()):
        build = builds.setdefault(info["abi_key"], {
            "abi_key": info["abi_key"], "channels": [], "channel": channel,
            "hyprland": info["packages"]["hyprland"]["version"],
            "needs_omarchy_repo": info["needs_omarchy_repo"], "expected_packages": versions(info)})
        build["channels"].append(channel)
        # Prefer a configuration that does not need the Omarchy repository.
        if build["needs_omarchy_repo"] and not info["needs_omarchy_repo"]:
            build.update(channel=channel, needs_omarchy_repo=False, expected_packages=versions(info))
    trees = {}
    for source in sources:
        trees.setdefault(source["tree"], {**source, "refs": []})["refs"].append(source["ref"])
    include = []
    for build in builds.values():
        for tree in trees.values():
            if not force and (build["abi_key"], tree["tree"]) in done:
                continue
            include.append({
                "id": f"{build['abi_key']}-{tree['tree'][:12]}",
                "abi_key": build["abi_key"], "channel": build["channel"], "channels": build["channels"],
                "hyprland": build["hyprland"], "needs_omarchy_repo": build["needs_omarchy_repo"],
                "expected_packages": build["expected_packages"],
                "ref": tree["ref"], "refs": tree["refs"], "tree": tree["tree"], "commit": tree.get("commit", ""),
                "driver_version": tree.get("driver_version", ""),
            })
    return include


def upstream_status(snapshot):
    """Is upstream Hyprland ahead of every packaged channel?"""
    upstream = snapshot.get("upstream")
    if not upstream:
        return None
    packaged = [header_version(c["packages"]["hyprland"]["version"]) for c in snapshot["channels"].values()]
    ahead = all(version_tuple(upstream["version"]) > version_tuple(v) for v in packaged) if packaged else True
    return {**upstream, "ahead_of_packages": ahead}


PLUGIN_PATH = "libs/cua-driver/hyprland-plugin"
NON_BUILD = re.compile(r"(requalify|docs)/|[^/]+\.md$")
DRIVER_TAG = re.compile(r"cua-driver-rs-v([0-9]+\.[0-9]+\.[0-9]+)")


def git(repo, *args):
    return subprocess.check_output(["git", "-C", str(repo), *args], text=True).strip()


def plugin_sources(repo, snapshot, head="HEAD"):
    """main, the latest stable Driver tag, and each Omarchy channel's packaged Driver source."""
    tags = [t for t in git(repo, "tag", "-l", "cua-driver-rs-v*").splitlines() if DRIVER_TAG.fullmatch(t)]
    tags.sort(key=lambda t: version_tuple(DRIVER_TAG.fullmatch(t)[1]))
    wanted = [("main", head, "")]
    if tags:
        wanted.append((tags[-1], tags[-1], DRIVER_TAG.fullmatch(tags[-1])[1]))
    for channel in snapshot["channels"].values():
        plugin = channel.get("omarchy_plugin")
        if plugin:
            version = header_version(plugin["version"])
            tag = f"cua-driver-rs-v{version}"
            if tag in tags:
                wanted.append((tag, tag, version))
    sources, seen = [], set()
    for label, ref, version in wanted:
        if label in seen:
            continue
        seen.add(label)
        commit = git(repo, "rev-parse", f"{ref}^{{commit}}")
        sources.append({"ref": label, "commit": commit, "driver_version": version,
                        "tree": source_id(repo, commit)})
    return sources


def source_id(repo, commit):
    """Content ID of the plugin's build inputs at a commit.

    Documentation and this requalification tooling (which lives under the
    plugin directory) do not change the module, so they do not make a new
    plugin build.
    """
    listing = git(repo, "ls-tree", "-r", commit, "--", PLUGIN_PATH).splitlines()
    inputs = [line for line in listing if not NON_BUILD.match(line.split("\t", 1)[1][len(PLUGIN_PATH) + 1:])]
    return hashlib.sha1("\n".join(inputs).encode()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    d = sub.add_parser("detect", help="write a snapshot of every channel's tracked packages")
    d.add_argument("--output", type=Path, required=True)
    d.add_argument("--channel", action="append", choices=sorted(CHANNELS))
    s = sub.add_parser("sources", help="write the plugin sources to build")
    s.add_argument("--snapshot", type=Path, required=True)
    s.add_argument("--repo", type=Path, default=Path("."))
    s.add_argument("--head", default="HEAD")
    s.add_argument("--output", type=Path, required=True)
    p = sub.add_parser("plan", help="write the build matrix for new combinations")
    p.add_argument("--snapshot", type=Path, required=True)
    p.add_argument("--sources", type=Path, required=True, help="JSON list of {ref, tree, driver_version}")
    p.add_argument("--manifest", type=Path, help="previous compatibility manifest")
    p.add_argument("--force", action="store_true")
    p.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.command == "detect":
        snapshot = detect(args.channel, token=os.environ.get("GITHUB_TOKEN"))
        snapshot["upstream"] = upstream_status(snapshot)
        args.output.write_text(json.dumps(snapshot, indent=2, sort_keys=True) + "\n")
        for name, error in snapshot["errors"].items():
            print(f"::warning::{name}: {error}")
        if not snapshot["channels"]:
            sys.exit("no channel could be read")
    elif args.command == "sources":
        found = plugin_sources(args.repo, json.loads(args.snapshot.read_text()), args.head)
        args.output.write_text(json.dumps(found, indent=2) + "\n")
        for source in found:
            print(f"{source['ref']}: {source['commit'][:12]} tree {source['tree'][:12]}")
    else:
        snapshot = json.loads(args.snapshot.read_text())
        manifest = json.loads(args.manifest.read_text()) if args.manifest and args.manifest.is_file() else None
        include = plan(snapshot, json.loads(args.sources.read_text()), manifest, args.force)
        args.output.write_text(json.dumps({"include": include}) + "\n")
        print(f"{len(include)} build(s) planned")


if __name__ == "__main__":
    main()
