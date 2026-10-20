#!/usr/bin/env python3
"""Fail if any tracked lockfile or manifest is below the versions that distrust
the compromised eMudhra intermediates (reported 2026-10-17, CUA-1948).

Checks, over `git ls-files`:
  - Cargo.lock: rustls-webpki >= 0.103.16 (rejects paths through the eight
    compromised intermediate keys); webpki-roots 1.x >= 1.0.10 (drops emSign
    ECC Root CA - G3). webpki-roots 0.26.x has no fix upstream; the webpki
    check covers it (see libs/cua/crates/cua-spacesd-client/tests/emudhra_distrust.rs).
  - uv.lock / requirements*.txt pins: certifi >= 2026.10.19.
  - package.json / package-lock.json / pnpm-lock.yaml: Electron >= 42.11.16,
    43.8.1, 44.9.1 on those lines, any later major.
  - Dockerfiles: any that install ca-certificates carry the eMudhra distrust block.
  - go.mod: toolchain (or go) directive >= go1.26.10 on 1.26, >= go1.27.3 on 1.27+.
Offline and fast. Exit 1 listing every offender.
"""
import json
import re
import subprocess
import sys
from pathlib import Path

MIN_WEBPKI = (0, 103, 16)
MIN_WEBPKI_ROOTS_1 = (1, 0, 10)
MIN_CERTIFI = (2026, 10, 19)
MIN_ELECTRON = {42: (42, 11, 16), 43: (43, 8, 1), 44: (44, 9, 1)}
MIN_GO = {26: (1, 26, 10), 27: (1, 27, 3)}


def ver(s):
    m = re.match(r"v?(\d+)(?:\.(\d+))?(?:\.(\d+))?", s.strip())
    return tuple(int(x or 0) for x in m.groups()) if m else None


def lock_packages(text):
    """Yield (name, version) for [[package]] tables of a Cargo.lock / uv.lock."""
    for m in re.finditer(r'^name = "([^"]+)"\nversion = "([^"]+)"', text, re.M):
        yield m.group(1), m.group(2)


def check_cargo_lock(text):
    bad = []
    for name, v in lock_packages(text):
        t = ver(v)
        if name == "rustls-webpki" and t < MIN_WEBPKI:
            bad.append(f"rustls-webpki {v} < 0.103.16")
        if name == "webpki-roots" and t[0] == 1 and t < MIN_WEBPKI_ROOTS_1:
            bad.append(f"webpki-roots {v} < 1.0.10")
    return bad


def check_uv_lock(text):
    return [f"certifi {v} < 2026.10.19" for n, v in lock_packages(text) if n == "certifi" and ver(v) < MIN_CERTIFI]


def check_requirements(text):
    bad = []
    for line in text.splitlines():
        m = re.match(r"\s*certifi\s*(==|<=|<|~=)\s*([0-9.]+)", line, re.I)
        if m and ver(m.group(2)) < MIN_CERTIFI:
            bad.append(f"certifi {m.group(1)}{m.group(2)} < 2026.10.19")
    return bad


def electron_ok(v):
    t = ver(v)
    if t is None:
        return True
    if t[0] > max(MIN_ELECTRON):
        return True
    return t[0] in MIN_ELECTRON and t >= MIN_ELECTRON[t[0]]


def check_package_json(text):
    bad = []
    try:
        d = json.loads(text)
    except ValueError:
        return bad
    for sect in ("dependencies", "devDependencies", "optionalDependencies"):
        spec = (d.get(sect) or {}).get("electron")
        if spec and not electron_ok(spec.lstrip("^~=v")):
            bad.append(f"electron {spec} is not on a fixed line (42.11.16, 43.8.1, 44.9.1)")
    return bad


def check_package_lock(text):
    bad = []
    try:
        d = json.loads(text)
    except ValueError:
        return bad
    for k, p in (d.get("packages") or {}).items():
        if k.endswith("node_modules/electron") and not electron_ok(p.get("version", "")):
            bad.append(f"locked electron {p.get('version')} is not on a fixed line")
    return bad


def check_pnpm_lock(text):
    return [f"locked electron {m} is not on a fixed line" for m in re.findall(r"^  electron@(\d[^:(]*)[:(]", text, re.M) if not electron_ok(m)]


def check_go_mod(text):
    tc = re.search(r"^toolchain go(\S+)", text, re.M)
    go = re.search(r"^go (\S+)", text, re.M)
    v = ver((tc or go).group(1)) if (tc or go) else None
    if v is None:
        return []
    if len(v) == 3 and v[0] == 1:
        need = MIN_GO.get(v[1])
        if (need is None and v[1] < 26) or (need and v < need):
            return [f"Go {'.'.join(map(str, v))} is below 1.26.10 / 1.27.3"]
    return []


def check_dockerfile(text):
    """A Dockerfile that installs ca-certificates must also take the fixed package
    and disable the two roots (Debian has no fix yet); see the CUA-1948 block."""
    if re.search(r"^\s*(RUN|\s).*ca-certificates", text, re.M) and "emSign_ECC_Root_CA_-_G3" not in text \
            and "distrust-emudhra" not in text:
        return ["installs ca-certificates without the CUA-1948 eMudhra distrust block"]
    return []


CHECKS = [
    (lambda f: re.search(r"(^|/)[^/]*(Dockerfile|\.dockerfile)[^/]*$", f) and not f.endswith("ignore"), check_dockerfile),
    (lambda f: f.endswith("Cargo.lock"), check_cargo_lock),
    (lambda f: f.endswith("uv.lock"), check_uv_lock),
    (lambda f: re.search(r"requirements[^/]*\.txt$", f), check_requirements),
    (lambda f: f.endswith("package.json"), check_package_json),
    (lambda f: f.endswith("package-lock.json"), check_package_lock),
    (lambda f: f.endswith("pnpm-lock.yaml"), check_pnpm_lock),
    (lambda f: f.endswith("go.mod"), check_go_mod),
]


# Read-only mirrors of private canonical repositories: a change here is overwritten
# by the next sync (fleet-mirror-guard), so the bump must land upstream instead.
MIRRORED = ("libs/fleet/",)

# Test harnesses that never ship and never connect to Cua endpoints. Each needs a reason.
EXEMPT = {
    # Electron 42 fails to install on the Windows E2E runners (npm install leaves no electron.exe,
    # build.ps1 aborts); the fixture is a UI-automation target, not a client. Revisit with the E2E owners.
    "libs/cua-driver/tests/fixtures/apps/cross-platform/electron/": "UI-automation fixture pinned at Electron 39.8.5",
}


def main(root="."):
    files = subprocess.run(["git", "ls-files"], cwd=root, capture_output=True, text=True, check=True).stdout.split("\n")
    failures = []
    for f in filter(None, files):
        if f.startswith(MIRRORED) or f.startswith(tuple(EXEMPT)):
            continue
        for match, fn in CHECKS:
            if match(f):
                try:
                    text = (Path(root) / f).read_text()
                except (OSError, UnicodeDecodeError):
                    continue
                failures += [f"{f}: {m}" for m in fn(text)]
    if failures:
        print("Below the eMudhra-distrust versions (CUA-1948):", *failures, sep="\n  ")
        return 1
    print("eMudhra distrust: all lockfiles and manifests are at or above the fixed versions")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1] if len(sys.argv) > 1 else "."))
