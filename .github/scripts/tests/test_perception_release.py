from __future__ import annotations

import importlib.util
import json
from pathlib import Path

import pytest


ROOT = Path(__file__).resolve().parents[3]
SPEC = importlib.util.spec_from_file_location(
    "perception_release", ROOT / ".github/scripts/perception_release.py"
)
assert SPEC and SPEC.loader
release = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(release)


def test_manifest_version_uses_crate_authority(monkeypatch, tmp_path: Path):
    version_authority = tmp_path / "crate/VERSION"
    version_authority.parent.mkdir(parents=True)
    version_authority.write_text("1.2.3\n", encoding="utf-8")
    manifest_path = tmp_path / "manifest.json"
    manifest_path.write_text(
        json.dumps(
            {
                "schemaVersion": 1,
                "component": "cua-perception",
                "version": "9.9.9",
                "driverVersion": "1.0.0",
                "sourceSha": "a" * 40,
                "target": {},
                "protocol": {},
                "artifacts": [],
                "modelLedger": {},
                "sourceLedger": {},
                "suppliedVerification": {},
            }
        ),
        encoding="utf-8",
    )
    monkeypatch.setattr(release, "VERSION_AUTHORITY", version_authority)
    monkeypatch.setattr(release, "validate_schema", lambda *_args: None)

    with pytest.raises(
        release.CandidateError,
        match=r"candidate version 9\.9\.9 differs from release authority 1\.2\.3",
    ):
        release.load_and_validate_manifest(manifest_path, tmp_path)
