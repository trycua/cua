#!/usr/bin/env python3
"""Independent final-state evaluator for CDB-S03."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import sys
import xml.etree.ElementTree as ET
import zipfile
from decimal import Decimal, InvalidOperation
from fractions import Fraction
from pathlib import Path

from gnucash import read_book
from ods import Cell, read_tables, values

HERE = Path(__file__).resolve().parent
BUNDLE = HERE.parent
FIXTURE = BUNDLE / "fixture"
ORACLE = json.loads((HERE / "oracle.json").read_text(encoding="utf-8"))
BOOK_REL = Path(ORACLE["book_file"])
WORKBOOK_REL = Path(ORACLE["workbook_file"])
CHECKS = [
    ("ledger_balanced", 15),
    ("supported_matches", 20),
    ("exact_discrepancies_flagged", 20),
    ("unmatched_preserved", 15),
    ("workbook_agreement", 15),
    ("source_preserved", 5),
    ("preservation", 10),
]
IGNORED_OS_METADATA = {".ds_store", "desktop.ini", "thumbs.db"}


def _failure(reason: str) -> dict:
    return {"passed": False, "reason": reason}


def load_book(path: Path) -> tuple[dict | None, str | None]:
    try:
        return read_book(path), None
    except (OSError, EOFError, ValueError, TypeError, ET.ParseError) as error:
        return None, f"{type(error).__name__}: {error}"


def load_workbook(path: Path) -> tuple[dict[str, list[list[Cell]]] | None, str | None]:
    try:
        return read_tables(path), None
    except (OSError, ValueError, TypeError, ET.ParseError, zipfile.BadZipFile, RuntimeError) as error:
        return None, f"{type(error).__name__}: {error}"


def checking_split(book: dict, number: str) -> dict:
    transaction = book["transactions"][number]
    matches = [split for split in transaction["splits"] if book["accounts"][split["account"]]["type"] == "BANK"]
    if len(matches) != 1:
        raise ValueError(f"{number} does not have exactly one checking split")
    return matches[0]


def transaction_amount(book: dict, number: str) -> Fraction:
    return abs(checking_split(book, number)["value"])


def money_fraction(value: str) -> Fraction:
    whole, cents = value.split(".")
    return Fraction(int(whole) * 100 + int(cents), 100)


def check_balanced(book: dict) -> dict:
    problems = []
    if not book["transactions"]:
        problems.append("book contains no transactions")
    for number, transaction in book["transactions"].items():
        if not transaction["balanced"]:
            problems.append(f"unbalanced transaction: {number}")
        for split in transaction["splits"]:
            if split["value"] != split["quantity"]:
                problems.append(f"value/quantity mismatch: {number}/{split['id']}")
    return {"passed": not problems, "problems": problems, "transactions": len(book["transactions"])}


def _states(book: dict, number: str) -> list[str]:
    return [split["state"] for split in book["transactions"][number]["splits"]]


def _memo_matches(number: str, memos: set[str]) -> bool:
    if len(memos) != 1:
        return False
    normalized = re.sub(r"[^a-z0-9.]+", " ", next(iter(memos)).casefold().replace(",", "")).strip()
    if number == "GNC-514":
        expected = "discrepancy ocp 4827 amount mismatch book 864.50 vs source 846.50"
    elif number == "GNC-519":
        expected = "duplicate svl 119 already matched to gnc 518 left unreconciled"
    else:
        return False
    return normalized == expected


def check_supported_matches(book: dict) -> dict:
    problems = []
    for number in ORACLE["allowed_reconciled"]:
        expected = ORACLE["expected"][number]
        try:
            if transaction_amount(book, number) != money_fraction(expected["amount"]):
                problems.append(f"{number} amount changed")
            checking = checking_split(book, number)
            if checking["state"] != "y":
                problems.append(f"{number} checking split is not reconciled")
            if any(split["state"] != "n" for split in book["transactions"][number]["splits"] if split is not checking):
                problems.append(f"{number} non-checking split must remain unreconciled")
            if any(split["reconcile_date"] for split in book["transactions"][number]["splits"] if split is not checking):
                problems.append(f"{number} reconcile date is attached to the wrong split")
            if any(split["memo"] for split in book["transactions"][number]["splits"]):
                problems.append(f"{number} exact match must not gain a memo")
        except (KeyError, TypeError, ValueError) as error:
            problems.append(f"malformed {number}: {type(error).__name__}")
    allowed = set(ORACLE["allowed_reconciled"])
    for number, transaction in book["transactions"].items():
        if number not in allowed and number not in ORACLE["decoy_transactions"]:
            if any(split["state"] == "y" for split in transaction["splits"]):
                problems.append(f"unsupported transaction reconciled: {number}")
    return {"passed": not problems, "problems": problems}


def check_discrepancies(book: dict) -> dict:
    problems = []
    for number in ("GNC-514", "GNC-519"):
        expected = ORACLE["expected"][number]
        try:
            if _states(book, number) != ["n"] * len(book["transactions"][number]["splits"]):
                problems.append(f"{number} must remain unreconciled")
            checking = checking_split(book, number)
            if not _memo_matches(number, {checking["memo"]} if checking["memo"] else set()):
                problems.append(f"{number} must use the required exception memo with exact identifiers and amounts")
            if any(split["memo"] for split in book["transactions"][number]["splits"] if split is not checking):
                problems.append(f"{number} exception memo must be on the checking split only")
            if any(split["reconcile_date"] for split in book["transactions"][number]["splits"]):
                problems.append(f"{number} must not have a reconcile date")
            if transaction_amount(book, number) != money_fraction(expected["amount"]):
                problems.append(f"{number} amount changed")
        except (KeyError, TypeError, ValueError) as error:
            problems.append(f"malformed {number}: {type(error).__name__}")
    return {"passed": not problems, "problems": problems}


def _transaction_equal(current: dict, original: dict, *, mutable_reconciliation: bool) -> bool:
    core_fields = ("guid", "description", "posted", "entered", "currency")
    if any(current[field] != original[field] for field in core_fields):
        return False
    current_splits = {split["id"]: split for split in current["splits"]}
    original_splits = {split["id"]: split for split in original["splits"]}
    if set(current_splits) != set(original_splits):
        return False
    for split_id, split in current_splits.items():
        prior = original_splits[split_id]
        fields = ["account", "value", "quantity"]
        if not mutable_reconciliation:
            fields.extend(["state", "memo", "reconcile_date"])
        if any(split[field] != prior[field] for field in fields):
            return False
    return True


def check_unmatched(book: dict, pristine: dict) -> dict:
    problems = []
    current = book["transactions"]
    if "NAR-773" in current:
        problems.append("source-only NAR-773 was created in the ledger")
    try:
        if not _transaction_equal(
            current["GNC-521"], pristine["transactions"]["GNC-521"], mutable_reconciliation=False
        ):
            problems.append("ledger-only GNC-521 was not preserved exactly")
    except KeyError:
        problems.append("ledger-only GNC-521 is missing")
    return {"passed": not problems, "problems": problems}


def _row_map(tables: dict[str, list[list[Cell]]]) -> tuple[dict[str, list[Cell]], list[str]]:
    problems = []
    rows: dict[str, list[Cell]] = {}
    reconciliation = tables.get("Reconciliation", [])
    for row in reconciliation[1:]:
        logical = values(row, 9)
        if not logical[0]:
            continue
        if logical[0] in rows:
            problems.append(f"duplicate workbook row: {logical[0]}")
        else:
            rows[logical[0]] = row
    return rows, problems


def _cell_at(row: list[Cell], column: int) -> Cell:
    return row[column] if column < len(row) else Cell("")


def _numeric_equal(cell: Cell, expected: str) -> bool:
    try:
        wanted = Decimal(expected)
    except InvalidOperation:
        return False
    return cell.number is not None and cell.number == wanted


def _rows_equal(left: list[Cell], right: list[Cell]) -> bool:
    width = max(len(left), len(right))
    for column in range(width):
        first = _cell_at(left, column)
        second = _cell_at(right, column)
        if first.formula or second.formula:
            continue
        if first.number is not None or second.number is not None:
            if first.number is None or second.number is None or first.number != second.number:
                return False
        elif first.value != second.value:
            return False
    return True


def _normalize_formula(formula: str) -> str:
    normalized = re.sub(r"\s+", "", formula).casefold()
    normalized = re.sub(r"^(?:(?:of|oooc):=)+", "", normalized)
    normalized = normalized.replace("com.microsoft.", "").replace("$", "").replace("'", "")
    return normalized


def _formula_map(tables: dict[str, list[list[Cell]]]) -> dict[tuple[str, int, int], str]:
    return {
        (sheet, row_index, column_index): _normalize_formula(cell.formula)
        for sheet, rows in tables.items()
        for row_index, row in enumerate(rows)
        for column_index, cell in enumerate(row)
        if cell.formula
    }


def check_workbook(tables: dict[str, list[list[Cell]]], pristine_tables: dict[str, list[list[Cell]]], book: dict) -> dict:
    rows, problems = _row_map(tables)
    expected_ids = list(ORACLE["expected"]) + ORACLE["decoy_transactions"] + ORACLE["source_only"]
    if set(rows) != set(expected_ids):
        problems.append(f"workbook ledger IDs changed: {sorted(rows)}")
    pristine_rows, _ = _row_map(pristine_tables)
    for number, expected in ORACLE["expected"].items():
        row = rows.get(number, [])
        pristine_row = pristine_rows.get(number, [])
        logical = values(rows.get(number, []), 9)
        if logical[0:3] != values(pristine_row, 9)[0:3] or not _numeric_equal(
            _cell_at(row, 3), values(pristine_row, 9)[3]
        ):
            problems.append(f"{number} workbook ledger fields changed")
        source_amount_ok = (
            _numeric_equal(_cell_at(row, 5), expected["source_amount"])
            if expected["source_amount"]
            else _cell_at(row, 5).value == "" and _cell_at(row, 5).number is None
        )
        if logical[4] != expected["source"] or not source_amount_ok or logical[6] != expected["status"]:
            problems.append(
                f"{number} source/status cells do not match {expected['source']}, "
                f"{expected['source_amount']}, {expected['status']}"
            )
        evidence = logical[7]
        if number in ORACLE["allowed_reconciled"]:
            if evidence.strip().casefold() != expected["source"].casefold():
                problems.append(f"{number} evidence must be its source invoice ID")
        elif number in {"GNC-514", "GNC-519"}:
            if not _memo_matches(number, {evidence}):
                problems.append(f"{number} workbook evidence must use the required exception template")
        elif evidence != expected["evidence"]:
            problems.append(f"{number} evidence cell must be {expected['evidence']}")
        try:
            payable_state = checking_split(book, number)["state"]
            should_reconcile = number in ORACLE["allowed_reconciled"]
            if (payable_state == "y") != should_reconcile:
                problems.append(f"{number} workbook and ledger reconciliation disagree")
        except (KeyError, ValueError):
            problems.append(f"{number} cannot be compared to ledger")
    if not _rows_equal(rows.get("GNC-406", []), pristine_rows.get("GNC-406", [])):
        problems.append("prior-period workbook control row changed")
    source_only = [row for row in tables.get("Reconciliation", [])[1:] if values(row, 9)[0] == "NAR-773"]
    if len(source_only) != 1:
        problems.append("workbook must retain NAR-773 as one source-only unmatched row")
    else:
        source_row = source_only[0]
        logical = values(source_row, 9)
        if (
            logical[:2] != ["NAR-773", ""]
            or logical[2].strip().casefold() != "nacre audio repair"
            or logical[3:5] != ["", "NAR-773"]
            or not _numeric_equal(_cell_at(source_row, 5), "275.00")
            or logical[6:8] != ["unmatched", "No ledger payment found"]
        ):
            problems.append("workbook must retain NAR-773 as one source-only unmatched row")
    if len(tables.get("Reconciliation", [])) != len(pristine_tables.get("Reconciliation", [])):
        problems.append("workbook reconciliation row count changed")
    if list(tables) != list(pristine_tables):
        problems.append(f"workbook sheet set/order changed: {list(tables)}")
    controls = tables.get("Controls", [])
    pristine_controls = pristine_tables.get("Controls", [])
    if len(controls) != len(pristine_controls) or any(
        not _rows_equal(current, original) for current, original in zip(controls, pristine_controls)
    ):
        problems.append("workbook controls changed")
    if _formula_map(tables) != _formula_map(pristine_tables):
        problems.append("workbook formula structure changed")
    return {"passed": not problems, "problems": problems, "sheets": list(tables)}


def _safe_files(root: Path) -> tuple[dict[str, str], list[str]]:
    result = {}
    problems = []
    if not root.exists() or not root.is_dir() or root.is_symlink():
        return result, [f"missing or invalid source directory: {root.name}"]
    for current, directories, filenames in os.walk(root, followlinks=False):
        base = Path(current)
        if any((base / directory).is_symlink() for directory in directories):
            problems.append("source tree contains a symlink")
        directories[:] = [directory for directory in directories if not (base / directory).is_symlink()]
        for filename in filenames:
            path = base / filename
            if path.is_symlink():
                problems.append(f"invalid source file: {path.relative_to(root)}")
                continue
            if filename.casefold() in IGNORED_OS_METADATA:
                continue
            if path.stat().st_size > 4 * 1024 * 1024:
                problems.append(f"invalid source file: {path.relative_to(root)}")
                continue
            result[path.relative_to(root).as_posix()] = hashlib.sha256(path.read_bytes()).hexdigest()
            if len(result) > 256:
                problems.append("source tree contains too many files")
                return result, problems
    return result, problems


def check_source_preserved(workspace: Path) -> dict:
    problems = []
    actual, actual_problems = _safe_files(workspace / "mail")
    problems.extend(actual_problems)
    expected = ORACLE.get("mail_sha256", {})
    if actual != expected:
        missing = sorted(set(expected) - set(actual))
        extra = sorted(set(actual) - set(expected))
        changed = sorted(path for path in set(actual) & set(expected) if actual[path] != expected[path])
        problems.append(f"mail source files changed (missing={missing}, extra={extra}, changed={changed})")
    return {"passed": not problems, "problems": problems, "files_checked": len(expected)}


def check_preservation(book: dict, pristine: dict) -> dict:
    problems = []
    if book["book_core"] != pristine["book_core"]:
        problems.append("book identity or commodity changed")
    if set(book["accounts"]) != set(pristine["accounts"]):
        problems.append("account set changed")
    else:
        for guid in book["accounts"]:
            fields = ("name", "type", "parent", "commodity", "commodity_scu")
            if any(book["accounts"][guid][field] != pristine["accounts"][guid][field] for field in fields):
                problems.append(f"account changed: {guid}")
    if set(book["transactions"]) != set(pristine["transactions"]):
        problems.append("transaction set changed")
    else:
        mutable = set(ORACLE["allowed_reconciled"]) | {"GNC-514", "GNC-519"}
        for number, transaction in book["transactions"].items():
            original = pristine["transactions"][number]
            if not _transaction_equal(
                transaction, original, mutable_reconciliation=number in mutable
            ):
                problems.append(f"protected transaction content changed: {number}")
    return {"passed": not problems, "problems": problems}


def evaluate(workspace: Path, artifacts: Path, agent_exit_code: int) -> dict:
    del artifacts
    book, book_error = load_book(workspace / BOOK_REL)
    pristine, pristine_error = load_book(FIXTURE / BOOK_REL)
    tables, workbook_error = load_workbook(workspace / WORKBOOK_REL)
    pristine_tables, pristine_workbook_error = load_workbook(FIXTURE / WORKBOOK_REL)
    if book_error or pristine_error:
        reason = book_error or f"invalid pristine book: {pristine_error}"
        ledger = supported = discrepancies = unmatched = preservation = _failure(reason)
    else:
        ledger = check_balanced(book)
        supported = check_supported_matches(book)
        discrepancies = check_discrepancies(book)
        unmatched = check_unmatched(book, pristine)
        preservation = check_preservation(book, pristine)
    if workbook_error or pristine_workbook_error or book is None:
        workbook = _failure(workbook_error or pristine_workbook_error or "ledger unavailable")
    else:
        workbook = check_workbook(tables, pristine_tables, book)
    checks = {
        "ledger_balanced": ledger,
        "supported_matches": supported,
        "exact_discrepancies_flagged": discrepancies,
        "unmatched_preserved": unmatched,
        "workbook_agreement": workbook,
        "source_preserved": check_source_preserved(workspace),
        "preservation": preservation,
    }
    score = sum(weight for name, weight in CHECKS if checks[name].get("passed") is True) / 100
    return {
        "passed": all(checks[name].get("passed") is True for name, _ in CHECKS),
        "score": round(score, 6),
        "detail": {"checks": checks, "agent_exit_code": agent_exit_code, "artifacts_consulted": False},
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--workspace", type=Path, required=True)
    parser.add_argument("--artifacts", type=Path, required=True)
    parser.add_argument("--result", type=Path, required=True)
    parser.add_argument("--agent-exit-code", type=int, required=True)
    args = parser.parse_args()
    args.result.parent.mkdir(parents=True, exist_ok=True)
    try:
        result = evaluate(args.workspace.resolve(), args.artifacts.resolve(), args.agent_exit_code)
    except Exception as error:  # final containment: malformed submissions are graded, not harness crashes
        reason = f"unexpected evaluator error: {type(error).__name__}: {error}"
        checks = {name: _failure(reason) for name, _ in CHECKS}
        result = {"passed": False, "score": 0.0, "detail": {"checks": checks, "agent_exit_code": args.agent_exit_code, "artifacts_consulted": False}}
    args.result.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
