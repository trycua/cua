"""Validate pytest-json-report output without trusting a green CI label alone.

This is a structural receipt check, not independent task-oracle attestation.
"""
import argparse
import json
from pathlib import Path


def verify_report(doc):
    if not isinstance(doc, dict):
        raise ValueError("report must be an object")
    tests = doc.get("tests")
    summary = doc.get("summary")
    if not isinstance(tests, list) or not tests:
        raise ValueError("no executed test records")
    if not isinstance(summary, dict):
        raise ValueError("missing summary")
    if type(doc.get("exitcode")) is not int or doc["exitcode"] != 0:
        raise ValueError("pytest did not exit successfully")
    nodeids = [x.get("nodeid") for x in tests if isinstance(x, dict)]
    if len(nodeids) != len(tests) or any(not isinstance(x, str) or not x for x in nodeids):
        raise ValueError("invalid test identity")
    if len(nodeids) != len(set(nodeids)):
        raise ValueError("duplicate test identities")
    if any(x.get("outcome") != "passed" for x in tests):
        raise ValueError("not all test outcomes passed")
    if any(summary.get(k, 0) for k in ("failed", "error", "skipped", "xfailed", "xpassed", "deselected")):
        raise ValueError("report contains non-passing or unexecuted tests")
    if summary.get("passed") != len(tests) or summary.get("total") != len(tests):
        raise ValueError("reported totals disagree with test records")
    if summary.get("collected", len(tests)) != len(tests):
        raise ValueError("collection count disagrees with test records")
    if any(x.get("outcome") == "failed" for x in doc.get("collectors", []) if isinstance(x, dict)):
        raise ValueError("collection failure")
    return {"passed": len(tests), "distinct_tests": len(nodeids), "report_valid": True}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("reports", nargs="+", type=Path)
    args = parser.parse_args()
    for path in args.reports:
        print(json.dumps({"file": str(path), **verify_report(json.loads(path.read_text()))}))


if __name__ == "__main__":
    main()
