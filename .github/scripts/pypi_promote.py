"""Promote a GitHub release's Python wheels to PyPI (docs/pypi-publishing.md).

Every Cua Driver and cua SDK build attaches its wheels to its GitHub release.
Only this script, run by .github/workflows/pypi-promote.yml, puts them on
PyPI. It picks the newest *promotable* build:

  - a published (not draft) GitHub release with a plain X.Y.Z version;
  - at least --min-age-hours old (a soak, so a broken build gets replaced
    before it reaches PyPI);
  - its component's release workflow has a successful run for the tag;
  - every expected wheel is attached and matches the release's checksums.

Subcommands:
  plan   --component C [--version V]   decide what to promote; writes
         tag/version/action/reason (and stale/pypi_version) to $GITHUB_OUTPUT
  fetch  --component C --tag T --dir D download and verify the wheels; writes
         D/upload-order.txt (one path per line, dependencies first)

`action` is `promote` (upload), `skip` (PyPI already has it, or it is older
than PyPI's newest) or `none` (no promotable build). `stale` is `true` when
PyPI's newest release of the component is more than --max-stale-days older
than the newest promotable build: the workflow fails on it, so PyPI updates
never stop silently.
"""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import os
import re
import subprocess
import sys
import urllib.error
import urllib.request
import zipfile
from dataclasses import dataclass
from pathlib import Path

REPO = os.environ.get("GITHUB_REPOSITORY", "trycua/cua")
STRICT_VERSION = re.compile(r"^\d+\.\d+\.\d+$")


@dataclass(frozen=True)
class Component:
    name: str
    tag_prefix: str
    workflow: str
    checksums: str
    # (PyPI project, wheel filename prefix, required platform tags). For
    # cua-cli a single py3-none-any wheel also satisfies the set.
    projects: tuple[tuple[str, str, tuple[str, ...]], ...]


DRIVER_TAGS = (
    "py3-none-macosx_13_0_universal2",
    "py3-none-manylinux_2_31_x86_64",
    "py3-none-manylinux_2_31_aarch64",
    "py3-none-win_amd64",
    "py3-none-win_arm64",
)
SDK_TAGS = (
    "py3-none-macosx_11_0_arm64",
    "py3-none-macosx_10_12_x86_64",
    "py3-none-manylinux_2_31_x86_64",
    "py3-none-manylinux_2_31_aarch64",
    "py3-none-win_amd64",
    "py3-none-win_arm64",
)
COMPONENTS = {
    "cua-driver": Component(
        "cua-driver", "cua-driver-rs-v", "cd-rust-cua-driver.yml", "python-sdk-checksums.txt",
        (("cua-driver", "cua_driver", DRIVER_TAGS),),
    ),
    "cua-sdk": Component(
        "cua-sdk", "cua-sdk-v", "cd-cua-sdk.yml", "checksums.txt",
        # Upload order: cua requires cua-cli==<version>.
        (("cua-cli", "cua_cli", SDK_TAGS), ("cua", "cua", SDK_TAGS)),
    ),
}


def gh_json(args: list[str]):
    out = subprocess.run(["gh", *args], check=True, capture_output=True, text=True).stdout
    return json.loads(out) if out.strip() else None


def pypi(project: str) -> dict | None:
    try:
        with urllib.request.urlopen(f"https://pypi.org/pypi/{project}/json", timeout=30) as r:
            return json.load(r)
    except urllib.error.HTTPError as e:
        if e.code == 404:
            return None
        raise


def vkey(v: str) -> tuple[int, ...]:
    return tuple(int(p) for p in v.split("."))


def ts(s: str) -> dt.datetime:
    return dt.datetime.fromisoformat(s.replace("Z", "+00:00"))


def releases(comp: Component) -> list[dict]:
    """Published releases of the component with a plain X.Y.Z version, newest first."""
    rows = gh_json([
        "release", "list", "--repo", REPO, "--limit", "300",
        "--json", "tagName,isDraft,isPrerelease,publishedAt",
    ]) or []
    out = []
    for r in rows:
        tag = r["tagName"]
        if r["isDraft"] or not tag.startswith(comp.tag_prefix):
            continue
        version = tag[len(comp.tag_prefix):]
        if STRICT_VERSION.match(version) and r.get("publishedAt"):
            out.append({**r, "version": version})
    return sorted(out, key=lambda r: vkey(r["version"]), reverse=True)


def wheel_names(comp: Component, version: str, assets: set[str]) -> list[str] | None:
    """The wheels to upload, dependencies first, or None if the set is incomplete."""
    names = []
    for project, prefix, tags in comp.projects:
        want = [f"{prefix}-{version}-{t}.whl" for t in tags]
        thin = f"{prefix}-{version}-py3-none-any.whl"
        if all(w in assets for w in want):
            names += want
        elif project == "cua-cli" and thin in assets:
            names.append(thin)
        else:
            return None
    return names


def green(comp: Component, tag: str) -> bool:
    runs = gh_json([
        "api", f"repos/{REPO}/actions/workflows/{comp.workflow}/runs?branch={tag}&status=success&per_page=1",
    ])
    return bool(runs and runs.get("total_count"))


def assets_of(tag: str) -> set[str]:
    rel = gh_json(["release", "view", tag, "--repo", REPO, "--json", "assets"])
    return {a["name"] for a in rel["assets"]}


def promotable(comp: Component, rel: dict, *, require_green: bool) -> str | None:
    """None if the release can be promoted, else why not."""
    assets = assets_of(rel["tagName"])
    if comp.checksums not in assets:
        return f"no {comp.checksums}"
    if wheel_names(comp, rel["version"], assets) is None:
        return "wheel set incomplete"
    if require_green and not green(comp, rel["tagName"]):
        return f"no successful {comp.workflow} run for the tag"
    return None


def pypi_state(comp: Component) -> tuple[str | None, dt.datetime | None, set[str]]:
    """(newest version, its upload time, all versions) of the component's main project."""
    project = comp.projects[-1][0]
    doc = pypi(project)
    if not doc:
        return None, None, set()
    have = {v for v, fs in doc["releases"].items() if fs}
    strict = sorted((v for v in have if STRICT_VERSION.match(v)), key=vkey)
    if not strict:
        return None, None, have
    newest = strict[-1]
    up = min(ts(f["upload_time_iso_8601"]) for f in doc["releases"][newest])
    return newest, up, have


def plan(args) -> dict[str, str]:
    comp = COMPONENTS[args.component]
    now = dt.datetime.now(dt.timezone.utc)
    rels = releases(comp)
    pypi_version, pypi_uploaded, have = pypi_state(comp)
    out = {"component": comp.name, "pypi_version": pypi_version or "", "stale": "false"}

    target, notes = None, []
    if args.version:
        match = [r for r in rels if r["version"] == args.version]
        if not match:
            return {**out, "action": "none", "reason": f"no published release {comp.tag_prefix}{args.version}"}
        why = promotable(comp, match[0], require_green=args.require_green)
        if why:
            return {**out, "action": "none", "reason": f"{match[0]['tagName']}: {why}"}
        target = match[0]
    else:
        for r in rels:
            if now - ts(r["publishedAt"]) < dt.timedelta(hours=args.min_age_hours):
                notes.append(f"{r['tagName']}: younger than {args.min_age_hours}h")
                continue
            why = promotable(comp, r, require_green=args.require_green)
            if why is None:
                target = r
                break
            notes.append(f"{r['tagName']}: {why}")
            if len(notes) >= 25:
                break

    if target is None:
        reason = "no promotable build" + (f" ({'; '.join(notes[:5])})" if notes else "")
        # Builds keep shipping to GitHub but none qualifies for PyPI.
        newer = [r for r in rels if not pypi_version or vkey(r["version"]) > vkey(pypi_version)]
        if pypi_uploaded and newer and now - pypi_uploaded > dt.timedelta(days=args.max_stale_days):
            out["stale"] = "true"
        return {**out, "action": "none", "reason": reason}

    out.update(tag=target["tagName"], version=target["version"])
    # PyPI more than max-stale-days behind the newest promotable build.
    if pypi_uploaded and target["version"] not in have and pypi_version and vkey(target["version"]) > vkey(pypi_version):
        lag = ts(target["publishedAt"]) - pypi_uploaded
        if lag > dt.timedelta(days=args.max_stale_days):
            out["stale"] = "true"
    if target["version"] in have:
        return {**out, "action": "skip", "reason": f"{target['version']} is already on PyPI"}
    if not args.version and pypi_version and vkey(target["version"]) < vkey(pypi_version):
        return {**out, "action": "skip", "reason": f"{target['version']} is older than PyPI's {pypi_version}"}
    return {**out, "action": "promote", "reason": f"newest promotable build {target['tagName']}"}


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def parse_checksums(text: str) -> dict[str, str]:
    sums = {}
    for line in text.splitlines():
        parts = line.split()
        if len(parts) >= 2 and re.fullmatch(r"[0-9a-f]{64}", parts[0]):
            sums[os.path.basename(parts[-1].lstrip("*"))] = parts[0]
    return sums


def wheel_requires(path: Path) -> list[str]:
    with zipfile.ZipFile(path) as z:
        meta = z.read(next(n for n in z.namelist() if n.endswith(".dist-info/METADATA"))).decode()
    return [line.split(":", 1)[1].strip() for line in meta.splitlines() if line.startswith("Requires-Dist:")]


def verify_thin_cli(path: Path, sums: dict[str, str]) -> None:
    """A thin cua-cli wheel names the CLI archives it downloads; each must be
    an asset of this release with the same sha256."""
    with zipfile.ZipFile(path) as z:
        name = next((n for n in z.namelist() if n.endswith("/_release.json")), None)
        if name is None:
            raise SystemExit(f"{path.name}: a py3-none-any cua-cli wheel without cua_cli/_release.json")
        manifest = json.loads(z.read(name))
    for platform, asset in manifest["assets"].items():
        if sums.get(asset["name"]) != asset["sha256"]:
            raise SystemExit(f"{path.name}: {platform} archive {asset['name']} does not match the release checksums")


def fetch(args) -> None:
    comp = COMPONENTS[args.component]
    tag = args.tag
    if not tag.startswith(comp.tag_prefix):
        raise SystemExit(f"{tag} is not a {comp.name} tag")
    version = tag[len(comp.tag_prefix):]
    dest = Path(args.dir)
    dest.mkdir(parents=True, exist_ok=True)
    names = wheel_names(comp, version, assets_of(tag))
    if names is None:
        raise SystemExit(f"{tag}: wheel set incomplete")
    patterns = [p for n in [comp.checksums, *names] for p in ("--pattern", n)]
    subprocess.run(["gh", "release", "download", tag, "--repo", REPO, "--dir", str(dest), "--clobber", *patterns], check=True)
    sums = parse_checksums((dest / comp.checksums).read_text())
    for n in names:
        if sums.get(n) != sha256(dest / n):
            raise SystemExit(f"{n}: sha256 does not match {comp.checksums}")
        if n.startswith("cua_cli-") and n.endswith("-py3-none-any.whl"):
            verify_thin_cli(dest / n, sums)
        if n.startswith(f"cua-{version}-"):
            pins = [r for r in wheel_requires(dest / n) if r.replace(" ", "").startswith("cua-cli")]
            if pins != [f"cua-cli=={version}"]:
                raise SystemExit(f"{n}: requires {pins}, expected cua-cli=={version}")
    (dest / "upload-order.txt").write_text("".join(f"{dest / n}\n" for n in names))
    print(f"{tag}: {len(names)} wheel(s) verified")


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("plan")
    p.add_argument("--component", choices=sorted(COMPONENTS), required=True)
    p.add_argument("--version", default="")
    p.add_argument("--min-age-hours", type=int, default=24)
    p.add_argument("--max-stale-days", type=int, default=14)
    p.add_argument("--require-green", type=lambda s: s.lower() != "false", default=True)
    f = sub.add_parser("fetch")
    f.add_argument("--component", choices=sorted(COMPONENTS), required=True)
    f.add_argument("--tag", required=True)
    f.add_argument("--dir", required=True)
    args = ap.parse_args(argv)
    if args.cmd == "fetch":
        fetch(args)
        return 0
    out = plan(args)
    for k, v in out.items():
        print(f"{k}={v}")
    if os.environ.get("GITHUB_OUTPUT"):
        with open(os.environ["GITHUB_OUTPUT"], "a") as fh:
            fh.writelines(f"{k}={v}\n" for k, v in out.items())
    return 0


if __name__ == "__main__":
    sys.exit(main())
