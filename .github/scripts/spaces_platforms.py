#!/usr/bin/env python3
"""Per-platform patch releases for Cua Spaces (CUA-2068, design trycua/cua#5704).

Two subcommands, both used by .github/workflows/cd-cua-spaces.yml.

plan   Decide which platforms (macos, windows, linux) a stable release builds
       and publishes, from the paths changed since the previous published
       stable release tag. Prints key=value lines (--github-output also appends them to $GITHUB_OUTPUT).

         spaces_platforms.py plan --previous cua-spaces-v0.13.0 [--head HEAD]
         spaces_platforms.py plan --manual windows,linux --reason "..."
         spaces_platforms.py plan --force-all "prerelease"

       The default is ALL platforms. A subset is chosen only when every changed
       path is classified: shared code, an unknown path, no previous tag, a
       prerelease, a Sparkle cutover or any doubt means all.

notes  Print the release-notes paragraph that says which platforms a release
       contains and which version each skipped platform stays on.

         spaces_platforms.py notes --platforms macos --manifest release-artifacts.json

Path rules (first match wins; see classify()):
  neutral    docs, Markdown, desktop tests, and the version-only edits that
             Release Please makes to its own files. They never pick a platform.
  macos      apps/cua-spaces-macos/**, and the Electron macOS signing and
             Sparkle files.
  windows    the NSIS installer script.
  linux      the Linux packaging files (prerm, icons, tray, permissions, polkit).
  all        anything else: libs/cua/**, the renderer and main process in
             apps/cua-spaces-desktop/src/**, the web UI, the cua-spacesd pin,
             the release workflow and every path not listed above.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from pathlib import Path

OSES = ("macos", "windows", "linux")
ALL, NEUTRAL = "all", "neutral"

# Files Release Please edits on a release PR. Neutral only when the diff is a
# version bump (see version_only). The cua-spacesd pin (libs/cua-spacesd/VERSION)
# is deliberately not here: it changes what every platform bundles.
RELEASE_BUMP_FILES = {
    ".release-please-manifest.json",
    "apps/cua-spaces-macos/VERSION",
    "apps/cua-spaces-macos/Support/Info.plist",
    "apps/cua-spaces-desktop/package.json",
    "apps/cua-spaces/package.json",
    "apps/cua-spaces/src-tauri/tauri.conf.json",
    "apps/cua-spaces/src-tauri/Cargo.toml",
    "apps/cua-spaces/src-tauri/Cargo.lock",
}

D = "apps/cua-spaces-desktop/"
NEUTRAL_PATTERNS = (
    r".*\.mdx?$",
    r"docs/",
    D + r"docs/",
    D + r"test/",
    r"apps/cua-spaces-macos/(Tests|UITests)/",
)
MACOS_PATTERNS = (
    r"apps/cua-spaces-macos/",
    D + r"packaging/entitlements\.mac[^/]*\.plist$",
    D + r"packaging/sign-mac\.cjs$",
    D + r"scripts/sparkle/",
)
WINDOWS_PATTERNS = (D + r"packaging/installer\.nsh$",)
LINUX_PATTERNS = (
    D + r"packaging/linux/",
    D + r"packaging/linux-icons/",
    D + r"packaging/linux-tray/",
    D + r"packaging/linux-permissions\.cjs$",
    D + r"packaging/ai\.cua\.spaces\.policy$",
)

# A changed line that is only a version number: `"version": "1.2.3",`,
# `<string>1.2.3</string>`, `1.2.3`, a manifest entry `"path": "1.2.3"`,
# a Cargo `version = "1.2.3"`.
VERSION_LINE = re.compile(
    r'^[+-]\s*(?:"[\w@./-]+"\s*[:=]\s*|version\s*=\s*)?"?(?:<string>)?'
    r"\d+\.\d+\.\d+[-+.0-9A-Za-z]*(?:</string>)?\"?,?\s*$"
)


def version_only_diff(diff: str) -> bool:
    """True when every changed line of a unified diff is a version number."""
    changed = [
        line for line in diff.splitlines()
        if line[:1] in "+-" and not line.startswith(("+++", "---"))
    ]
    return bool(changed) and all(VERSION_LINE.match(line) for line in changed)


def classify(path: str, version_only=lambda p: False) -> str:
    """One of OSES, NEUTRAL or ALL for a changed repo-relative path."""
    for pattern in NEUTRAL_PATTERNS:
        if re.match(pattern, path):
            return NEUTRAL
    if path in RELEASE_BUMP_FILES and version_only(path):
        return NEUTRAL
    for os_name, patterns in (("macos", MACOS_PATTERNS), ("windows", WINDOWS_PATTERNS), ("linux", LINUX_PATTERNS)):
        if any(re.match(p, path) for p in patterns):
            return os_name
    return ALL


def decide(paths: list[str], version_only=lambda p: False) -> tuple[list[str], str]:
    """(platforms, reason) for the changed paths. Doubt means all three."""
    everything = list(OSES)
    if not paths:
        return everything, "no changed paths found: all platforms"
    picked: set[str] = set()
    for path in paths:
        kind = classify(path, version_only)
        if kind == ALL:
            return everything, f"{path} is shared or not classified: all platforms"
        if kind != NEUTRAL:
            picked.add(kind)
    if not picked:
        return everything, "only documentation or version bumps changed: all platforms (nothing to narrow on)"
    chosen = [o for o in OSES if o in picked]
    return chosen, "only " + ", ".join(chosen) + " paths changed"


def parse_platforms(value: str) -> list[str]:
    names = [n.strip().lower() for n in value.split(",") if n.strip()]
    if not names or names == ["all"]:
        return list(OSES)
    bad = [n for n in names if n not in OSES]
    if bad:
        raise SystemExit(f"bad platforms {bad}: use all or a comma list of {', '.join(OSES)}")
    return [o for o in OSES if o in names]


def git(*args: str) -> str:
    return subprocess.run(["git", *args], check=True, capture_output=True, text=True).stdout


def changed_paths(previous: str, head: str) -> list[str]:
    return [p for p in git("diff", "--name-only", f"{previous}...{head}").splitlines() if p]


def plan(args: argparse.Namespace) -> dict:
    if args.manual and args.manual.strip().lower() != "all":
        platforms = parse_platforms(args.manual)
        if not (args.reason or "").strip():
            raise SystemExit("a manual platforms list needs a reason (platforms_reason)")
        reason = f"manual: {args.reason.strip()}"
    elif args.force_all:
        platforms, reason = list(OSES), f"all platforms: {args.force_all}"
    elif not args.previous:
        platforms, reason = list(OSES), "no previous stable release to compare with: all platforms"
    else:
        try:
            paths = changed_paths(args.previous, args.head)
            platforms, reason = decide(
                paths, lambda p: version_only_diff(git("diff", "-U0", f"{args.previous}...{args.head}", "--", p))
            )
        except subprocess.CalledProcessError as error:
            platforms, reason = list(OSES), f"could not diff against {args.previous} ({error.returncode}): all platforms"
    return {
        **{o: str(o in platforms).lower() for o in OSES},
        "platforms": ",".join(platforms),
        "partial": str(len(platforms) < len(OSES)).lower(),
        "reason": reason,
    }


def manifest_versions(manifest: dict, component: str = "app") -> dict[str, str]:
    """OS -> the version install.sh / install.ps1 would install there."""
    found: dict[str, str] = {}
    for artifact in manifest.get("artifacts", []):
        if artifact.get("component") != component:
            continue
        prefix = artifact.get("platform", "").split("-")[0]
        os_name = {"darwin": "macos", "windows": "windows", "linux": "linux"}.get(prefix)
        if os_name:
            found.setdefault(os_name, artifact.get("version", ""))
    return found


LABEL = {"macos": "macOS", "windows": "Windows", "linux": "Linux"}


def join_names(names: list[str]) -> str:
    labels = [LABEL[n] for n in names]
    return labels[0] if len(labels) == 1 else ", ".join(labels[:-1]) + " and " + labels[-1]


def notes(platforms: list[str], manifest: dict) -> str:
    skipped = [o for o in OSES if o not in platforms]
    if not skipped:
        return ""
    versions = manifest_versions(manifest)
    lines = [f"**Platforms in this release:** {join_names(platforms)} only."]
    detail = []
    for o in skipped:
        detail.append(f"{LABEL[o]} stays on {versions[o]}" if versions.get(o) else f"{LABEL[o]} is unchanged")
    lines.append(
        "; ".join(detail)
        + ". Those installers and update feeds still point at the earlier build, so "
        + join_names(skipped)
        + " users are not offered this update."
    )
    return "\n\n".join(lines) + "\n"


def write_outputs(outputs: dict, github_output: bool) -> None:
    lines = "".join(f"{k}={v}\n" for k, v in outputs.items())
    target = os.environ.get("GITHUB_OUTPUT")
    if github_output and target:
        with open(target, "a") as handle:
            handle.write(lines)
    sys.stdout.write(lines)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    p = sub.add_parser("plan")
    p.add_argument("--previous", default="", help="previous published stable release tag")
    p.add_argument("--head", default="HEAD")
    p.add_argument("--manual", default="", help="manual platforms input (all or a comma list)")
    p.add_argument("--reason", default="", help="why, for a manual list")
    p.add_argument("--force-all", default="", help="build everything, with this reason")
    p.add_argument("--github-output", action="store_true", help="also append the outputs to $GITHUB_OUTPUT")
    n = sub.add_parser("notes")
    n.add_argument("--platforms", required=True)
    n.add_argument("--manifest", required=True)
    args = parser.parse_args(argv)
    if args.command == "plan":
        write_outputs(plan(args), args.github_output)
    else:
        sys.stdout.write(notes(parse_platforms(args.platforms), json.loads(Path(args.manifest).read_text())))
    return 0


if __name__ == "__main__":
    sys.exit(main())
