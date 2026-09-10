#!/usr/bin/env python3
"""Wrap unchanged Driver 0.24.0 source in a separately reviewed native-profile kit."""

import argparse
import gzip
import io
from pathlib import Path
import re
import subprocess
import tarfile

import profile_verify as verify

RELEASE = "libs/cua-driver/hyprland-plugin/packaging/release/"


def committed_file(repo, revision, name):
    path = RELEASE + name
    entry = subprocess.check_output(["git", "-C", str(repo), "ls-tree", revision, "--", path], text=True)
    verify.require(entry.startswith(("100644 blob ", "100755 blob ")), f"missing committed tooling file: {name}")
    return subprocess.check_output(["git", "-C", str(repo), "show", f"{revision}:{path}"])


def deterministic_archive(payload):
    raw = io.BytesIO()
    with tarfile.open(fileobj=raw, mode="w", format=tarfile.USTAR_FORMAT) as archive:
        for name, data in sorted(payload.items()):
            info = tarfile.TarInfo(name)
            info.size, info.mode, info.mtime = len(data), 0o644, 0
            archive.addfile(info, io.BytesIO(data))
    compressed = io.BytesIO()
    with gzip.GzipFile(filename="", fileobj=compressed, mode="wb", mtime=0, compresslevel=9) as archive:
        archive.write(raw.getvalue())
    return compressed.getvalue()


def generate(repo, tooling_revision, profile_path, source_archive, output):
    verify.require(re.fullmatch(r"[0-9a-f]{40}", tooling_revision), "requires a full tooling commit SHA")
    resolved = subprocess.check_output(["git", "-C", str(repo), "rev-parse", f"{tooling_revision}^{{commit}}"], text=True).strip()
    verify.require(resolved == tooling_revision, "tooling revision must name a commit")
    payload = {name: committed_file(repo, tooling_revision, name) for name in verify.TOOLING}
    # Do not validate with dirty or differently versioned generator/verifier code.
    for name in ("profile_bundle.py", "profile_verify.py"):
        verify.require(payload[name] == (Path(__file__).parent / name).read_bytes(), f"executing tooling differs from commit: {name}")
    verify.require(profile_path.is_file() and not profile_path.is_symlink(), "profile must be an explicit regular file")
    profile_data = profile_path.read_bytes()
    profile = verify.validate_profile(verify.read_json(profile_data))
    manifest = verify.verify_archive(source_archive, profile)
    provenance = {"schema": 1, "tooling_revision": tooling_revision, "profile_sha256": verify.sha256(profile_data),
                  "source": profile["source"], "cmake_options": verify.OPTIONS, "native_certified": False,
                  "tooling_files": {name: verify.sha256(data) for name, data in payload.items()}}
    payload["PROFILE.json"] = profile_data
    payload["KIT-PROVENANCE.json"] = verify.json_bytes(provenance)
    payload["SOURCE-PROVENANCE.json"] = verify.json_bytes(manifest)
    # Preserve original manifest bytes too, even if its JSON formatting differs.
    with tarfile.open(source_archive, "r:gz") as archive:
        payload["SOURCE-PROVENANCE.json"] = archive.extractfile(verify.STEM + "/SOURCE-PROVENANCE.json").read()
    payload[verify.STEM + ".tar.gz"] = source_archive.read_bytes()
    verify.require(verify.sha256(payload[verify.STEM + ".tar.gz"]) == profile["source"]["archive_sha256"], "source archive changed during generation")
    verify.source_manifest(payload["SOURCE-PROVENANCE.json"], profile)
    payload["PKGBUILD"] = verify.render_recipe(payload["PROFILE-PKGBUILD.in"].decode(), profile, provenance)
    payload["SHA256SUMS"] = "".join(f"{verify.sha256(data)}  {name}\n" for name, data in sorted(payload.items())).encode()
    # The archive identity binds profile bytes and tooling commit, not just labels.
    name = (f"{verify.STEM}-profile-{profile['profile_id']}-kit-{profile['kit_version']}"
            f"-{provenance['profile_sha256']}-{tooling_revision}.tar.gz")
    archive_data = deterministic_archive(payload)
    output.mkdir(parents=True, exist_ok=False)
    (output / name).write_bytes(archive_data)
    (output / (name + ".sha256")).write_text(f"{verify.sha256(archive_data)}  {name}\n")
    return provenance


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", required=True, type=Path)
    parser.add_argument("--tooling-revision", required=True)
    parser.add_argument("--profile", required=True, type=Path)
    parser.add_argument("--source-archive", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path, help="new output directory")
    args = parser.parse_args()
    try:
        generate(args.repo.resolve(strict=True), args.tooling_revision, args.profile, args.source_archive, args.output)
    except (ValueError, KeyError, TypeError, OSError, tarfile.TarError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"error: {error}\n")


if __name__ == "__main__":
    main()
