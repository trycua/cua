import importlib.util
import hashlib
import json
from pathlib import Path
import tarfile


SCRIPT = Path(__file__).resolve().parents[1] / "package_cua_driver_skills.py"
SPEC = importlib.util.spec_from_file_location("package_cua_driver_skills", SCRIPT)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def test_release_archive_contains_complete_hashed_manifest(tmp_path: Path) -> None:
    source = tmp_path / "source"
    source.mkdir()
    (source / "SKILL.md").write_text("---\nversion: 1.2.3\n---\n", encoding="utf-8")
    (source / "README.md").write_text("read me", encoding="utf-8")
    archive = tmp_path / "cua-driver-rs-v1.2.3-skills.tar.gz"
    commit = "0123456789abcdef0123456789abcdef01234567"

    manifest = MODULE.build_archive(
        source,
        archive,
        "cua-driver-rs-v1.2.3-skills",
        "1.2.3",
        "release",
        commit,
    )

    assert manifest["schema_version"] == 1
    assert manifest["compatible_driver_version"] == "1.2.3"
    assert manifest["source"] == {"kind": "release", "git_commit": commit}
    assert [entry["path"] for entry in manifest["files"]] == ["README.md", "SKILL.md"]
    assert manifest["files"][0]["sha256"] == hashlib.sha256(b"read me").hexdigest()
    with tarfile.open(archive, "r:gz") as bundle:
        names = sorted(bundle.getnames())
        assert "cua-driver-rs-v1.2.3-skills/skill-pack.json" in names
        assert all(member.uname == member.gname == "" for member in bundle.getmembers())
        assert all(member.mtime == 0 for member in bundle.getmembers())
        manifest_file = bundle.extractfile("cua-driver-rs-v1.2.3-skills/skill-pack.json")
        assert manifest_file is not None
        assert json.load(manifest_file) == manifest


def test_release_archive_rejects_version_mismatch(tmp_path: Path) -> None:
    source = tmp_path / "source"
    source.mkdir()
    (source / "SKILL.md").write_text("---\nversion: 1.2.2\n---\n", encoding="utf-8")

    try:
        MODULE.build_archive(source, tmp_path / "pack.tar.gz", "pack", "1.2.3", "release", None)
    except ValueError as error:
        assert "does not match release" in str(error)
    else:
        raise AssertionError("version mismatch should fail packaging")
