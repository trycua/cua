"""Falsification tests for checksum-linked CUA CI receipts."""
import importlib.util
import json
from pathlib import Path
import sys

import pytest

scripts = Path(__file__).resolve().parents[2] / "scripts"
sys.path.insert(0, str(scripts))
spec = importlib.util.spec_from_file_location("ci_receipt", scripts / "ci_receipt.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


def report(nodeid):
    return {
        "exitcode": 0,
        "summary": {"collected": 1, "total": 1, "passed": 1},
        "tests": [{"nodeid": nodeid, "outcome": "passed"}],
        "collectors": [],
    }


def setup_reports(tmp_path):
    a, b = tmp_path / "core-report.json", tmp_path / "full-report.json"
    a.write_text(json.dumps(report("core::one")))
    b.write_text(json.dumps(report("full::one")))
    return a, b


def test_valid_receipt(tmp_path):
    a, b = setup_reports(tmp_path)
    receipt = module.build_receipt(a, b, "a" * 40)
    assert module.verify_receipt(receipt, tmp_path) is True
    assert receipt["reports"][0]["sha256"] != receipt["reports"][1]["sha256"]


def test_modified_report_is_rejected(tmp_path):
    a, b = setup_reports(tmp_path)
    receipt = module.build_receipt(a, b, "a" * 40)
    b.write_text(json.dumps(report("full::changed")))
    with pytest.raises(ValueError, match="checksum mismatch"):
        module.verify_receipt(receipt, tmp_path)


def test_invalid_commit_is_rejected(tmp_path):
    a, b = setup_reports(tmp_path)
    from jsonschema import ValidationError
    with pytest.raises(ValidationError):
        module.build_receipt(a, b, "not-a-real-sha")


def test_missing_full_is_rejected(tmp_path):
    a, b = setup_reports(tmp_path)
    receipt = module.build_receipt(a, b, "a" * 40)
    receipt["reports"][1]["kind"] = "core"
    with pytest.raises(ValueError, match="exactly one"):
        module.verify_receipt(receipt, tmp_path)


def test_path_traversal_is_rejected(tmp_path):
    a, b = setup_reports(tmp_path)
    receipt = module.build_receipt(a, b, "a" * 40)
    receipt["reports"][1]["path"] = "../outside.json"
    with pytest.raises(ValueError, match="basenames"):
        module.verify_receipt(receipt, tmp_path)


def test_failed_underlying_test_is_rejected(tmp_path):
    a, b = setup_reports(tmp_path)
    bad = report("full::bad")
    bad["tests"][0]["outcome"] = "failed"
    b.write_text(json.dumps(bad))
    with pytest.raises(ValueError, match="not all test outcomes passed"):
        module.build_receipt(a, b, "a" * 40)

def test_external_consumer_verifies_downloaded_receipt(tmp_path):
    import subprocess
    import sys
    a, b = setup_reports(tmp_path)
    receipt = module.build_receipt(a, b, "b" * 40)
    path = tmp_path / "ci-receipt-v1.json"
    path.write_text(json.dumps(receipt))
    result = subprocess.run(
        [
            sys.executable, str(scripts / "ci_receipt.py"),
            "--verify", str(path), "--evidence-dir", str(tmp_path),
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr
    assert "independently verified" in result.stdout
