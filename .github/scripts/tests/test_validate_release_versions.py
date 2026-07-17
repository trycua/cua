from pathlib import Path
import shutil

import pytest

from validate_release_versions import VersionError, validate


REPO_ROOT = Path(__file__).resolve().parents[3]


def copy_release_sources(destination: Path) -> None:
    shutil.copy(REPO_ROOT / ".release-please-manifest.json", destination)
    for relative in ("libs/cua-driver", "libs/lume"):
        source = REPO_ROOT / relative
        target = destination / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copytree(source, target)
    docs = "docs/content/docs/reference/lume"
    shutil.copytree(REPO_ROOT / docs, destination / docs)


def test_current_release_versions_agree():
    validate(REPO_ROOT, "all")


def test_version_drift_fails_with_the_source_name(tmp_path: Path):
    copy_release_sources(tmp_path)
    path = tmp_path / "libs/lume/src/Main.swift"
    current = (tmp_path / "libs/lume/VERSION").read_text().strip()
    path.write_text(path.read_text().replace(f'"{current}"', '"9.9.9"'))
    with pytest.raises(VersionError, match="src/Main.swift=9.9.9"):
        validate(tmp_path, "lume")
