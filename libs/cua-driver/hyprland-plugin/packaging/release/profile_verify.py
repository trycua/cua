#!/usr/bin/env python3
"""Verify a reviewed native profile against immutable source or an installed module."""

import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import platform
import re
import shlex
import subprocess
import tarfile
import tempfile

SOURCE_REVISION = "4b3396d9fe4bd3cf723b0eb8db83c18a8764b520"
DRIVER_VERSION = "0.24.0"
STEM = f"cua-hyprland-plugin-{DRIVER_VERSION}-{SOURCE_REVISION}"
OPTIONS = {"CUA_HYPRLAND_INPUT": "ON", "CUA_HYPRLAND_TEST_INPUT": "OFF", "CUA_HYPRLAND_INPUT_TRACE": "OFF"}
TOOLING = ("profile_bundle.py", "profile_verify.py", "PROFILE-PKGBUILD.in", "PROFILE-USAGE.md", "lifecycle.py")
PKGCONF = Path("/usr/bin/pkgconf")
HYPRLAND_PC = Path("/usr/share/pkgconfig/hyprland.pc")
SYSTEM_INCLUDE = Path("/usr/include")
HEADER_ROOT = SYSTEM_INCLUDE / "hyprland"
BUILD_ROUTING_ENV = {"CPATH", "CPLUS_INCLUDE_PATH", "C_INCLUDE_PATH", "OBJC_INCLUDE_PATH",
                     "GCC_EXEC_PREFIX", "COMPILER_PATH", "LIBRARY_PATH", "SDKROOT", "SYSROOT"}
EMPTY_CMAKE_ROUTING = ("CMAKE_PREFIX_PATH", "CMAKE_MODULE_PATH", "CMAKE_TOOLCHAIN_FILE", "CMAKE_SYSROOT",
                       "CMAKE_SYSROOT_COMPILE", "CMAKE_SYSROOT_LINK", "CMAKE_FIND_ROOT_PATH",
                       "CMAKE_CXX_COMPILER_EXTERNAL_TOOLCHAIN", "CMAKE_CXX_COMPILER_LAUNCHER",
                       "CMAKE_CXX_LINKER_LAUNCHER", "CMAKE_PROJECT_INCLUDE", "CMAKE_PROJECT_INCLUDE_BEFORE",
                       "CMAKE_PROJECT_TOP_LEVEL_INCLUDES", "CMAKE_USER_MAKE_RULES_OVERRIDE", "CMAKE_USER_MAKE_RULES_OVERRIDE_CXX",
                       "CMAKE_CXX_COMPILER_TARGET", "CMAKE_CXX_STANDARD_INCLUDE_DIRECTORIES")


def require(condition, message):
    if not condition:
        raise ValueError(message)


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def digest(path):
    return sha256(path.read_bytes())


def json_bytes(value):
    return (json.dumps(value, sort_keys=True, indent=2) + "\n").encode()


def read_json(data):
    def unique(pairs):
        result = {}
        for key, value in pairs:
            require(key not in result, f"duplicate JSON key: {key}")
            result[key] = value
        return result
    return json.loads(data, object_pairs_hook=unique)


def keys(value, expected, label):
    require(isinstance(value, dict) and set(value) == set(expected.split()), f"invalid {label} fields")


def hash_value(value):
    require(isinstance(value, str) and re.fullmatch(r"[0-9a-f]{64}", value), "requires a lowercase SHA-256")


def validate_profile(profile):
    keys(profile, "schema profile_id kit_version package_release source architecture hyprland compiler runtime", "profile")
    require(type(profile["schema"]) is int and profile["schema"] == 1, "unsupported profile schema")
    require(len(profile["profile_id"]) <= 32 and re.fullmatch(r"[a-z0-9]+(?:-[a-z0-9]+)*", profile["profile_id"]), "invalid profile ID")
    require(len(profile["kit_version"]) <= 20 and re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", profile["kit_version"]), "invalid kit version")
    require(type(profile["package_release"]) is int and profile["package_release"] >= 2, "profile package release must be >=2")
    require(profile["architecture"] == "x86_64", "only x86_64 is supported")
    source = profile["source"]
    keys(source, "revision driver_version archive_sha256 manifest_sha256", "source")
    require(source["revision"] == SOURCE_REVISION and source["driver_version"] == DRIVER_VERSION, "requires the original Driver 0.24.0 source")
    hash_value(source["archive_sha256"])
    hash_value(source["manifest_sha256"])
    hyprland = profile["hyprland"]
    keys(hyprland, "package_version header_version headers_sha256 sha256", "Hyprland")
    require(hyprland["header_version"] == "0.56.2", "unchanged source requires Hyprland 0.56.2 headers")
    require(re.fullmatch(r"0\.56\.2-[0-9]+(?:\.[0-9]+)?", hyprland["package_version"]), "invalid Hyprland package version")
    hash_value(hyprland["sha256"])
    hash_value(hyprland["headers_sha256"])
    compiler = profile["compiler"]
    keys(compiler, "version comment sha256", "compiler")
    require(re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+ [0-9]{8}", compiler["version"]), "requires exact GCC version/date")
    require(compiler["comment"] == "GCC: (GNU) " + compiler["version"], "invalid compiler ELF comment")
    hash_value(compiler["sha256"])
    runtime = profile["runtime"]
    keys(runtime, "basename sha256 packages", "runtime")
    require(re.fullmatch(r"libstdc\+\+\.so\.6\.0\.[0-9]+", runtime["basename"]), "invalid shared runtime basename")
    hash_value(runtime["sha256"])
    packages = runtime["packages"]
    require(isinstance(packages, dict) and packages, "requires reviewed ABI runtime packages")
    for name in packages:
        require(re.fullmatch(r"[a-z0-9][a-z0-9@._+-]*", name) and name not in {"hyprland", "python", "binutils"}, "invalid ABI package name")
    for version in packages.values():
        require(isinstance(version, str) and re.fullmatch(r"[0-9][0-9A-Za-z.:+_~-]*-[0-9]+(?:\.[0-9]+)?", version), "invalid runtime package version")
    return profile


def source_manifest(data, profile):
    require(sha256(data) == profile["source"]["manifest_sha256"], "historical manifest checksum mismatch")
    manifest = read_json(data)
    expected = {"schema": 1, "source_revision": SOURCE_REVISION, "driver_version": DRIVER_VERSION,
                "release_tag": "cua-driver-rs-v0.24.0", "plugin_version": "0.1.0",
                "architecture": "x86_64", "native_certified": False, "cmake_options": OPTIONS,
                "hyprland_version": "0.56.2", "hyprland_package": "0.56.2-1",
                "compiler_version": "16.1.1 20260728", "compiler_comment": "GCC: (GNU) 16.1.1 20260728"}
    require(set(manifest) == set(expected) | {"files"}, "invalid historical manifest fields")
    for key, value in expected.items():
        require(manifest[key] == value, f"historical source provenance mismatch: {key}")
    require(isinstance(manifest["files"], dict) and {"CMakeLists.txt", "LICENSE.md", "verify.py"} <= set(manifest["files"]), "invalid source inventory")
    for name, checksum in manifest["files"].items():
        path = PurePosixPath(name)
        require(not path.is_absolute() and path.as_posix() == name and ".." not in path.parts and "\\" not in name and name != "SOURCE-PROVENANCE.json", "unsafe source inventory path")
        hash_value(checksum)
    return manifest


def verify_archive(archive, profile):
    require(archive.is_file() and not archive.is_symlink(), "source archive must be a regular file")
    require(digest(archive) == profile["source"]["archive_sha256"], "source archive checksum mismatch")
    payload = {}
    with tarfile.open(archive, "r:gz") as contents:
        for member in contents:
            require(member.isfile() and member.name.startswith(STEM + "/"), "invalid source archive member")
            name = member.name[len(STEM) + 1:]
            path = PurePosixPath(name)
            require(name and path.as_posix() == name and not path.is_absolute() and ".." not in path.parts and "\\" not in name, "unsafe source archive path")
            require(name not in payload, "duplicate source archive member")
            payload[name] = contents.extractfile(member).read()
    require("SOURCE-PROVENANCE.json" in payload, "missing source manifest")
    manifest = source_manifest(payload["SOURCE-PROVENANCE.json"], profile)
    require(set(payload) == set(manifest["files"]) | {"SOURCE-PROVENANCE.json"}, "source archive inventory mismatch")
    for name, checksum in manifest["files"].items():
        require(sha256(payload[name]) == checksum, f"source archive content mismatch: {name}")
    return manifest


def verify_source(source, profile):
    require(source.is_dir() and not source.is_symlink(), "source must be a real directory")
    manifest = source_manifest((source / "SOURCE-PROVENANCE.json").read_bytes(), profile)
    actual = set()
    for path in source.rglob("*"):
        require(not path.is_symlink() and (path.is_dir() or path.is_file()), "source contains a nonregular entry")
        if path.is_file():
            actual.add(path.relative_to(source).as_posix())
    require(actual == set(manifest["files"]) | {"SOURCE-PROVENANCE.json"}, "source file inventory mismatch")
    for name, checksum in manifest["files"].items():
        require(digest(source / name) == checksum, f"source checksum mismatch: {name}")
    require(f"project(cua_hyprland_plugin VERSION {manifest['plugin_version']} LANGUAGES CXX)" in (source / "CMakeLists.txt").read_text(), "plugin version mismatch")
    return manifest


def verify_kit(kit, expected_sha, *, complete=False):
    hash_value(expected_sha)
    provenance_path = kit / "KIT-PROVENANCE.json"
    require(provenance_path.is_file() and not provenance_path.is_symlink() and digest(provenance_path) == expected_sha, "kit provenance checksum mismatch")
    provenance = read_json(provenance_path.read_bytes())
    keys(provenance, "schema tooling_revision profile_sha256 source cmake_options native_certified tooling_files", "kit provenance")
    require(provenance["schema"] == 1 and provenance["native_certified"] is False and provenance["cmake_options"] == OPTIONS, "invalid kit contract")
    require(re.fullmatch(r"[0-9a-f]{40}", provenance["tooling_revision"]), "invalid tooling revision")
    require(set(provenance["tooling_files"]) == set(TOOLING), "tooling inventory mismatch")
    for checksum in provenance["tooling_files"].values():
        hash_value(checksum)
    profile_path = kit / "PROFILE.json"
    require(profile_path.is_file() and not profile_path.is_symlink() and digest(profile_path) == provenance["profile_sha256"], "profile checksum mismatch")
    profile = validate_profile(read_json(profile_path.read_bytes()))
    require(provenance["source"] == profile["source"], "kit source identity mismatch")
    required = TOOLING if complete else ("profile_verify.py",)
    for name in required:
        path = kit / name
        require(path.is_file() and not path.is_symlink() and digest(path) == provenance["tooling_files"][name], f"tooling checksum mismatch: {name}")
    require(digest(Path(__file__)) == provenance["tooling_files"]["profile_verify.py"], "executing verifier differs from reviewed kit")
    if complete:
        expected_recipe = render_recipe((kit / "PROFILE-PKGBUILD.in").read_text(), profile, provenance)
        require((kit / "PKGBUILD").read_bytes() == expected_recipe, "recipe differs from reviewed tooling/profile")
    return profile, provenance


def render_recipe(template, profile, provenance):
    replacements = {"DRIVER_VERSION": DRIVER_VERSION, "PKGREL": str(profile["package_release"]),
                    "PROFILE_ID": profile["profile_id"], "STEM": STEM,
                    "HYPRLAND_PACKAGE": profile["hyprland"]["package_version"],
                    "RUNTIME_DEPENDS": " ".join(f"'{name}={version}'" for name, version in sorted(profile["runtime"]["packages"].items())),
                    "ARCHIVE_SHA256": profile["source"]["archive_sha256"],
                    "PROFILE_SHA256": provenance["profile_sha256"],
                    "KIT_SHA256": sha256(json_bytes(provenance)),
                    "VERIFIER_SHA256": provenance["tooling_files"]["profile_verify.py"]}
    for key, value in replacements.items():
        template = template.replace(f"@{key}@", value)
    require(not re.search(r"@[A-Z_]+@", template), "unresolved recipe placeholder")
    return template.encode()


def run(*command, input=None):
    return subprocess.check_output(command, input=input, text=True, stderr=subprocess.PIPE,
                                   env={**os.environ, "LC_ALL": "C"}).strip()


def elf_comment(binary, expected):
    lines = run("readelf", "-p", ".comment", str(binary)).splitlines()
    entries = [re.sub(r"^\s*\[[^]]+\]\s*", "", line).strip() for line in lines]
    require(expected in entries, "ELF compiler comment mismatch: " + binary.name)


def linked_runtime(binary, profile):
    dynamic = run("readelf", "-d", str(binary))
    require("Shared library: [libstdc++.so.6]" in dynamic and "Shared library: [libc++.so" not in dynamic, "binary must use shared libstdc++")
    resolved = {}
    for line in run("ldd", str(binary)).splitlines():
        # ldd may exit zero even when a different required library is missing.
        # Reject unresolved dependencies and diagnostics, not just a missing C++ runtime.
        match = re.fullmatch(r"\s*(\S+) => (/\S+) \(0x[0-9a-fA-F]+\)\s*", line)
        if match:
            name, path = match.groups()
        else:
            match = re.fullmatch(r"\s*(/\S+) \(0x[0-9a-fA-F]+\)\s*", line)
            if match:
                path = match[1]
                name = Path(path).name
            else:
                require(re.fullmatch(r"\s*linux-(?:vdso|gate)\.so\.[0-9]+ \(0x[0-9a-fA-F]+\)\s*", line),
                        "unresolved or malformed shared dependency: " + line.strip())
                continue
        require(name not in resolved, "duplicate shared dependency resolution")
        resolved[name] = path
    needed = re.findall(r"Shared library: \[([^]]+)\]", dynamic)
    require(set(needed) <= set(resolved), "missing shared dependency resolution")
    require("libstdc++.so.6" in resolved, "cannot resolve shared libstdc++")
    runtime = Path(resolved["libstdc++.so.6"]).resolve(strict=True)
    require(runtime.name == profile["runtime"]["basename"] and digest(runtime) == profile["runtime"]["sha256"], "loaded shared runtime mismatch")
    require(run("pacman", "-Qoq", str(runtime)) in profile["runtime"]["packages"], "shared runtime owner is not pinned by profile")
    return digest(runtime)


def verify_environment(profile):
    require(platform.system() == "Linux" and platform.machine() == profile["architecture"], "requires Linux x86_64")
    packages = {"hyprland": profile["hyprland"]["package_version"], **profile["runtime"]["packages"]}
    for name, version in packages.items():
        require(run("pacman", "-Q", name) == f"{name} {version}", f"native package mismatch: {name}")
    compositor = Path("/usr/bin/Hyprland")
    require(digest(compositor) == profile["hyprland"]["sha256"], "compositor checksum mismatch")
    elf_comment(compositor, profile["compiler"]["comment"])
    return linked_runtime(compositor, profile)


def verify_native(cxx, profile):
    verify_build_environment()
    runtime_sha = verify_environment(profile)
    require(cxx.is_absolute() and cxx.is_file(), "C++ compiler must be an existing absolute path")
    require(digest(cxx) == profile["compiler"]["sha256"], "compiler checksum mismatch")
    pkgconfig = pkgconfig_selection(profile)
    require(header_inventory_sha256() == profile["hyprland"]["headers_sha256"], "Hyprland header inventory mismatch")
    macros = run(str(cxx), "-dM", "-E", "-x", "c++", "-", input="")
    require(f'#define __VERSION__ "{profile["compiler"]["version"]}"' in macros.splitlines() and not re.search(r"^#define __clang__\b", macros, re.MULTILINE), "GCC version/date mismatch")
    with tempfile.TemporaryDirectory(prefix="cua-profile-probe-") as temporary:
        probe = Path(temporary) / "probe.o"
        run(str(cxx), "-x", "c++", "-c", "-o", str(probe), "-", input="int cua_compiler_probe;\n")
        elf_comment(probe, profile["compiler"]["comment"])
    runtime = Path(run(str(cxx), "--print-file-name=libstdc++.so.6")).resolve(strict=True)
    require(runtime.name == profile["runtime"]["basename"] and digest(runtime) == runtime_sha, "compiler shared runtime mismatch")
    return {"compiler_sha256": digest(cxx), "compiler_version": profile["compiler"]["version"],
            "compiler_probe_comment": profile["compiler"]["comment"], "compiler_runtime_sha256": runtime_sha,
            "compositor_sha256": profile["hyprland"]["sha256"], "compositor_runtime_sha256": runtime_sha,
            "pkgconfig": pkgconfig}


def verify_flags(flags, label):
    # Keep normal makepkg optimization/hardening flags, but refuse options that
    # inject headers, an alternate toolchain, or hidden response-file arguments.
    routing = ("-I", "-L", "-B", "-isystem", "-iquote", "-idirafter", "-iprefix", "-iwithprefix",
               "-include", "-imacros", "-isysroot", "--sysroot", "-nostdinc", "-specs", "--specs",
               "-fplugin", "-wrapper", "-Xpreprocessor", "-Xclang", "-Xlinker", "--library-path",
               "-rpath", "--rpath", "--gcc-toolchain", "-gcc-toolchain", "-resource-dir")
    for flag in flags:
        arguments = flag[4:].split(",") if flag.startswith(("-Wp,", "-Wl,")) else [flag]
        for argument in arguments:
            require(not argument.startswith(("@", *routing)), f"header/toolchain flag override refused: {label}")


def verify_build_environment():
    for name, value in os.environ.items():
        routed = (name in BUILD_ROUTING_ENV or name.startswith(("PKG_CONFIG", "PKGCONF")) or
                  (name.startswith("CMAKE_") and name != "CMAKE_BUILD_PARALLEL_LEVEL"))
        require(not value or not routed, f"build routing environment refused: {name}")
    for name in ("CFLAGS", "CXXFLAGS", "CPPFLAGS", "LDFLAGS"):
        verify_flags(shlex.split(os.environ.get(name, "")), name)


def pkgconfig_selection(profile):
    require(PKGCONF.is_file(), "canonical /usr/bin/pkgconf is required")
    require(run("pacman", "-Qoq", str(PKGCONF)) == "pkgconf", "pkgconf executable owner mismatch")
    require(run(str(PKGCONF), "--variable=pcfiledir", "hyprland") == str(HYPRLAND_PC.parent), "noncanonical Hyprland pkg-config source")
    require(HYPRLAND_PC.is_file() and not HYPRLAND_PC.is_symlink() and HYPRLAND_PC.resolve() == HYPRLAND_PC,
            "Hyprland pkg-config source must be canonical")
    require(run("pacman", "-Qoq", str(HYPRLAND_PC)) == "hyprland", "Hyprland pkg-config owner mismatch")
    require(run(str(PKGCONF), "--modversion", "hyprland") == profile["hyprland"]["header_version"], "Hyprland header mismatch")
    cflags = shlex.split(run(str(PKGCONF), "--cflags", "hyprland"))
    includes = shlex.split(run(str(PKGCONF), "--cflags-only-I", "hyprland"))
    other = shlex.split(run(str(PKGCONF), "--cflags-only-other", "hyprland"))
    require(all(flag.startswith("-I") and len(flag) > 2 for flag in includes), "unexpected pkg-config include flags")
    include_dirs = [flag[2:] for flag in includes]
    # The source includes <src/...>. Native Hyprland puts its hashed protocols
    # directory before the root, then src. Require the whole leading selection
    # through the root to stay inside that hashed tree, before any external root.
    require(str(HEADER_ROOT) in include_dirs, "canonical Hyprland header root is missing")
    leading = include_dirs[:include_dirs.index(str(HEADER_ROOT)) + 1]
    require(all(Path(name).is_relative_to(HEADER_ROOT) for name in leading), "Hyprland headers are not first in pkg-config include selection")
    for name in include_dirs:
        path = Path(name)
        require(path.is_dir() and path.resolve() == path and path.is_relative_to(SYSTEM_INCLUDE), "noncanonical pkg-config include path")
    require([flag for flag in cflags if flag.startswith("-I")] == includes and
            [flag for flag in cflags if not flag.startswith("-I")] == other, "inconsistent pkg-config flags")
    verify_flags(other, "pkg-config CFLAGS_OTHER")
    libraries = shlex.split(run(str(PKGCONF), "--libs", "hyprland"))
    return {"executable": str(PKGCONF), "executable_sha256": digest(PKGCONF),
            "pc_path": str(HYPRLAND_PC), "pc_sha256": digest(HYPRLAND_PC),
            "cflags": cflags, "include_dirs": include_dirs, "cflags_other": other, "ldflags": libraries}


def header_inventory_sha256(root=Path("/usr/include/hyprland")):
    require(root.is_dir() and not root.is_symlink(), "missing canonical Hyprland headers")
    prefix = str(root) + "/"
    packaged = {name[len(prefix):] for name in run("pacman", "-Qlq", "hyprland").splitlines()
                if name.startswith(prefix) and not name.endswith("/")}
    actual = {}
    for path in root.rglob("*"):
        require(not path.is_symlink() and (path.is_file() or path.is_dir()), "nonregular Hyprland header entry")
        if path.is_file():
            actual[path.relative_to(root).as_posix()] = digest(path)
    require(actual and set(actual) == packaged, "Hyprland package header inventory mismatch")
    return sha256(json_bytes(actual))


def verify_build(build, source, cxx, profile):
    verify_build_environment()
    pkgconfig = pkgconfig_selection(profile)
    cache = {}
    for line in (build / "CMakeCache.txt").read_text().splitlines():
        match = re.match(r"([^:#/][^:]*):[^=]+=(.*)", line)
        if match:
            require(match[1] not in cache, "duplicate CMake cache entry")
            cache[match[1]] = match[2]
    expected = dict(OPTIONS, BUILD_TESTING="ON", CUA_HYPRLAND_BUILD_PLUGIN="ON", CMAKE_BUILD_TYPE="Release",
                    CMAKE_GENERATOR="Ninja",
                    PKG_CONFIG_EXECUTABLE=str(PKGCONF), PKG_CONFIG_ARGN="", PKG_CONFIG_USE_CMAKE_PREFIX_PATH="OFF",
                    HYPRLAND_VERSION=profile["hyprland"]["header_version"],
                    HYPRLAND_CFLAGS=";".join(pkgconfig["cflags"]),
                    HYPRLAND_INCLUDE_DIRS=";".join(pkgconfig["include_dirs"]),
                    HYPRLAND_CFLAGS_OTHER=";".join(pkgconfig["cflags_other"]),
                    HYPRLAND_LDFLAGS=";".join(pkgconfig["ldflags"]),
                    CUA_HYPRLAND_EXPECTED_VERSION=profile["hyprland"]["header_version"],
                    CUA_HYPRLAND_TEST_OPERATOR_KEY="", CMAKE_CXX_COMPILER=str(cxx),
                    CMAKE_HOME_DIRECTORY=str(source.resolve()))
    for name, value in expected.items():
        require(cache.get(name) == value, f"build configuration mismatch: {name}")
    for name in EMPTY_CMAKE_ROUTING:
        require(not cache.get(name), f"CMake routing override refused: {name}")
    for name, value in cache.items():
        if name.startswith("CMAKE_PROJECT_") and name.endswith(("_INCLUDE", "_INCLUDE_BEFORE", "_TOP_LEVEL_INCLUDES")):
            require(not value, f"CMake routing override refused: {name}")
        if name.startswith(("CMAKE_CXX_FLAGS", "CMAKE_EXE_LINKER_FLAGS", "CMAKE_MODULE_LINKER_FLAGS", "CMAKE_SHARED_LINKER_FLAGS")):
            verify_flags(shlex.split(value), name)
    module = build / "cua-hyprland-plugin.so"
    elf_comment(module, profile["compiler"]["comment"])
    linked_runtime(module, profile)
    return digest(module)


def verify_consumer(module, kit, profile, provenance):
    build = read_json((kit / "BUILD-PROVENANCE.json").read_bytes())
    manifest = source_manifest((kit / "SOURCE-PROVENANCE.json").read_bytes(), profile)
    require(build["source"] == manifest and build["profile"] == profile and build["kit"] == provenance, "installed provenance identity mismatch")
    require(module.is_file() and not module.is_symlink() and digest(module) == build["module_sha256"], "installed module checksum mismatch")
    require(build["compiler_sha256"] == profile["compiler"]["sha256"] and build["compiler_version"] == profile["compiler"]["version"] and build["compiler_probe_comment"] == profile["compiler"]["comment"], "installed compiler provenance mismatch")
    require(build["compositor_sha256"] == profile["hyprland"]["sha256"], "installed compositor provenance mismatch")
    runtime_sha = profile["runtime"]["sha256"]
    require(all(build[key] == runtime_sha for key in ("compiler_runtime_sha256", "compositor_runtime_sha256", "module_runtime_sha256")), "installed runtime provenance mismatch")
    verify_environment(profile)
    elf_comment(module, profile["compiler"]["comment"])
    linked_runtime(module, profile)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--kit", required=True, type=Path)
    parser.add_argument("--kit-sha256", required=True, help="reviewed KIT-PROVENANCE.json SHA-256")
    parser.add_argument("--source", type=Path)
    parser.add_argument("--archive", type=Path)
    parser.add_argument("--cxx", type=Path)
    parser.add_argument("--build", type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--consumer", type=Path, help="installed module; requires no compiler or headers")
    args = parser.parse_args()
    try:
        profile, provenance = verify_kit(args.kit, args.kit_sha256)
        if args.consumer:
            require(not any((args.source, args.archive, args.cxx, args.build, args.output)), "consumer mode cannot take build inputs")
            verify_consumer(args.consumer, args.kit, profile, provenance)
            print("Passed installed profile compatibility checks; live activation remains separate.")
            return
        require(args.source and args.archive and args.cxx, "build verification requires --source, --archive and --cxx")
        archive_manifest = verify_archive(args.archive, profile)
        manifest = verify_source(args.source, profile)
        require(manifest == archive_manifest, "extracted source differs from archive")
        native = verify_native(args.cxx, profile)
        if args.build:
            native["module_sha256"] = verify_build(args.build, args.source, args.cxx, profile)
            native["module_runtime_sha256"] = profile["runtime"]["sha256"]
        if args.output:
            require(args.build is not None, "build evidence is required for output")
            args.output.write_bytes(json_bytes(dict(native, source=manifest, profile=profile, kit=provenance)))
    except (ValueError, KeyError, TypeError, OSError, tarfile.TarError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"error: {error}\n")


if __name__ == "__main__":
    main()
