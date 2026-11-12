"""Validate the exact published Sandbox release and its Python distributions."""

import email.parser
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tarfile
import tomllib
import zipfile


PACKAGE_PATH = "libs/python/cua-sandbox"
STABLE_TAG = re.compile(r"sandbox-v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)")


def version_from_tag(tag):
    if not STABLE_TAG.fullmatch(tag):
        raise ValueError("Expected a stable sandbox-vX.Y.Z release tag")
    return tag.removeprefix("sandbox-v")


def validate_release(tag, release):
    version = version_from_tag(tag)
    if (
        release.get("tag_name") != tag
        or release.get("draft") is not False
        or release.get("prerelease") is not False
        or not release.get("published_at")
    ):
        raise ValueError("The exact Sandbox release must be published, non-draft and stable")
    return version


def validate_versions(version, manifest, project):
    if manifest.get(PACKAGE_PATH) != version:
        raise ValueError("Release manifest version does not match the Sandbox tag")
    if project.get("name") != "cua-sandbox" or project.get("version") != version:
        raise ValueError("Sandbox package name/version does not match the release")


def command(*args):
    return subprocess.check_output(args, text=True).strip()


def validate_source_versions(version, authority, runtime, lock):
    if authority.strip() != version:
        raise ValueError("Sandbox VERSION does not match the release")
    runtime_versions = re.findall(r'^__version__\s*=\s*"([^"]+)"', runtime, re.MULTILINE)
    if runtime_versions != [version]:
        raise ValueError("Sandbox runtime version does not match the release")
    packages = [entry for entry in lock.get("package", []) if entry.get("name") == "cua-sandbox"]
    if (
        len(packages) != 1
        or packages[0].get("version") != version
        or packages[0].get("source") != {"editable": "."}
    ):
        raise ValueError("Sandbox uv.lock editable root does not match the release")


def resolve(tag, repository):
    version_from_tag(tag)
    release = json.loads(command("gh", "api", f"repos/{repository}/releases/tags/{tag}"))
    version = validate_release(tag, release)
    shallow = command("git", "rev-parse", "--is-shallow-repository") == "true"
    command(
        "git",
        "fetch",
        "--no-tags",
        *(["--unshallow"] if shallow else []),
        "origin",
        "+refs/heads/main:refs/remotes/origin/main",
        f"+refs/tags/{tag}:refs/tags/{tag}",
    )
    sha = command("git", "rev-parse", f"refs/tags/{tag}^{{commit}}")
    if not re.fullmatch(r"[0-9a-f]{40}", sha):
        raise ValueError("Release tag did not resolve to a commit SHA")
    command("git", "merge-base", "--is-ancestor", sha, "refs/remotes/origin/main")
    manifest = json.loads(command("git", "show", f"{sha}:.release-please-manifest.json"))
    project = tomllib.loads(command("git", "show", f"{sha}:{PACKAGE_PATH}/pyproject.toml"))
    validate_versions(version, manifest, project["project"])
    validate_source_versions(
        version,
        command("git", "show", f"{sha}:{PACKAGE_PATH}/VERSION"),
        command("git", "show", f"{sha}:{PACKAGE_PATH}/cua_sandbox/__init__.py"),
        tomllib.loads(command("git", "show", f"{sha}:{PACKAGE_PATH}/uv.lock")),
    )
    return version, sha


def validate_metadata(raw, version):
    metadata = email.parser.BytesParser().parsebytes(raw)
    names, versions = metadata.get_all("Name", []), metadata.get_all("Version", [])
    if len(names) != 1 or re.sub(r"[-_.]+", "-", names[0]).lower() != "cua-sandbox":
        raise ValueError("Distribution metadata must identify cua-sandbox")
    if versions != [version]:
        raise ValueError("Distribution metadata version does not match the release")


def validate_artifacts(directory, version):
    version_from_tag(f"sandbox-v{version}")
    files = list(Path(directory).iterdir())
    wheels = [path for path in files if path.name.endswith(".whl")]
    sdists = [path for path in files if path.name.endswith(".tar.gz")]
    if len(files) != 2 or len(wheels) != 1 or len(sdists) != 1:
        raise ValueError("Expected exactly one wheel and one source distribution")
    with zipfile.ZipFile(wheels[0]) as wheel:
        entries = [name for name in wheel.namelist() if name.endswith(".dist-info/METADATA")]
        if len(entries) != 1:
            raise ValueError("Wheel must contain exactly one package metadata file")
        validate_metadata(wheel.read(entries[0]), version)
    with tarfile.open(sdists[0], "r:gz") as sdist:
        entries = [
            entry
            for entry in sdist.getmembers()
            if len(entry.name.split("/")) == 2
            and entry.name.endswith("/PKG-INFO")
            and entry.isfile()
        ]
        if len(entries) != 1:
            raise ValueError("Source distribution must contain one root PKG-INFO")
        with sdist.extractfile(entries[0]) as metadata:
            validate_metadata(metadata.read(), version)


if __name__ == "__main__":
    if sys.argv[1:] == ["resolve"]:
        version, sha = resolve(os.environ["RELEASE_TAG"], os.environ["GITHUB_REPOSITORY"])
        with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as output:
            output.write(f"version={version}\nsha={sha}\n")
    elif len(sys.argv) == 3 and sys.argv[1] == "artifacts":
        validate_artifacts(sys.argv[2], os.environ["RELEASE_VERSION"])
    else:
        raise SystemExit("Usage: resolve_sandbox_release.py resolve | artifacts DIST_DIRECTORY")
