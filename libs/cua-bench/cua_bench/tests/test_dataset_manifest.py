"""Dataset manifest contract tests with independent tampering examples."""
import importlib.util
from pathlib import Path
import sys

import pytest

SCRIPT = Path(__file__).resolve().parents[2] / "scripts" / "dataset_manifest.py"
spec = importlib.util.spec_from_file_location("dataset_manifest", SCRIPT)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


def dataset(tmp_path):
    root = tmp_path / "dataset"
    for name in ("click", "type"):
        task = root / name
        task.mkdir(parents=True)
        (task / "main.py").write_text("# task\n")
        (task / "asset.txt").write_text("asset")
    return root


def test_manifest_is_deterministic(tmp_path):
    root = dataset(tmp_path)
    first = module.scan_dataset(root)
    assert first == module.scan_dataset(root)
    assert module.verify_dataset(root, first) == 2
    assert [x["task"] for x in first["tasks"]] == ["click", "type"]


@pytest.mark.parametrize("mutation", ["change", "add", "remove"])
def test_dataset_drift_fails_closed(tmp_path, mutation):
    root = dataset(tmp_path)
    saved = module.scan_dataset(root)
    asset = root / "click" / "asset.txt"
    if mutation == "change":
        asset.write_text("tampered")
    elif mutation == "add":
        (root / "click" / "new.txt").write_text("extra")
    else:
        asset.unlink()
    with pytest.raises(ValueError, match="dataset differs"):
        module.verify_dataset(root, saved)


def test_new_task_fails_closed(tmp_path):
    root = dataset(tmp_path)
    saved = module.scan_dataset(root)
    extra = root / "new"
    extra.mkdir()
    (extra / "main.py").write_text("# inserted task")
    with pytest.raises(ValueError, match="dataset differs"):
        module.verify_dataset(root, saved)


def test_symlink_escape_refused(tmp_path):
    root = dataset(tmp_path)
    external = tmp_path / "secret.txt"
    external.write_text("secret")
    (root / "click" / "secret-link.txt").symlink_to(external)
    with pytest.raises(ValueError, match="symlink"):
        module.scan_dataset(root)


def test_manifest_never_executes_task_source(tmp_path):
    root = dataset(tmp_path)
    marker = tmp_path / "executed"
    (root / "click" / "main.py").write_text(f"open({str(marker)!r}, 'w').write('unsafe')")
    module.scan_dataset(root)
    assert not marker.exists()


def test_missing_tasks_refused(tmp_path):
    empty = tmp_path / "empty"
    empty.mkdir()
    with pytest.raises(ValueError, match="no task"):
        module.scan_dataset(empty)
