#!/usr/bin/env python3
"""Build the private Cua Driver wheel from an immutable native checkout."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import shutil
import subprocess
import sys
import tempfile
import tomllib
from pathlib import Path

from pydantic import BaseModel, ConfigDict

PUBLIC_VERSION = "0.19.3"
PRIVATE_DISTRIBUTION = "cua-driver-hcomp"
SOURCE_SHA_PATTERN = re.compile(r"^[0-9a-f]{40}$")


class WheelProvenance(BaseModel):
    """Provenance embedded in the private wheel."""

    model_config = ConfigDict(extra="forbid")

    distribution: str
    version: str
    source_sha: str
    packaging_sha: str
    platform: str
    architecture: str
    executable_sha256: str
    sdk_sha256: str


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as file:
        for chunk in iter(lambda: file.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def private_version(source_sha: str) -> str:
    validate_sha("source SHA", source_sha)
    return f"{PUBLIC_VERSION}+hcomp.{source_sha[:12]}"


def validate_sha(label: str, value: str) -> None:
    if not SOURCE_SHA_PATTERN.fullmatch(value):
        raise ValueError(f"{label} must be a lowercase 40-character Git SHA")


def git_output(source_dir: Path, *args: str) -> str:
    return subprocess.run(
        ["git", *args],
        cwd=source_dir,
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()


def verify_source_checkout(source_dir: Path, source_sha: str) -> None:
    actual_sha = git_output(source_dir, "rev-parse", "HEAD")
    if actual_sha != source_sha:
        raise ValueError(f"source checkout is {actual_sha}, expected {source_sha}")
    dirty = git_output(source_dir, "status", "--porcelain", "--untracked-files=no")
    if dirty:
        raise ValueError("native source checkout has tracked modifications")


def replace_project_identity(package_dir: Path, version: str) -> None:
    pyproject = package_dir / "pyproject.toml"
    text = pyproject.read_text(encoding="utf-8")
    text, name_count = re.subn(
        r'(?m)^name = "cua-driver"$',
        f'name = "{PRIVATE_DISTRIBUTION}"',
        text,
    )
    text, version_count = re.subn(
        r'(?m)^version = "[^"]+"$',
        f'version = "{version}"',
        text,
        count=1,
    )
    if name_count != 1 or version_count != 1:
        raise ValueError("public project identity did not match the expected package metadata")
    pyproject.write_text(text, encoding="utf-8")

    init_path = package_dir / "src" / "cua_driver" / "__init__.py"
    init_text, init_count = re.subn(
        r'(?m)^__version__ = "[^"]+"',
        f'__version__ = "{version}"',
        init_path.read_text(encoding="utf-8"),
        count=1,
    )
    if init_count != 1:
        raise ValueError("could not set cua_driver.__version__ in the staging tree")
    init_path.write_text(init_text, encoding="utf-8")


def verify_staged_identity(package_dir: Path, version: str) -> None:
    project = tomllib.loads((package_dir / "pyproject.toml").read_text(encoding="utf-8"))["project"]
    if project["name"] != PRIVATE_DISTRIBUTION:
        raise ValueError("staged distribution identity is not private")
    if project["version"] != version:
        raise ValueError("staged version does not match the source SHA")
    if project.get("scripts", {}).get("cua-driver") != "cua_driver.__main__:main":
        raise ValueError("staged wheel does not preserve the cua-driver console script")
    packages = project_path(package_dir, "tool", "hatch", "build", "targets", "wheel", "packages")
    if packages != ["src/cua_driver"]:
        raise ValueError("staged wheel does not preserve the cua_driver import package")


def project_path(package_dir: Path, *keys: str) -> object:
    value: object = tomllib.loads((package_dir / "pyproject.toml").read_text(encoding="utf-8"))
    for key in keys:
        if not isinstance(value, dict) or key not in value:
            raise ValueError(f"missing pyproject metadata: {'.'.join(keys)}")
        value = value[key]
    return value


def native_payload(package_dir: Path) -> tuple[Path, Path]:
    package = package_dir / "src" / "cua_driver"
    executable = package / "bin" / ("cua-driver.exe" if sys.platform == "win32" else "cua-driver")
    sdk_names = {
        "darwin": "libcua_driver_sdk.dylib",
        "linux": "libcua_driver_sdk.so",
        "win32": "cua_driver_sdk.dll",
    }
    sdk_name = sdk_names.get(sys.platform)
    if sdk_name is None:
        raise ValueError(f"unsupported build platform: {sys.platform}")
    sdk = package / sdk_name
    for path in (executable, sdk):
        if not path.is_file():
            raise FileNotFoundError(f"required native payload is missing: {path}")
    return executable, sdk


def stage_package(
    source_dir: Path,
    staging_dir: Path,
    source_sha: str,
    packaging_sha: str,
    architecture: str,
) -> WheelProvenance:
    validate_sha("packaging SHA", packaging_sha)
    version = private_version(source_sha)
    source_package = source_dir / "libs" / "cua-driver" / "python"
    shutil.copytree(
        source_package,
        staging_dir,
        ignore=shutil.ignore_patterns("dist", "build", "*.egg-info", "__pycache__"),
        dirs_exist_ok=True,
    )
    replace_project_identity(staging_dir, version)
    verify_staged_identity(staging_dir, version)
    executable, sdk = native_payload(staging_dir)
    provenance = WheelProvenance(
        distribution=PRIVATE_DISTRIBUTION,
        version=version,
        source_sha=source_sha,
        packaging_sha=packaging_sha,
        platform=sys.platform,
        architecture=architecture,
        executable_sha256=sha256_file(executable),
        sdk_sha256=sha256_file(sdk),
    )
    provenance_path = staging_dir / "src" / "cua_driver" / "hcomp_build.json"
    provenance_path.write_text(
        json.dumps(provenance.model_dump(), indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    return provenance


def build_private_wheel(
    source_dir: Path,
    source_sha: str,
    packaging_sha: str,
    architecture: str,
    output_dir: Path,
) -> Path:
    verify_source_checkout(source_dir, source_sha)
    output_dir.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="cua-driver-hcomp-") as temporary:
        staging_dir = Path(temporary) / "package"
        provenance = stage_package(
            source_dir,
            staging_dir,
            source_sha,
            packaging_sha,
            architecture,
        )
        subprocess.run(
            [
                sys.executable,
                "build_wheel.py",
                "--version",
                provenance.version,
                "--arch",
                architecture,
                "--skip-download",
            ],
            cwd=staging_dir,
            check=True,
        )
        wheels = list((staging_dir / "dist").glob("*.whl"))
        if len(wheels) != 1:
            raise RuntimeError(f"expected one wheel, found {len(wheels)}")
        destination = output_dir / wheels[0].name
        shutil.copy2(wheels[0], destination)
        return destination


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--source-dir", type=Path, required=True)
    parser.add_argument("--source-sha", required=True)
    parser.add_argument("--packaging-sha", required=True)
    parser.add_argument("--arch", required=True)
    parser.add_argument("--output-dir", type=Path, default=Path("dist"))
    parser.add_argument("--skip-download", action="store_true", required=True)
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    wheel = build_private_wheel(
        args.source_dir.resolve(),
        args.source_sha,
        args.packaging_sha,
        args.arch,
        args.output_dir.resolve(),
    )
    print(wheel)


if __name__ == "__main__":
    main()
