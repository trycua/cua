"""Regression tests for the private wheel identity and provenance boundary."""

from __future__ import annotations

import json
import sys
import zipfile
from pathlib import Path

import pytest

SCRIPTS = Path(__file__).resolve().parents[1] / "scripts"
sys.path.insert(0, str(SCRIPTS))

from build_hcomp_private_wheel import (  # noqa: E402
    PRIVATE_DISTRIBUTION,
    private_version,
    stage_package,
)
from gate_list_tools_schema import (
    WheelGateError,
    sha256_bytes,
    verify_wheel_provenance,
)  # noqa: E402

SOURCE_SHA = "8662ab0e110fa2dee791a6a27f4726e404a6568c"
PACKAGING_SHA = "1" * 40


def write_source_package(root: Path) -> Path:
    package_dir = root / "libs" / "cua-driver" / "python"
    package = package_dir / "src" / "cua_driver"
    (package / "bin").mkdir(parents=True)
    (package / "__init__.py").write_text('__version__ = "0.19.3"\n', encoding="utf-8")
    (package / "bin" / "cua-driver").write_bytes(b"executable")
    sdk_name = "libcua_driver_sdk.dylib" if sys.platform == "darwin" else "libcua_driver_sdk.so"
    (package / sdk_name).write_bytes(b"sdk")
    (package_dir / "pyproject.toml").write_text(
        """
[project]
name = "cua-driver"
version = "0.19.3"
[project.scripts]
cua-driver = "cua_driver.__main__:main"
[tool.hatch.build.targets.wheel]
packages = ["src/cua_driver"]
""".lstrip(),
        encoding="utf-8",
    )
    return package_dir


def test_staging_uses_private_distribution_without_renaming_python_api(tmp_path: Path) -> None:
    source_package = write_source_package(tmp_path / "source")
    staging = tmp_path / "staging"

    provenance = stage_package(
        tmp_path / "source",
        staging,
        SOURCE_SHA,
        PACKAGING_SHA,
        "universal" if sys.platform == "darwin" else "x86_64",
    )

    staged_pyproject = (staging / "pyproject.toml").read_text(encoding="utf-8")
    assert f'name = "{PRIVATE_DISTRIBUTION}"' in staged_pyproject
    assert 'cua-driver = "cua_driver.__main__:main"' in staged_pyproject
    assert 'packages = ["src/cua_driver"]' in staged_pyproject
    assert provenance.version == private_version(SOURCE_SHA)
    assert 'name = "cua-driver"' in (source_package / "pyproject.toml").read_text(encoding="utf-8")


def write_test_wheel(path: Path, *, version: str, sdk_hash: str) -> None:
    executable = b"executable"
    sdk = b"sdk"
    provenance = {
        "distribution": PRIVATE_DISTRIBUTION,
        "version": version,
        "source_sha": SOURCE_SHA,
        "packaging_sha": PACKAGING_SHA,
        "platform": "linux",
        "architecture": "x86_64",
        "executable_sha256": sha256_bytes(executable),
        "sdk_sha256": sdk_hash,
    }
    dist_info = "cua_driver_hcomp-0.19.3.dist-info"
    with zipfile.ZipFile(path, "w") as archive:
        archive.writestr("cua_driver/hcomp_build.json", json.dumps(provenance))
        archive.writestr("cua_driver/bin/cua-driver", executable)
        archive.writestr("cua_driver/libcua_driver_sdk.so", sdk)
        archive.writestr(
            f"{dist_info}/METADATA",
            f"Metadata-Version: 2.3\nName: {PRIVATE_DISTRIBUTION}\nVersion: {version}\n",
        )
        archive.writestr(
            f"{dist_info}/entry_points.txt",
            "[console_scripts]\ncua-driver = cua_driver.__main__:main\n",
        )


@pytest.mark.parametrize(
    ("version", "sdk_hash", "message"),
    [
        ("0.19.3+hcomp.000000000000", sha256_bytes(b"sdk"), "version does not match"),
        (private_version(SOURCE_SHA), "0" * 64, "SDK hash does not match"),
    ],
)
def test_provenance_tampering_is_rejected(
    tmp_path: Path,
    version: str,
    sdk_hash: str,
    message: str,
) -> None:
    wheel = tmp_path / "cua_driver_hcomp.whl"
    write_test_wheel(wheel, version=version, sdk_hash=sdk_hash)

    with pytest.raises(WheelGateError, match=message):
        verify_wheel_provenance(wheel)
