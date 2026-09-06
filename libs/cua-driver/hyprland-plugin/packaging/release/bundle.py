#!/usr/bin/env python3
"""Create a deterministic, pinned plugin source release from one Git commit."""

import argparse
import gzip
import hashlib
import io
import json
from pathlib import Path
import re
import subprocess
import tarfile
import tomllib

PLUGIN = "libs/cua-driver/hyprland-plugin/"
RELEASE = PLUGIN + "packaging/release/"
# Deliberate allowlist: adding source or build fixtures requires release review.
# No directory traversal, live harnesses, documentation/evidence, or worktree reads.
SOURCE_FILES = """
CMakeLists.txt
cmake/DetectHyprlandAPI.cmake
cmake/VerifyRuntime.cmake
include/cua_hyprland/protocol.hpp
include/cua_hyprland/session.hpp
include/cua_hyprland/status.hpp
src/drag_geometry.hpp
src/inject_server.cpp
src/inject_server.hpp
src/input_client_deadline.hpp
src/input_experiment.cpp
src/input_experiment.hpp
src/input_grant.hpp
src/owned_socket_path.hpp
src/passive_pointer_target.hpp
src/plugin.cpp
src/primary_trace.cpp
src/primary_trace.hpp
src/protocol.cpp
src/seat_lifetime.hpp
src/session.cpp
src/status.cpp
tests/cmake-api/CMakeLists.txt
tests/cmake-api/include/src/plugins/PluginAPI.hpp
tests/drag_geometry_test.cpp
tests/input_grant_test.cpp
tests/input_client_deadline_test.cpp
tests/mock-hyprland/mock.hpp
tests/mock-hyprland/src/config/values/types/BoolValue.hpp
tests/mock-hyprland/src/plugins/PluginAPI.hpp
tests/owned_socket_path_test.cpp
tests/passive_pointer_target_test.cpp
tests/plugin_api_test.cpp
tests/plugin_input_lifetime_test.cpp
tests/protocol_test.cpp
tests/seat_lifetime_test.cpp
tests/status_test.cpp
tests/transport_test.cpp
""".split()


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def json_bytes(value):
    return (json.dumps(value, sort_keys=True, indent=2) + "\n").encode()


def git(repo, *args):
    return subprocess.check_output(["git", "-C", str(repo), *args])


def committed_file(repo, revision, path):
    entry = git(repo, "ls-tree", revision, "--", path).decode().strip()
    if not entry.startswith("100644 blob ") and not entry.startswith("100755 blob "):
        raise ValueError(f"required committed regular file missing: {path}")
    return git(repo, "show", f"{revision}:{path}")


def deterministic_archive(payload):
    raw_tar = io.BytesIO()
    with tarfile.open(fileobj=raw_tar, mode="w", format=tarfile.USTAR_FORMAT) as archive:
        for name, data in sorted(payload.items()):
            info = tarfile.TarInfo(name)
            info.size = len(data)
            info.mode = 0o644
            info.mtime = 0
            archive.addfile(info, io.BytesIO(data))
    compressed = io.BytesIO()
    with gzip.GzipFile(filename="", fileobj=compressed, mode="wb", mtime=0, compresslevel=9) as archive:
        archive.write(raw_tar.getvalue())
    return compressed.getvalue()


def generate(repo, revision, driver_version, output, *, release_assets=False):
    if not re.fullmatch(r"[0-9a-f]{40}", revision):
        raise ValueError("source revision must be a full lowercase commit SHA")
    if git(repo, "rev-parse", f"{revision}^{{commit}}").decode().strip() != revision:
        raise ValueError("source revision must name a commit")
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", driver_version):
        raise ValueError("driver version must be a stable numeric release version")
    cargo = tomllib.loads(committed_file(repo, revision, "libs/cua-driver/rust/Cargo.toml").decode())
    if cargo["workspace"]["package"]["version"] != driver_version:
        raise ValueError("driver version does not match committed Cargo.toml")
    if release_assets:
        tag = f"refs/tags/cua-driver-rs-v{driver_version}"
        if git(repo, "rev-parse", "--verify", f"{tag}^{{commit}}").decode().strip() != revision:
            raise ValueError("source revision does not match the exact Driver release tag")
    payload = {name: committed_file(repo, revision, PLUGIN + name) for name in SOURCE_FILES}
    payload["LICENSE.md"] = committed_file(repo, revision, "LICENSE.md")
    payload["verify.py"] = committed_file(repo, revision, RELEASE + "verify.py")
    version = re.search(rb"project\(cua_hyprland_plugin VERSION ([0-9.]+) LANGUAGES CXX\)", payload["CMakeLists.txt"])
    if not version:
        raise ValueError("cannot resolve committed plugin version")
    metadata = {
        "schema": 1,
        "source_revision": revision,
        "driver_version": driver_version,
        "plugin_version": version[1].decode(),
        "release_tag": f"cua-driver-rs-v{driver_version}",
        "hyprland_version": "0.56.2",
        "hyprland_package": "0.56.2-1",
        "compiler_version": "16.1.1 20260728",
        "compiler_comment": "GCC: (GNU) 16.1.1 20260728",
        "architecture": "x86_64",
        "native_certified": False,
        "cmake_options": {"CUA_HYPRLAND_INPUT": "ON", "CUA_HYPRLAND_TEST_INPUT": "OFF", "CUA_HYPRLAND_INPUT_TRACE": "OFF"},
        "files": {name: sha256(data) for name, data in sorted(payload.items())},
    }
    payload["SOURCE-PROVENANCE.json"] = json_bytes(metadata)
    stem = f"cua-hyprland-plugin-{driver_version}-{revision}"
    tarball = deterministic_archive({f"{stem}/{name}": data for name, data in payload.items()})
    template = committed_file(repo, revision, RELEASE + "PKGBUILD.in").decode()
    replacements = {
        "DRIVER_VERSION": driver_version,
        "REVISION": revision,
        "STEM": stem,
        "ARCHIVE_SHA256": sha256(tarball),
        "MANIFEST_SHA256": sha256(payload["SOURCE-PROVENANCE.json"]),
    }
    for key, value in replacements.items():
        template = template.replace(f"@{key}@", value)
    if re.search(r"@[A-Z_]+@", template):
        raise ValueError("unresolved recipe placeholder")
    usage = committed_file(repo, revision, RELEASE + "USAGE.md")
    files = {
        f"{stem}.tar.gz": tarball,
        "PKGBUILD": template.encode(),
        "SOURCE-PROVENANCE.json": payload["SOURCE-PROVENANCE.json"],
        "README.md": usage,
        "lifecycle.py": committed_file(repo, revision, RELEASE + "lifecycle.py"),
    }
    files["SHA256SUMS"] = "".join(
        f"{sha256(data)}  {name}\n" for name, data in sorted(files.items())
    ).encode()
    if release_assets:
        # Keep generic recipe/document/checksum names inside one namespaced kit.
        # The source remains a separate asset at the URL pinned by PKGBUILD.
        kit = {name: data for name, data in files.items() if name != f"{stem}.tar.gz"}
        files = {
            f"{stem}.tar.gz": tarball,
            f"{stem}-build-kit.tar.gz": deterministic_archive(kit),
        }
    # Exclusive creation prevents replacing a previously reviewed release asset.
    output.mkdir(parents=True, exist_ok=False)
    for name, data in files.items():
        (output / name).write_bytes(data)
    return metadata


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, required=True)
    parser.add_argument("--revision", required=True)
    parser.add_argument("--driver-version", required=True)
    parser.add_argument("--output", type=Path, required=True, help="new directory; existing paths are refused")
    parser.add_argument("--release-assets", action="store_true", help="require the exact component tag and emit source/build-kit release archives")
    args = parser.parse_args()
    try:
        generate(args.repo.resolve(strict=True), args.revision, args.driver_version, args.output, release_assets=args.release_assets)
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"error: {error}\n")


if __name__ == "__main__":
    main()
