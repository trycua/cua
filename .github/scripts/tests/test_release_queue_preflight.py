from __future__ import annotations

from pathlib import Path

import release_queue_preflight as rqp

ROOT = Path(__file__).resolve().parents[3]


def test_only_touched_version_files_are_checked():
    assert rqp.changed_products(["README.md"]) == []
    assert rqp.changed_products(["libs/lume/VERSION", "libs/cua-driver/rust/VERSION"]) == [
        "libs/cua-driver/rust/VERSION",
        "libs/lume/VERSION",
    ]


def test_driver_arguments_match_the_release_workflow(tmp_path: Path):
    args = rqp.collect_args(
        "libs/cua-driver/rust/VERSION",
        version="0.47.20",
        repository="trycua/cua",
        sha="a" * 40,
        output=tmp_path / "m.json",
        addenda=None,
    )
    assert args[args.index("--tag") + 1] == "cua-driver-rs-v0.47.20"
    assert args[args.index("--changelog") + 1] == "libs/cua-driver/rust/CHANGELOG.md"
    assert "--changelog-addenda" not in args
    # The release workflow's attribution preflight must use the same product definition.
    workflow = (ROOT / ".github/workflows/cd-rust-cua-driver.yml").read_text()
    for flag, value in zip(rqp.DRIVER_PATHS[::2], rqp.DRIVER_PATHS[1::2]):
        assert f"{flag} {value} " in workflow, (flag, value)


def test_addenda_are_passed_only_when_the_file_exists(tmp_path: Path):
    addenda = tmp_path / "a.json"
    kwargs = dict(version="1.0.0", repository="r/r", sha="b" * 40, output=tmp_path / "m.json")
    assert "--changelog-addenda" not in rqp.collect_args("libs/lume/VERSION", addenda=addenda, **kwargs)
    addenda.write_text("{}")
    assert "--changelog-addenda" in rqp.collect_args("libs/lume/VERSION", addenda=addenda, **kwargs)


def test_the_gate_runs_the_check_on_the_queue_commit():
    gate = (ROOT / ".github/workflows/ci-gate.yml").read_text()
    assert "release_queue_preflight.py" in gate
    assert "steps.release_queue.outputs.release == 'true'" in gate
    assert "--head-sha \"$HEAD_SHA\"" in gate
    metadata = (ROOT / ".github/workflows/ci-release-metadata.yml").read_text()
    assert "release_queue_preflight.py" in metadata
