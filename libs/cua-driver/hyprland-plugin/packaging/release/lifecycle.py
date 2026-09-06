#!/usr/bin/env python3
"""Build a standalone kit and qualify package transactions in fresh ALPM roots."""

import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import platform
import re
import shlex
import subprocess
import tarfile


PACKAGE = "cua-hyprland-plugin"
MODULE = "usr/lib/cua/hyprland/cua-hyprland-plugin.so"
SOURCE = f"usr/share/{PACKAGE}/SOURCE-PROVENANCE.json"
BUILD = f"usr/share/{PACKAGE}/BUILD-PROVENANCE.json"
LICENSE = f"usr/share/licenses/{PACKAGE}/LICENSE"
PAYLOAD = {MODULE, SOURCE, BUILD, LICENSE}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def run(command, *, cwd=None, env=None, check=True):
    result = subprocess.run(command, cwd=cwd, env=env, capture_output=True,
                            text=True, errors="replace")
    if check and result.returncode:
        raise ValueError(f"{command[0]} failed ({result.returncode}):\n{result.stdout}\n{result.stderr}")
    return result


def verify_kit(kit, revision, driver_version):
    require(re.fullmatch(r"[0-9a-f]{40}", revision), "requires a full source commit SHA")
    require(re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", driver_version), "requires a stable Driver version")
    stem = f"cua-hyprland-plugin-{driver_version}-{revision}"
    expected = {"PKGBUILD", "README.md", "SOURCE-PROVENANCE.json", "lifecycle.py", f"{stem}.tar.gz"}
    checksums = {}
    for line in (kit / "SHA256SUMS").read_text().splitlines():
        checksum, name = line.split("  ")
        require(name in expected and name not in checksums, "unexpected or duplicate kit checksum entry")
        require(re.fullmatch(r"[0-9a-f]{64}", checksum), "invalid kit checksum")
        checksums[name] = checksum
    require(set(checksums) == expected, "kit checksum inventory mismatch")
    require({p.name for p in kit.iterdir()} == expected | {"SHA256SUMS"}, "use a fresh, unbuilt standalone kit")
    for name, checksum in checksums.items():
        path = kit / name
        require(path.is_file() and not path.is_symlink(), "kit requires regular files")
        require(digest(path.read_bytes()) == checksum, f"kit checksum mismatch: {name}")
    require(digest(Path(__file__).read_bytes()) == checksums["lifecycle.py"], "runner differs from committed kit")
    manifest = json.loads((kit / "SOURCE-PROVENANCE.json").read_text())
    require(manifest["source_revision"] == revision and manifest["driver_version"] == driver_version,
            "kit source revision/version mismatch")
    return manifest, checksums


def package_payload(package, manifest):
    names = run(["bsdtar", "-tf", str(package)]).stdout.splitlines()
    files = [name for name in names if not name.endswith("/")]
    require(len(names) == len(set(names)), "duplicate package entries")
    require(set(files) == PAYLOAD | {".PKGINFO", ".BUILDINFO", ".MTREE"}, "unexpected package payload or hooks")
    directories = {str(parent) + "/" for name in PAYLOAD for parent in Path(name).parents if str(parent) != "."}
    require(set(names) - set(files) <= directories, "unexpected package directory")
    info = run(["bsdtar", "-xOf", str(package), ".PKGINFO"]).stdout.splitlines()
    require(f"pkgname = {PACKAGE}" in info, "package name mismatch")
    require(f"pkgver = {manifest['driver_version']}-1" in info, "package version mismatch")
    require("arch = x86_64" in info, "package architecture mismatch")
    require({line for line in info if line.startswith("depend = ")} ==
            {"depend = hyprland=0.56.2-1", "depend = gcc-libs"}, "package dependency mismatch")
    payload = {name: subprocess.check_output(["bsdtar", "-xOf", str(package), name]) for name in PAYLOAD}
    require(json.loads(payload[SOURCE]) == manifest, "packaged source provenance mismatch")
    require(digest(payload[LICENSE]) == manifest["files"]["LICENSE.md"], "packaged license provenance mismatch")
    build = json.loads(payload[BUILD])
    require(build["source"] == manifest, "packaged build source mismatch")
    require(build["module_sha256"] == digest(payload[MODULE]), "packaged module hash mismatch")
    require(build["module_runtime_sha256"] == build["compiler_runtime_sha256"] ==
            build["compositor_runtime_sha256"], "packaged runtime provenance mismatch")
    return {name: digest(data) for name, data in payload.items()}


def dependency_fixture(destination, name, version):
    # Metadata-only packages test ALPM's resolver, never native ABI compatibility.
    data = (f"pkgname = {name}\npkgver = {version}\npkgdesc = Lifecycle dependency fixture\n"
            "arch = x86_64\nsize = 0\n").encode()
    with tarfile.open(destination, "w:gz") as archive:
        info = tarfile.TarInfo(".PKGINFO")
        info.size = len(data)
        info.mode = 0o644
        archive.addfile(info, io.BytesIO(data))


def pacman_command(root, *arguments):
    require(root.is_absolute() and (root / "lifecycle-root").is_file(), "uninitialized isolated ALPM root")
    return ["sudo", "-n", "--", "pacman", "--root", str(root), "--dbpath", str(root / "var/lib/pacman"),
            "--config", str(root / "pacman.conf"), "--hookdir", str(root / "empty-hooks"),
            "--cachedir", str(root / "cache"), "--logfile", str(root / "pacman.log"),
            "--noscriptlet", "--noconfirm", *arguments]


def new_root(work, name):
    root = work / name
    root.mkdir()
    for path in ("var/lib/pacman", "empty-hooks", "cache", "etc/hypr"):
        (root / path).mkdir(parents=True)
    (root / "lifecycle-root").write_text("Disposable ALPM lifecycle root\n")
    (root / "pacman.conf").write_text("[options]\nArchitecture = x86_64\nSigLevel = Never\nLocalFileSigLevel = Never\n")
    (root / "etc/hypr/hyprland.conf").write_text("# Operator configuration sentinel\n")
    return root


def assert_state(root, payload, installed):
    require((root / "etc/hypr/hyprland.conf").read_text() == "# Operator configuration sentinel\n",
            "operator configuration changed")
    actual = {p.relative_to(root).as_posix() for p in (root / "usr").rglob("*") if not p.is_dir()}
    require(actual == (set(payload) if installed else set()), "installed payload inventory mismatch")
    for name, checksum in payload.items():
        path = root / name
        if installed:
            require(path.is_file() and not path.is_symlink(), "installed payload is not a regular file")
            require(digest(path.read_bytes()) == checksum, f"installed payload mismatch: {name}")
        else:
            require(not path.exists() and not path.is_symlink(), f"removed payload remains: {name}")


def qualify(work, package, payload, manifest):
    log = []

    def transaction(root, *arguments, check=True):
        result = run(pacman_command(root, *arguments), check=False,
                     env={**os.environ, "LC_ALL": "C"})
        log.append({"root": root.name, "operation": arguments[0], "returncode": result.returncode,
                    "stdout": result.stdout, "stderr": result.stderr})
        (work / "transactions.json").write_text(json.dumps(log, indent=2) + "\n")
        require(not check or result.returncode == 0, "pacman failed; see retained transactions.json")
        return result

    gcc = work / "gcc-libs-fixture.pkg.tar.gz"
    dependency_fixture(gcc, "gcc-libs", "1-1")
    for label, version in (("matching", "0.56.2-1"), ("mismatched", "0.56.2-2")):
        root = new_root(work, label)
        hyprland = work / f"hyprland-{label}-fixture.pkg.tar.gz"
        dependency_fixture(hyprland, "hyprland", version)
        transaction(root, "-U", str(gcc), str(hyprland))
        if label == "mismatched":
            result = transaction(root, "-U", str(package), check=False)
            diagnostic = result.stdout + result.stderr
            require(result.returncode != 0 and 'unable to satisfy dependency' in diagnostic and
                    'hyprland=0.56.2-1' in diagnostic, "missing specific Hyprland dependency refusal")
            assert_state(root, payload, False)
            require(transaction(root, "-Q", PACKAGE, check=False).returncode != 0,
                    "rejected package registered in ALPM")
            continue
        for stage in ("install", "remove", "reinstall"):
            installed = stage != "remove"
            transaction(root, "-U" if installed else "-R", str(package) if installed else PACKAGE)
            assert_state(root, payload, installed)
            result = transaction(root, "-Q", PACKAGE, check=False)
            if installed:
                require(result.returncode == 0 and result.stdout.strip() ==
                        f"{PACKAGE} {manifest['driver_version']}-1", "installed ALPM identity mismatch")
            else:
                require(result.returncode != 0, "removed package registered in ALPM")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--kit", type=Path, required=True, help="fresh standalone development or release kit")
    parser.add_argument("--revision", required=True)
    parser.add_argument("--driver-version", required=True)
    parser.add_argument("--cxx", type=Path, default=Path("/usr/bin/g++"))
    parser.add_argument("--output", type=Path, required=True, help="new retained evidence/build directory")
    args = parser.parse_args()
    try:
        require(platform.system() == "Linux" and platform.machine() == "x86_64", "requires native Linux x86_64")
        require(os.geteuid() != 0, "run as an ordinary build user; only isolated pacman uses sudo")
        require(args.cxx.is_absolute() and args.cxx.is_file(), "requires an absolute compiler path")
        kit = args.kit.resolve(strict=True)
        manifest, checksums = verify_kit(kit, args.revision, args.driver_version)
        work = args.output.absolute()
        work.mkdir(parents=True, exist_ok=False)
        work = work.resolve(strict=True)
        build = work / "kit"
        build.mkdir()
        for name in (*checksums, "SHA256SUMS"):
            (build / name).write_bytes((kit / name).read_bytes())
        env = {**os.environ, "LC_ALL": "C", "CUA_RELEASE_CXX": str(args.cxx),
               "PKGDEST": str(build), "SRCDEST": str(build), "BUILDDIR": str(build)}
        config = work / "makepkg.conf"
        config.write_text(Path("/etc/makepkg.conf").read_text() + "\n" +
                          "\n".join(f"{key}={shlex.quote(str(build))}"
                                    for key in ("PKGDEST", "SRCDEST", "BUILDDIR", "LOGDEST", "SRCPKGDEST")) + "\n")
        # No --syncdeps, --install, --skipinteg, --nocheck, or forced replacement.
        result = run(["makepkg", "--config", str(config), "--noconfirm"], cwd=build, env=env, check=False)
        (work / "makepkg.log").write_text(result.stdout + result.stderr)
        require(result.returncode == 0, "makepkg failed; see retained makepkg.log")
        packages = list(build.glob(f"{PACKAGE}-*.pkg.tar.*"))
        packages = [p for p in packages if not p.name.endswith(".sig")]
        require(len(packages) == 1, "expected one built plugin package")
        package = packages[0]
        payload = package_payload(package, manifest)
        qualify(work, package, payload, manifest)
        evidence = {"schema": 1, "result": "passed", "scope": "native build and isolated ALPM lifecycle",
                    "source_revision": args.revision, "driver_version": args.driver_version,
                    "plugin_version": manifest["plugin_version"], "kit_sha256": checksums,
                    "package_sha256": digest(package.read_bytes()), "payload_sha256": payload,
                    "dependency_fixtures": "metadata only; ABI verified by the native recipe",
                    "live_restart_verified": False, "live_rollback_verified": False,
                    "published_release_verified": False}
        (work / "RESULT.json").write_text(json.dumps(evidence, indent=2, sort_keys=True) + "\n")
        print("Passed: native build and isolated ALPM install/remove/reinstall/dependency refusal.")
        print("Live restart, rollback, and published release verification remain separate gates.")
    except (ValueError, KeyError, OSError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"error: {error}\n")


if __name__ == "__main__":
    main()
