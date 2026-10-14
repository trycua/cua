#!/usr/bin/env python3
"""Fail closed on source, Arch ABI, or compiler mismatches before packaging."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import subprocess
import tempfile

COMPILER_VERSION = "16.1.1 20260728"
COMPILER_COMMENT = "GCC: (GNU) " + COMPILER_VERSION
OPTIONS = {"CUA_HYPRLAND_INPUT": "ON", "CUA_HYPRLAND_TEST_INPUT": "OFF", "CUA_HYPRLAND_INPUT_TRACE": "OFF"}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def run(*command, input=None):
    return subprocess.check_output(command, input=input, text=True, stderr=subprocess.PIPE,
                                   env={**os.environ, "LC_ALL": "C"}).strip()


def linked_runtime(binary):
    dependencies = run("ldd", str(binary))
    matches = re.findall(r"^\s*libstdc\+\+\.so\.6 => (/\S+) \(", dependencies, re.MULTILINE)
    require(len(matches) == 1, "cannot resolve shared libstdc++ for " + binary.name)
    runtime = Path(matches[0]).resolve(strict=True)
    require(runtime.name == "libstdc++.so.6.0.36", "loaded shared libstdc++ runtime mismatch")
    return digest(runtime)


def verify_source(source, revision, driver_version):
    manifest = json.loads((source / "SOURCE-PROVENANCE.json").read_text())
    expected = {
        "schema": 1, "source_revision": revision, "driver_version": driver_version,
        "release_tag": f"cua-driver-rs-v{driver_version}",
        "hyprland_version": "0.56.2", "hyprland_package": "0.56.2-1",
        "compiler_version": COMPILER_VERSION, "compiler_comment": COMPILER_COMMENT,
        "architecture": "x86_64", "native_certified": False, "cmake_options": OPTIONS,
    }
    for key, value in expected.items():
        require(manifest.get(key) == value, f"source provenance mismatch: {key}")
    actual = set()
    for path in source.rglob("*"):
        require(not path.is_symlink(), f"source symlink refused: {path.name}")
        if path.is_file():
            actual.add(path.relative_to(source).as_posix())
    require(actual == set(manifest["files"]) | {"SOURCE-PROVENANCE.json"}, "source file inventory mismatch")
    for name, checksum in manifest["files"].items():
        require(digest(source / name) == checksum, f"source checksum mismatch: {name}")
    cmake = (source / "CMakeLists.txt").read_text()
    require(f"project(cua_hyprland_plugin VERSION {manifest['plugin_version']} LANGUAGES CXX)" in cmake, "plugin version mismatch")
    return manifest


def verify_native(cxx):
    require(platform.system() == "Linux" and platform.machine() == "x86_64", "requires Linux x86_64")
    require(cxx.is_absolute() and cxx.is_file(), "C++ compiler must be an existing absolute path")
    package = run("pacman", "-Q", "hyprland")
    require(package == "hyprland 0.56.2-1", "Hyprland package mismatch")
    require(run("pkg-config", "--modversion", "hyprland") == "0.56.2", "Hyprland header mismatch")
    macros = run(str(cxx), "-dM", "-E", "-x", "c++", "-", input="")
    require(f'#define __VERSION__ "{COMPILER_VERSION}"' in macros.splitlines(), "GCC version/date mismatch")
    require("#define __clang__ " not in macros, "Clang is not the pinned GCC compiler")
    comments = run("readelf", "-p", ".comment", "/usr/bin/Hyprland")
    entries = [re.sub(r"^\s*\[[^]]+\]\s*", "", line).strip() for line in comments.splitlines()]
    require(COMPILER_COMMENT in entries, "Hyprland compositor compiler mismatch")
    # Check what the selected compiler emits, not only its self-reported version.
    with tempfile.TemporaryDirectory(prefix="cua-compiler-probe-") as temporary:
        probe = Path(temporary) / "probe.o"
        run(str(cxx), "-x", "c++", "-c", "-o", str(probe), "-", input="int cua_compiler_probe;\n")
        probe_comments = run("readelf", "-p", ".comment", str(probe))
        probe_entries = [re.sub(r"^\s*\[[^]]+\]\s*", "", line).strip() for line in probe_comments.splitlines()]
        require(COMPILER_COMMENT in probe_entries, "compiler probe does not match compositor compiler")
    runtime = Path(run(str(cxx), "--print-file-name=libstdc++.so.6")).resolve(strict=True)
    require(runtime.name == "libstdc++.so.6.0.36", "compiler shared libstdc++ runtime mismatch")
    runtime_sha = digest(runtime)
    require(linked_runtime(Path("/usr/bin/Hyprland")) == runtime_sha, "compositor and compiler shared runtimes differ")
    return {
        "hyprland_package": package,
        "hyprland_header_version": "0.56.2",
        "compositor_sha256": digest(Path("/usr/bin/Hyprland")),
        "compositor_compiler": COMPILER_COMMENT,
        "compiler_version": COMPILER_VERSION,
        "compiler_banner": run(str(cxx), "--version"),
        "compiler_sha256": digest(cxx),
        "compiler_probe_comment": COMPILER_COMMENT,
        "compiler_runtime": runtime.name,
        "compiler_runtime_sha256": runtime_sha,
        "compositor_runtime_sha256": runtime_sha,
    }


def verify_build(build, source, cxx, runtime_sha):
    cache = {}
    for line in (build / "CMakeCache.txt").read_text().splitlines():
        match = re.match(r"([^:#/][^:]*):[^=]+=(.*)", line)
        if match:
            cache[match[1]] = match[2]
    expected = dict(OPTIONS, BUILD_TESTING="ON", CUA_HYPRLAND_BUILD_PLUGIN="ON",
                    CUA_HYPRLAND_EXPECTED_VERSION="0.56.2", CMAKE_BUILD_TYPE="Release",
                    CUA_HYPRLAND_TEST_OPERATOR_KEY="", CMAKE_CXX_COMPILER=str(cxx),
                    CMAKE_HOME_DIRECTORY=str(source.resolve()))
    for key, value in expected.items():
        require(cache.get(key) == value, f"build configuration mismatch: {key}")
    module = build / "cua-hyprland-plugin.so"
    dynamic = run("readelf", "-d", str(module))
    require("Shared library: [libstdc++.so.6]" in dynamic, "module must use shared libstdc++")
    require("Shared library: [libc++.so" not in dynamic, "module uses incompatible libc++")
    require(linked_runtime(module) == runtime_sha, "module and compiler shared runtimes differ")
    return digest(module)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", required=True, type=Path)
    parser.add_argument("--revision", required=True)
    parser.add_argument("--driver-version", required=True)
    parser.add_argument("--cxx", required=True, type=Path)
    parser.add_argument("--build", type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    try:
        manifest = verify_source(args.source, args.revision, args.driver_version)
        native = verify_native(args.cxx)
        if args.build:
            native["module_sha256"] = verify_build(args.build, args.source, args.cxx, native["compiler_runtime_sha256"])
            native["module_runtime_sha256"] = native["compiler_runtime_sha256"]
        if args.output:
            require(args.build is not None, "build evidence is required for output")
            native["source"] = manifest
            args.output.write_text(json.dumps(native, indent=2, sort_keys=True) + "\n")
    except (ValueError, KeyError, OSError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"error: {error}\n")


if __name__ == "__main__":
    main()
