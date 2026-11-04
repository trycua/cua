#!/usr/bin/env python3
"""Measure a native Omarchy host into a candidate profile, and classify rebuild reuse.

The output is reviewed data for profile_bundle.py, never certification. The
reviewer supplies the source identity and digests as trust roots; this tool
only measures the native environment and refuses a source that does not match.
"""

import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tarfile

import profile_verify as verify

HYPRLAND = Path("/usr/bin/Hyprland")
SOURCE_FIELDS = ("revision", "driver_version", "archive_sha256", "manifest_sha256")
# Sections whose change alters package bytes or ABI and needs a new build and requalification.
SECTIONS = ("source", "architecture", "hyprland", "compiler", "runtime")
LABELS = ("schema", "profile_id", "kit_version", "package_release")


def compositor_libraries():
    """Resolved shared-library paths of the installed compositor, by soname."""
    libraries = {}
    for line in verify.run("ldd", str(HYPRLAND)).splitlines():
        match = re.fullmatch(r"\s*(\S+) => (/\S+) \(0x[0-9a-fA-F]+\)\s*", line)
        if match:
            libraries[match[1]] = Path(match[2]).resolve(strict=True)
    return libraries


def owner_versions(paths):
    owners = {verify.run("pacman", "-Qoq", str(path)) for path in paths}
    return {name: verify.run("pacman", "-Q", name).split(" ", 1)[1] for name in sorted(owners)}


def measure(profile_id, kit_version, package_release, source, archive, cxx):
    """Return a candidate schema-2 profile measured from this host."""
    verify.require(set(source) == set(SOURCE_FIELDS), "source must name " + ", ".join(SOURCE_FIELDS))
    verify.require(not any(key.startswith("LD_") for key in os.environ),
                   "clear dynamic-loader LD_* overrides before measuring")
    verify.require(verify.platform.system() == "Linux" and verify.platform.machine() == "x86_64", "requires Linux x86_64")
    verify.require(cxx.is_absolute() and cxx.is_file(), "C++ compiler must be an existing absolute path")
    libraries = compositor_libraries()
    verify.require("libstdc++.so.6" in libraries, "compositor does not use shared libstdc++")
    runtime = libraries["libstdc++.so.6"]
    compiler_runtime = Path(verify.run(str(cxx), "--print-file-name=libstdc++.so.6")).resolve(strict=True)
    verify.require(compiler_runtime == runtime, "compiler and compositor resolve different shared libstdc++ files")
    macros = verify.run(str(cxx), "-dM", "-E", "-x", "c++", "-", input="")
    version = re.search(r'^#define __VERSION__ "([^"]+)"$', macros, re.MULTILINE)
    verify.require(version and not re.search(r"^#define __clang__\b", macros, re.MULTILINE), "compiler must be GCC with a version/date")
    profile = {
        "schema": 2, "profile_id": profile_id, "kit_version": kit_version, "package_release": package_release,
        "source": dict(source), "architecture": "x86_64",
        "hyprland": {
            "package_version": verify.run("pacman", "-Q", "hyprland").split(" ", 1)[1],
            "header_version": verify.run(str(verify.PKGCONF), "--modversion", "hyprland", extra_env=verify.PKGCONF_ENV),
            "headers_sha256": verify.header_inventory_sha256(), "sha256": verify.digest(HYPRLAND)},
        "compiler": {"version": version[1], "comment": "GCC: (GNU) " + version[1], "sha256": verify.digest(cxx)},
        # Packages are the owners of every library the compositor resolves; the reviewer confirms the set.
        "runtime": {"basename": runtime.name, "sha256": verify.digest(runtime),
                    "packages": owner_versions(set(libraries.values()))},
    }
    verify.validate_profile(profile)
    verify.verify_archive(archive, profile)
    # Self-consistency only: proves the profile matches this host, not that it is qualified.
    verify.verify_native(cxx, profile)
    return profile


def reuse_decision(reviewed, candidate):
    """Compare profile inputs; this does not compare package or evidence bytes.

    "profile-unchanged": identical profile data; independently verify package bytes and evidence.
    "relabel-rebuild": only schema, profile_id, kit_version or package_release differ; the
    package bytes change, so the affected evidence is repeated.
    "rebuild": source, architecture, compositor, headers, compiler or ABI runtime differ.
    """
    verify.validate_profile(reviewed)
    verify.validate_profile(candidate)
    reasons = [f"{name} differs" for name in SECTIONS if reviewed[name] != candidate[name]]
    if reasons:
        return "rebuild", reasons
    reasons = [f"{name} differs" for name in LABELS if reviewed[name] != candidate[name]]
    return ("relabel-rebuild", reasons) if reasons else ("profile-unchanged", [])


def load(path):
    return verify.validate_profile(verify.read_json(path.read_bytes()))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    m = sub.add_parser("measure", help="write a candidate profile from this host")
    m.add_argument("--profile-id", required=True)
    m.add_argument("--kit-version", required=True)
    m.add_argument("--package-release", required=True, type=int)
    m.add_argument("--revision", required=True, help="reviewed full source commit SHA")
    m.add_argument("--driver-version", required=True)
    m.add_argument("--archive", required=True, type=Path)
    m.add_argument("--archive-sha256", required=True, help="independently reviewed archive digest")
    m.add_argument("--manifest-sha256", required=True, help="independently reviewed manifest digest")
    m.add_argument("--cxx", required=True, type=Path)
    m.add_argument("--output", required=True, type=Path, help="new profile file")
    r = sub.add_parser("reuse", help="classify whether reviewed evidence survives a candidate profile")
    r.add_argument("--reviewed", required=True, type=Path)
    r.add_argument("--candidate", required=True, type=Path)
    args = parser.parse_args()
    try:
        if args.command == "measure":
            source = {"revision": args.revision, "driver_version": args.driver_version,
                      "archive_sha256": args.archive_sha256, "manifest_sha256": args.manifest_sha256}
            profile = measure(args.profile_id, args.kit_version, args.package_release, source, args.archive, args.cxx)
            with args.output.open("xb") as handle:
                handle.write(verify.json_bytes(profile))
            print(f"Wrote candidate {args.output}; review it. It is not native certification.")
        else:
            verdict, reasons = reuse_decision(load(args.reviewed), load(args.candidate))
            print(json.dumps({"verdict": verdict, "reasons": reasons, "qualification_verified": False}))
            sys.exit(0 if verdict == "profile-unchanged" else 3)
    except (ValueError, KeyError, TypeError, OSError, tarfile.TarError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"error: {error}\n")


if __name__ == "__main__":
    main()
