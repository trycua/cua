"""Build a versioned, hash-linked CI test receipt using the jsonschema project.

This witnesses archived pytest report bytes, not actual computer-use actions.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path

from jsonschema import Draft202012Validator

from verify_pytest_receipt import verify_report

SCHEMA = Path(__file__).resolve().parents[1] / "schemas" / "ci-receipt-v1.schema.json"


def build_receipt(core: Path, full: Path, commit: str):
    reports = []
    for kind, path in (("core", core), ("full", full)):
        raw = path.read_bytes()
        result = verify_report(json.loads(raw))
        reports.append({
            "kind": kind,
            "path": path.name,
            "sha256": hashlib.sha256(raw).hexdigest(),
            "passed": result["passed"],
            "distinct_tests": result["distinct_tests"],
        })
    receipt = {"schema_version": "cua-bench-ci-receipt/v1", "commit": commit, "reports": reports}
    validator = Draft202012Validator(json.loads(SCHEMA.read_text()))
    validator.validate(receipt)
    if set(x["kind"] for x in reports) != {"core", "full"}:
        raise ValueError("receipt must cover both core and full runs")
    return receipt


def verify_receipt(receipt, folder: Path):
    schema = json.loads(SCHEMA.read_text())
    Draft202012Validator(schema).validate(receipt)
    if {r["kind"] for r in receipt["reports"]} != {"core", "full"}:
        raise ValueError("receipt must contain exactly one core and full result")
    for item in receipt["reports"]:
        name = item["path"]
        # No paths outside evidence folder or URI-based remote fetches.
        if Path(name).name != name:
            raise ValueError("only local report basenames are allowed")
        raw = (folder / name).read_bytes()
        if hashlib.sha256(raw).hexdigest() != item["sha256"]:
            raise ValueError(f"report checksum mismatch: {name}")
        checked = verify_report(json.loads(raw))
        if checked["passed"] != item["passed"] or checked["distinct_tests"] != item["distinct_tests"]:
            raise ValueError(f"report summary mismatch: {name}")
    return True


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--core", type=Path, default=Path("core-report.json"))
    parser.add_argument("--full", type=Path, default=Path("full-report.json"))
    parser.add_argument("--commit", default=os.environ.get("GITHUB_SHA"))
    parser.add_argument("--output", type=Path, default=Path("ci-receipt-v1.json"))
    args = parser.parse_args()
    if not args.commit:
        parser.error("a real 40-character git commit is required")
    receipt = build_receipt(args.core, args.full, args.commit)
    verify_receipt(receipt, args.core.parent)
    args.output.write_text(json.dumps(receipt, indent=2, sort_keys=True) + "\n")
    print(f"CUA CI receipt checked: {args.output}")


if __name__ == "__main__":
    main()
