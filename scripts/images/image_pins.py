#!/usr/bin/env python3
"""Which catalog images run a given cua-spacesd, and moving their pins.

    image_pins.py status [--version X.Y.Z] [--json]
    image_pins.py apply  [--version X.Y.Z] [--body FILE] [--run-url URL]
                         [--results linux=success,windows=failure,...]
    image_pins.py lag    [--version X.Y.Z]

The version defaults to libs/cua-spacesd/VERSION. An OS is the catalog
entries (libs/images/sandbox-images.json, canonical and distro groups) of one
repository that run cua-spacesd and carry a ``digest``: ghcr.io/trycua/linux
(4 tags), windows, macos, omarchy. Benchmark images pin through
cd-bench-images.yml's own catalog PR and are not handled here.

A tag "reports" a version when every platform child of what it resolves to
that has a passing doctor report attached (an OCI referrer of artifact type
application/vnd.cua.doctor.report.v1+json, found with ``oras discover``) has
its newest passing report annotated ``ai.cua.doctor.spacesd=<version>``, and
at least one child has one. Children without a report (a hosted arm64 disk
that only boot-smoked) do not count either way.

Per OS, against the version:

* pinned   every floating tag reports it, and the catalog already holds the
           floating digests
* ready    every floating tag reports it, and some catalog digest differs: a
           new digest was really pushed
* pending  some floating tag does not report it yet

status   prints that table (or JSON).
apply    moves every ready OS: ``record-image-sizes.py --refresh-digest`` for
         its tags (new digest and sizes), checks the catalog now holds exactly
         the digests that were verified, and moves the dated pin named in the
         libs/images/README.md table row of each tag that has one. Writes the
         pins PR body (a per-OS checklist) to --body and prints a JSON summary
         (the only line on stdout).
         Exits 1 when an OS whose CI build (--results) succeeded still does
         not report the version: a green run that pushed nothing.
lag      the CI check: every catalog digest that does not report the version
         becomes a ``::warning::`` (and a step summary table). Never fails.

Needs ``crane`` and ``oras`` on PATH (anonymous pulls of the public repos).
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field
from typing import Callable

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
CATALOG = os.path.join(ROOT, "libs", "images", "sandbox-images.json")
README = os.path.join(ROOT, "libs", "images", "README.md")
RECORD_SIZES = os.path.join(ROOT, "scripts", "images", "record-image-sizes.py")
VERSION_FILE = os.path.join(ROOT, "libs", "cua-spacesd", "VERSION")
REPORT_TYPE = "application/vnd.cua.doctor.report.v1+json"
INDEX_TYPES = {
    "application/vnd.oci.image.index.v1+json",
    "application/vnd.docker.distribution.manifest.list.v2+json",
}
PIN_RE = re.compile(r"pin `([^`]+)`")

# How each OS gets onto a new cua-spacesd: (label, what builds it).
OSES = {
    "linux": ("Linux", "cd-image-linux.yml"),
    "windows": ("Windows", "cd-image-windows.yml"),
    "omarchy": ("Omarchy", "cd-image-omarchy.yml"),
    "macos": ("macOS", None),  # Lume needs Apple silicon: scripts/images/release-macos.sh
}


def macos_command(version: str) -> str:
    return f"scripts/images/release-macos.sh {version}"


class Registry:
    """crane and oras, anonymous."""

    def _run(self, *cmd: str) -> str:
        return subprocess.run(cmd, check=True, capture_output=True, text=True).stdout

    def digest(self, ref: str) -> str:
        return self._run("crane", "digest", ref).strip()

    def manifest(self, ref: str) -> dict:
        return json.loads(self._run("crane", "manifest", ref))

    def tags(self, repo: str) -> list[str]:
        return self._run("crane", "ls", repo).split()

    def reports(self, ref: str) -> list[dict]:
        """Annotations of every doctor report attached to ref."""
        out = json.loads(self._run("oras", "discover", "--format", "json", "--artifact-type", REPORT_TYPE, ref))
        found: list[dict] = []

        def walk(node: object) -> None:
            if isinstance(node, dict):
                ann = node.get("annotations")
                if node.get("artifactType") == REPORT_TYPE and isinstance(ann, dict):
                    found.append(ann)
                for v in node.values():
                    walk(v)
            elif isinstance(node, list):
                for v in node:
                    walk(v)

        walk(out)
        return found


def repo_of(ref: str) -> str:
    return ref.rsplit(":", 1)[0]


def os_key(ref: str) -> str:
    return repo_of(ref).rsplit("/", 1)[-1]


def spacesd_of(registry: Registry, repo: str, digest: str) -> str | None:
    """The cua-spacesd version repo@digest reports; None when no child has a
    passing report, ``mixed:a,b`` when its children disagree."""
    manifest = registry.manifest(f"{repo}@{digest}")
    if manifest.get("mediaType") in INDEX_TYPES or "manifests" in manifest:
        children = [
            m["digest"]
            for m in manifest.get("manifests", [])
            if (m.get("platform") or {}).get("architecture") not in (None, "unknown")
        ]
    else:
        children = [digest]
    versions = set()
    for child in children:
        passing = [
            a for a in registry.reports(f"{repo}@{child}")
            if a.get("ai.cua.doctor.status") == "pass" and a.get("ai.cua.doctor.spacesd")
        ]
        if passing:
            newest = max(passing, key=lambda a: a.get("org.opencontainers.image.created", ""))
            versions.add(newest["ai.cua.doctor.spacesd"])
    if not versions:
        return None
    if len(versions) == 1:
        return versions.pop()
    return "mixed:" + ",".join(sorted(versions))


@dataclass
class Tag:
    ref: str
    catalog: str  # digest main pins
    floating: str = ""  # digest the tag resolves to now
    floating_version: str | None = None
    catalog_version: str | None = None


@dataclass
class OS:
    key: str
    tags: list[Tag] = field(default_factory=list)
    status: str = ""  # pinned | ready | pending

    @property
    def label(self) -> str:
        return OSES.get(self.key, (self.key, None))[0]


def load_catalog(path: str = CATALOG) -> dict:
    with open(path) as fh:
        return json.load(fh)


def catalog_oses(catalog: dict) -> list[OS]:
    oses: dict[str, OS] = {}
    for img in catalog["images"]:
        if img.get("group") not in ("canonical", "distro") or not img.get("spacesd") or not img.get("digest"):
            continue
        key = os_key(img["ref"])
        oses.setdefault(key, OS(key)).tags.append(Tag(img["ref"], img["digest"]))
    order = list(OSES)
    return sorted(oses.values(), key=lambda o: (order.index(o.key) if o.key in order else len(order), o.key))


def resolve(registry: Registry, oses: list[OS], version: str, *, floating: bool = True, catalog: bool = False) -> None:
    """Fill in digests and versions (in parallel) and each OS's status."""
    tags = [t for o in oses for t in o.tags]

    def one(t: Tag) -> None:
        if floating:
            t.floating = registry.digest(t.ref)
            t.floating_version = spacesd_of(registry, repo_of(t.ref), t.floating)
        if catalog:
            t.catalog_version = spacesd_of(registry, repo_of(t.ref), t.catalog)

    with ThreadPoolExecutor(max_workers=8) as pool:
        list(pool.map(one, tags))
    if not floating:
        return
    for o in oses:
        if any(t.floating_version != version for t in o.tags):
            o.status = "pending"
        elif all(t.floating == t.catalog for t in o.tags):
            o.status = "pinned"
        else:
            o.status = "ready"


def short(ref: str) -> str:
    return ref.removeprefix("ghcr.io/trycua/")


def parse_results(text: str) -> dict[str, str]:
    out = {}
    for item in filter(None, (s.strip() for s in (text or "").split(","))):
        key, _, value = item.partition("=")
        out[key.strip()] = value.strip()
    return out


def silent_pushes(oses: list[OS], results: dict[str, str]) -> list[OS]:
    """OSes whose CI build succeeded but whose tags do not report the version."""
    return [o for o in oses if o.status == "pending" and results.get(o.key) == "success"]


def body(oses: list[OS], version: str, *, run_url: str = "", results: dict[str, str] | None = None) -> str:
    results = results or {}
    done = sum(o.status in ("pinned", "ready") for o in oses)
    lines = [
        f"Moves the catalog pins of every Space image that runs **cua-spacesd {version}**. "
        f"**{done} of {len(oses)}** OS images are on {version}.",
        "",
    ]
    for o in oses:
        tags = ", ".join(f"`{short(t.ref)}`" for t in o.tags)
        if o.status == "ready":
            lines.append(f"- [x] **{o.label}** ({tags}): pinned by this PR")
        elif o.status == "pinned":
            lines.append(f"- [x] **{o.label}** ({tags}): already pinned on main")
        else:
            seen = sorted({t.floating_version or "no doctor report" for t in o.tags})
            if o.key == "macos":
                todo = f"on an Apple silicon Mac, run `{macos_command(version)}`"
            else:
                wf = OSES.get(o.key, (o.key, None))[1]
                result = results.get(o.key)
                if result == "success":
                    todo = f"**its CI build passed but pushed nothing that reports {version}**; check {wf}"
                elif result in ("failure", "cancelled"):
                    todo = f"its CI build ended `{result}`: re-run the failed jobs of the run below"
                elif wf:
                    todo = f"waiting for `{wf}`"
                else:
                    todo = "no automated build"
            lines.append(f"- [ ] **{o.label}** ({tags}): pending, the floating tags report {', '.join(seen)}; {todo}")
    moving = [t for o in oses if o.status == "ready" for t in o.tags]
    if moving:
        lines += ["", "| Tag | Digest | cua-spacesd |", "|---|---|---|"]
        lines += [f"| `{short(t.ref)}` | `{t.floating}` | {t.floating_version} |" for t in moving]
    lines += [
        "",
        "A tag counts as on the version when the newest passing doctor report attached to each of its "
        "platform children (an OCI referrer) says so: a green run that pushed nothing cannot move a pin. "
        "Sizes are from `record-image-sizes.py --refresh-digest`, the app-core parity goldens from "
        "`UPDATE_PARITY=1`, the docs pages generated from the catalog from their docs generators "
        "(`scripts/docs-generators`), and README pin rows follow their tags.",
        "",
        "This PR is regenerated from `main` and the registry on every run of "
        "`.github/workflows/cd-images-spacesd-release.yml` (each cua-spacesd release, and after "
        f"`{macos_command(version)}`): do not push to `images/pins` by hand. Merge it whenever the "
        "checks pass; OSes still pending land in the next PR.",
    ]
    if run_url:
        lines += ["", f"Run: {run_url}"]
    return "\n".join(lines) + "\n"


def pin_tag(registry: Registry, ref: str, digest: str, *, look: int = 12) -> str | None:
    """The newest dated pin <tag>-<yyyymmdd>-<sha7> of ref's repo at digest."""
    repo, tag = ref.rsplit(":", 1)
    pat = re.compile(rf"^{re.escape(tag)}-(\d{{8}})-[0-9a-f]{{7}}$")
    pins = sorted((t for t in registry.tags(repo) if pat.match(t)), key=lambda t: pat.match(t).group(1), reverse=True)
    for t in pins[:look]:
        if registry.digest(f"{repo}:{t}") == digest:
            return t
    return None


def update_readme(text: str, ref: str, pin: str) -> str:
    """Move the `pin` of the README table row whose first cell is exactly ref."""
    row = re.compile(rf"^(\| `{re.escape(ref)}` \|.*)$", re.M)
    return row.sub(lambda m: PIN_RE.sub(f"pin `{pin}`", m.group(1)), text)


def refresh_sizes(refs: list[str], script: str = RECORD_SIZES) -> None:
    cmd = [sys.executable, script, "--refresh-digest"]
    for r in refs:
        cmd += ["--ref", r]
    # apply's stdout is the one-line JSON summary pins-pr.sh parses: the
    # child's progress lines go to stderr.
    proc = subprocess.run(cmd, check=True, stdout=subprocess.PIPE, text=True)
    sys.stderr.write(proc.stdout)


def apply(
    registry: Registry,
    oses: list[OS],
    *,
    refresh: Callable[[list[str]], None] = refresh_sizes,
    catalog_path: str = CATALOG,
    readme_path: str = README,
) -> list[str]:
    """Move every ready OS's pins; returns the refs moved."""
    ready = [t for o in oses if o.status == "ready" for t in o.tags]
    if not ready:
        return []
    refresh([t.ref for t in ready])
    now = {i["ref"]: i.get("digest") for i in load_catalog(catalog_path)["images"]}
    moved = [t for t in ready if now.get(t.ref) != t.floating]
    if moved:
        # The tag moved again between the check and the refresh: never pin
        # bytes that were not verified.
        raise SystemExit(
            "image_pins: the catalog now holds digests other than the verified ones: "
            + ", ".join(f"{t.ref} {now.get(t.ref)} (verified {t.floating})" for t in moved)
        )
    with open(readme_path) as fh:
        text = fh.read()
    new = text
    for t in ready:
        if re.search(rf"^\| `{re.escape(t.ref)}` \|.*pin `", new, re.M):
            pin = pin_tag(registry, t.ref, t.floating)
            if pin is None:
                raise SystemExit(f"image_pins: no dated pin of {t.ref} at {t.floating} for the README")
            new = update_readme(new, t.ref, pin)
    if new != text:
        with open(readme_path, "w") as fh:
            fh.write(new)
    return [t.ref for t in ready]


def lag(registry: Registry, oses: list[OS], version: str) -> list[Tag]:
    resolve(registry, oses, version, floating=False, catalog=True)
    behind = [t for o in oses for t in o.tags if t.catalog_version != version]
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    lines = [f"### Catalog images on cua-spacesd {version}", "", "| Tag | Catalog digest reports |", "|---|---|"]
    for o in oses:
        for t in o.tags:
            mark = "" if t.catalog_version == version else " (behind)"
            lines.append(f"| `{short(t.ref)}` | {t.catalog_version or 'no doctor report'}{mark} |")
    if behind:
        lines += [
            "",
            f"{len(behind)} catalog tags are not on cua-spacesd {version} yet. This is a warning, not a failure: "
            "images follow a cua-spacesd release by a few hours (`cd-images-spacesd-release.yml` rebuilds them "
            "and opens the `images/pins` PR; macOS needs `scripts/images/release-macos.sh`).",
        ]
        for o in oses:
            late = [t for t in o.tags if t in behind]
            if late:
                seen = ", ".join(sorted({t.catalog_version or "no doctor report" for t in late}))
                print(
                    f"::warning title={o.label} images behind cua-spacesd {version}::"
                    f"{', '.join(short(t.ref) for t in late)} are pinned at builds that report {seen}"
                )
    else:
        print(f"every catalog image reports cua-spacesd {version}")
    if summary:
        with open(summary, "a") as fh:
            fh.write("\n".join(lines) + "\n")
    else:
        print("\n".join(lines))
    return behind


def main(argv: list[str] | None = None, registry: Registry | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("command", choices=["status", "apply", "lag"])
    ap.add_argument("--version", help="cua-spacesd X.Y.Z (default libs/cua-spacesd/VERSION)")
    ap.add_argument("--json", action="store_true")
    ap.add_argument("--body", help="apply: write the pins PR body here")
    ap.add_argument("--run-url", default="")
    ap.add_argument("--results", default="", help="apply: CI build results, os=success|failure|cancelled|skipped,...")
    a = ap.parse_args(argv)
    version = (a.version or open(VERSION_FILE).read()).strip().removeprefix("cua-spacesd-v").removeprefix("v")
    if not re.fullmatch(r"\d+\.\d+\.\d+", version):
        ap.error(f"--version must be X.Y.Z, got {version!r}")
    registry = registry or Registry()
    oses = catalog_oses(load_catalog())

    if a.command == "lag":
        lag(registry, oses, version)
        return 0

    resolve(registry, oses, version)
    summary = {s: [o.key for o in oses if o.status == s] for s in ("ready", "pinned", "pending")}
    if a.command == "status":
        if a.json:
            print(json.dumps({"version": version, **summary}))
        else:
            for o in oses:
                for t in o.tags:
                    print(f"{o.status:8} {short(t.ref):28} {t.floating_version or '-':10} {t.floating}")
        return 0

    results = parse_results(a.results)
    summary["moved"] = apply(registry, oses)
    if a.body:
        with open(a.body, "w") as fh:
            fh.write(body(oses, version, run_url=a.run_url, results=results))
    silent = silent_pushes(oses, results)
    summary["silent"] = [o.key for o in silent]
    print(json.dumps({"version": version, **summary}))
    for o in silent:
        print(
            f"::error::{OSES[o.key][1]} succeeded but {', '.join(t.ref for t in o.tags)} "
            f"does not report cua-spacesd {version}: the run pushed nothing new",
            file=sys.stderr,
        )
    return 1 if silent else 0


if __name__ == "__main__":
    sys.exit(main())
