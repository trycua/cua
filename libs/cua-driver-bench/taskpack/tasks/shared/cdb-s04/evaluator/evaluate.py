#!/usr/bin/env python3
"""Bounded, trajectory-agnostic final-state evaluator for CDB-S04."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import signal
import socket
import subprocess
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
ORACLE = json.loads((HERE / "oracle.json").read_text(encoding="utf-8"))
CHECKS = [
    ("duplicate_links_correct", 25),
    ("evidence_complete_and_correlated", 25),
    ("browser_reproduction_semantics", 20),
    ("terminal_log_correlation", 15),
    ("unsupported_cases_open", 10),
    ("preservation", 5),
]
IGNORED_DIRS = {"node_modules", ".runtime", "__pycache__"}
IGNORED_OS_METADATA = {".DS_Store", "desktop.ini", "Thumbs.db"}
MAX_JSON_BYTES = 1024 * 1024
MAX_FILE_BYTES = 4 * 1024 * 1024
MAX_TREE_BYTES = 16 * 1024 * 1024
MAX_TREE_FILES = 256
MAX_HTTP_BYTES = 1024 * 1024


def failure(reason: str) -> dict:
    return {"passed": False, "reason": reason}


def digest_bytes(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()


def digest_json(value: object) -> str:
    payload = json.dumps(
        value, sort_keys=True, separators=(",", ":"), ensure_ascii=False
    ).encode("utf-8")
    return digest_bytes(payload)


def bounded_bytes(path: Path, limit: int = MAX_FILE_BYTES) -> bytes:
    if path.is_symlink() or not path.is_file():
        raise ValueError(f"missing or invalid file: {path.name}")
    size = path.stat().st_size
    if size > limit:
        raise ValueError(f"oversized file: {path.name} ({size} bytes)")
    return path.read_bytes()


def load_json(path: Path) -> tuple[object | None, str | None]:
    try:
        payload = bounded_bytes(path, MAX_JSON_BYTES)
        return json.loads(payload.decode("utf-8")), None
    except (OSError, UnicodeError, ValueError, TypeError, json.JSONDecodeError) as error:
        return None, f"{type(error).__name__}: {error}"


def scan_tree(workspace: Path) -> tuple[dict[str, str], list[str]]:
    files: dict[str, str] = {}
    problems: list[str] = []
    total = 0
    if workspace.is_symlink() or not workspace.is_dir():
        return files, ["workspace is missing, not a directory, or a symlink"]
    for current, directories, filenames in os.walk(workspace, followlinks=False):
        base = Path(current)
        kept = []
        for directory in directories:
            path = base / directory
            if path.is_symlink():
                problems.append(f"symlink is not allowed: {path.relative_to(workspace).as_posix()}")
                continue
            if directory in IGNORED_DIRS:
                # Browser profiles contain legitimate symlinks and large caches.
                # Nothing below an ignored root is graded or executed, and
                # followlinks=False prevents traversal outside the workspace.
                continue
            else:
                kept.append(directory)
        directories[:] = kept
        for filename in filenames:
            path = base / filename
            relative = path.relative_to(workspace).as_posix()
            if path.is_symlink():
                problems.append(f"symlink is not allowed: {relative}")
                continue
            if filename in IGNORED_OS_METADATA:
                continue
            try:
                size = path.stat().st_size
                if size > MAX_FILE_BYTES:
                    problems.append(f"oversized workspace file: {relative}")
                    continue
                total += size
                if total > MAX_TREE_BYTES:
                    problems.append("workspace tree exceeds byte limit")
                    return files, problems
                files[relative] = digest_bytes(path.read_bytes())
                if len(files) > MAX_TREE_FILES:
                    problems.append("workspace tree contains too many files")
                    return files, problems
            except OSError as error:
                problems.append(f"cannot inspect {relative}: {type(error).__name__}")
    return files, problems


def case_map(document: object) -> tuple[dict[str, dict] | None, str | None]:
    if not isinstance(document, dict) or set(document) != {"cases"} or not isinstance(document["cases"], list):
        return None, "case store must be an object containing only a cases array"
    result: dict[str, dict] = {}
    for case in document["cases"]:
        if not isinstance(case, dict) or not isinstance(case.get("id"), str):
            return None, "every case must be an object with a string id"
        if case["id"] in result:
            return None, f"duplicate case id: {case['id']}"
        result[case["id"]] = case
    return result, None


def check_links(cases: dict[str, dict] | None) -> dict:
    if cases is None:
        return failure("case store unavailable")
    problems = []
    for case_id, expected in ORACLE["duplicates"].items():
        case = cases.get(case_id)
        if case is None:
            problems.append(f"missing duplicate candidate: {case_id}")
            continue
        if case.get("status") != "duplicate":
            problems.append(f"{case_id} status must be duplicate")
        if case.get("duplicate_of") != ORACLE["root_case"]:
            problems.append(f"{case_id} must link directly to {ORACLE['root_case']}")
    return {"passed": not problems, "problems": problems}


def _semantic_url(value: object) -> tuple[str, str, tuple[tuple[str, str], ...]] | None:
    if not isinstance(value, str):
        return None
    try:
        parsed = urllib.parse.urlsplit(value)
        if (
            parsed.scheme != "http"
            or parsed.hostname != "127.0.0.1"
            or parsed.port != 4184
            or parsed.username is not None
            or parsed.password is not None
        ):
            return None
        query = tuple(sorted(urllib.parse.parse_qsl(parsed.query, keep_blank_values=True)))
        return parsed.path, parsed.fragment, query
    except ValueError:
        return None


def check_evidence(cases: dict[str, dict] | None) -> dict:
    if cases is None:
        return failure("case store unavailable")
    problems = []
    for case_id, expected in ORACLE["duplicates"].items():
        evidence = cases.get(case_id, {}).get("evidence")
        if not isinstance(evidence, dict) or set(evidence) != {"wiki_ref", "repro_url", "log_ref"}:
            problems.append(f"{case_id} evidence must contain exactly wiki_ref, repro_url, and log_ref")
            continue
        wiki_ref = evidence.get("wiki_ref")
        log_ref = evidence.get("log_ref")
        if not isinstance(wiki_ref, str) or wiki_ref.strip() != expected["wiki_ref"]:
            problems.append(f"{case_id} has the wrong wiki correlation")
        if _semantic_url(evidence.get("repro_url")) != _semantic_url(expected["repro_url"]):
            problems.append(f"{case_id} has the wrong reproduction URL")
        if not isinstance(log_ref, str) or log_ref.strip() != expected["log_ref"]:
            problems.append(f"{case_id} has the wrong log correlation")
    return {"passed": not problems, "problems": problems}


def free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


def http_get(url: str, timeout: float = 2.0) -> tuple[bytes, str]:
    request = urllib.request.Request(url, headers={"Connection": "close"})
    with urllib.request.urlopen(request, timeout=timeout) as response:  # noqa: S310 - loopback URL only
        content_type = response.headers.get("Content-Type", "")
        declared = response.headers.get("Content-Length")
        if declared and int(declared) > MAX_HTTP_BYTES:
            raise ValueError("HTTP response exceeds size limit")
        payload = response.read(MAX_HTTP_BYTES + 1)
        if len(payload) > MAX_HTTP_BYTES:
            raise ValueError("HTTP response exceeds size limit")
        return payload, content_type


def _contains_semantics(payload: object, record: str, view: str, signature: str, *, defect: bool) -> bool:
    rendered = json.dumps(payload, sort_keys=True).casefold()
    basics = record.casefold() in rendered and view.casefold() in rendered
    has_signature = signature.casefold() in rendered
    return basics and (has_signature if defect else not has_signature)


def _process_group_options(platform: str | None = None) -> dict:
    """Return platform-appropriate Popen options for isolated termination."""
    platform = os.name if platform is None else platform
    if platform == "nt":
        return {"creationflags": getattr(subprocess, "CREATE_NEW_PROCESS_GROUP", 0x200)}
    return {"start_new_session": True}


def _terminate(process: subprocess.Popen, platform: str | None = None) -> None:
    if process.poll() is not None:
        return
    platform = os.name if platform is None else platform
    try:
        if platform == "nt":
            process.send_signal(getattr(signal, "CTRL_BREAK_EVENT", signal.SIGTERM))
        else:
            os.killpg(process.pid, signal.SIGTERM)
    except (OSError, ProcessLookupError):
        process.terminate()
    try:
        process.wait(timeout=3)
    except subprocess.TimeoutExpired:
        try:
            if platform == "nt":
                process.kill()
            else:
                os.killpg(process.pid, signal.SIGKILL)
        except (OSError, ProcessLookupError):
            process.kill()
        process.wait(timeout=3)


def _stage_verified_site(workspace: Path, scratch: Path) -> tuple[Path | None, str | None]:
    """Copy only oracle-pinned site files, validating before any code executes."""
    site = scratch / "archive-site"
    for relative, expected_hash in ORACLE["protected_sha256"].items():
        if not relative.startswith("archive-site/"):
            continue
        source = workspace / relative
        try:
            payload = bounded_bytes(source)
        except (OSError, ValueError) as error:
            return None, f"protected browser source unavailable: {relative}: {type(error).__name__}"
        if digest_bytes(payload) != expected_hash:
            return None, f"protected browser source changed: {relative}"
        destination = scratch / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(payload)
    return site, None


def _server_environment(port: int) -> dict[str, str]:
    """Avoid submission-controlled Node hooks while retaining Windows essentials."""
    env = {
        key: value for key, value in os.environ.items()
        if key.casefold() in {"systemroot", "windir", "comspec", "pathext", "temp", "tmp"}
    }
    env.update({"CDB_S04_HOST": "127.0.0.1", "CDB_S04_PORT": str(port)})
    return env


def check_browser(workspace: Path, scratch: Path, cases: dict[str, dict] | None) -> dict:
    site, stage_error = _stage_verified_site(workspace, scratch)
    node = shutil.which("node")
    if node is None:
        return failure("node unavailable")
    if stage_error is not None or site is None:
        return failure(stage_error or "protected browser source unavailable")
    port = free_port()
    env = _server_environment(port)
    detail: dict = {"port": port, "probes": {}}
    try:
        process = subprocess.Popen(
            [node, "server.js"], cwd=str(site), env=env, stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL, **_process_group_options(),
        )
    except OSError as error:
        return failure(f"server start failed: {type(error).__name__}: {error}")
    try:
        base = f"http://127.0.0.1:{port}"
        deadline = time.monotonic() + 10
        ready = False
        while time.monotonic() < deadline and process.poll() is None:
            try:
                health, _ = http_get(base + "/healthz", 0.75)
                if health:
                    ready = True
                    break
            except (OSError, ValueError, urllib.error.URLError, urllib.error.HTTPError):
                time.sleep(0.1)
        if not ready:
            detail.update({"passed": False, "reason": "fresh server did not become ready"})
            return detail
        all_ok = True
        candidate_probes = []
        for case_id, expected in ORACLE["duplicates"].items():
            evidence = cases.get(case_id, {}).get("evidence", {}) if cases else {}
            semantic = _semantic_url(evidence.get("repro_url"))
            expected_semantic = _semantic_url(expected["repro_url"])
            expected_query = dict(expected_semantic[2]) if expected_semantic else {}
            candidate_probes.append((case_id, {
                "record": expected_query.get("record", ""),
                "view": expected_query.get("view", ""),
                "signature": expected["log_ref"].rsplit(":", 1)[-1],
                "defect": True,
            }, semantic))
        for probe in ORACLE["browser_probes"]:
            if not probe["defect"]:
                candidate_probes.append((f"control:{probe['record']}", probe, _semantic_url(
                    f"http://127.0.0.1:4184/inspect?record={probe['record']}&view={probe['view']}"
                )))

        for key, probe, semantic in candidate_probes:
            if semantic is None:
                detail["probes"][key] = {"passed": False, "reason": "invalid stored reproduction URL"}
                all_ok = False
                continue
            path, fragment, query_items = semantic
            query = urllib.parse.urlencode(query_items)
            page_raw, page_type = http_get(base + path + "?" + query)
            api_raw, api_type = http_get(base + "/api/inspect?" + query)
            page = page_raw.decode("utf-8")
            api = json.loads(api_raw.decode("utf-8"))
            page_ok = _contains_semantics(page, probe["record"], probe["view"], probe["signature"], defect=probe["defect"])
            api_ok = _contains_semantics(api, probe["record"], probe["view"], probe["signature"], defect=probe["defect"])
            type_ok = "html" in page_type.casefold() and "json" in api_type.casefold()
            ok = page_ok and api_ok and type_ok
            detail["probes"][key] = {
                "passed": ok, "page": page_ok, "api": api_ok,
                "content_types": type_ok, "fragment": fragment,
            }
            all_ok = all_ok and ok
        detail["passed"] = all_ok
        return detail
    except (OSError, UnicodeError, ValueError, TypeError, json.JSONDecodeError,
            urllib.error.URLError, urllib.error.HTTPError) as error:
        detail.update({"passed": False, "reason": f"browser probe failed: {type(error).__name__}: {error}"})
        return detail
    finally:
        _terminate(process)


def check_logs(workspace: Path, cases: dict[str, dict] | None) -> dict:
    try:
        text = bounded_bytes(workspace / "logs" / "archive-access.log").decode("utf-8")
    except (OSError, UnicodeError, ValueError) as error:
        return failure(f"log unavailable: {type(error).__name__}: {error}")
    problems = []
    for case_id, expected in ORACLE["duplicates"].items():
        reference = cases.get(case_id, {}).get("evidence", {}).get("log_ref") if cases else None
        reference = reference.strip() if isinstance(reference, str) else None
        canonical_line = expected["log_line"]
        normalized_lines = [" ".join(line.split()) for line in text.splitlines()]
        if reference != expected["log_ref"] or normalized_lines.count(canonical_line) != 1:
            problems.append(f"{case_id} lacks its unique canonical log correlation")
    return {"passed": not problems, "problems": problems}


def check_unsupported(cases: dict[str, dict] | None) -> dict:
    if cases is None:
        return failure("case store unavailable")
    problems = []
    for case_id in ORACLE["unsupported_cases"]:
        if digest_json(cases.get(case_id)) != ORACLE["initial_case_sha256"].get(case_id):
            problems.append(f"unsupported case changed: {case_id}")
    return {"passed": not problems, "problems": problems}


def check_preservation(workspace: Path, cases: dict[str, dict] | None, tree: dict[str, str], tree_problems: list[str]) -> dict:
    problems = list(tree_problems)
    expected_files = ORACLE["workspace_files"]
    if set(tree) != set(expected_files):
        problems.append(
            f"workspace files changed (missing={sorted(set(expected_files) - set(tree))}, "
            f"extra={sorted(set(tree) - set(expected_files))})"
        )
    for relative, expected_hash in ORACLE["protected_sha256"].items():
        if tree.get(relative) != expected_hash:
            problems.append(f"protected source changed: {relative}")
    initial_hashes = ORACLE["initial_case_sha256"]
    if cases is None:
        problems.append("case store unavailable")
    elif set(cases) != set(initial_hashes):
        problems.append("case identifiers were added or removed")
    else:
        for case_id, current in cases.items():
            if case_id in ORACLE["duplicates"]:
                immutable = {
                    key: value for key, value in current.items()
                    if key not in {"status", "duplicate_of", "evidence"}
                }
                if digest_json(immutable) != ORACLE["duplicate_immutable_sha256"].get(case_id):
                    problems.append(f"immutable case content changed: {case_id}")
                if set(current) != set(ORACLE["case_fields"]):
                    problems.append(f"case field set changed: {case_id}")
            elif digest_json(current) != initial_hashes.get(case_id):
                problems.append(f"protected case changed: {case_id}")
    return {"passed": not problems, "problems": problems, "files_checked": len(expected_files)}


def evaluate(workspace: Path, artifacts: Path, agent_exit_code: int) -> dict:
    del artifacts  # Agent-authored screenshots, logs, and claims are never evidence.
    document, json_error = load_json(workspace / "cases" / "cases.json")
    cases, shape_error = case_map(document) if json_error is None else (None, json_error)
    tree, tree_problems = scan_tree(workspace)
    with tempfile.TemporaryDirectory(prefix="cdb-s04-evaluator-") as temporary:
        browser = check_browser(workspace, Path(temporary), cases)
    checks = {
        "duplicate_links_correct": check_links(cases),
        "evidence_complete_and_correlated": check_evidence(cases),
        "browser_reproduction_semantics": browser,
        "terminal_log_correlation": check_logs(workspace, cases),
        "unsupported_cases_open": check_unsupported(cases),
        "preservation": check_preservation(workspace, cases, tree, tree_problems),
    }
    if shape_error:
        for name in ("duplicate_links_correct", "evidence_complete_and_correlated", "unsupported_cases_open"):
            checks[name] = failure(shape_error)
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
    except Exception as error:  # Malformed submissions are graded, never allowed to crash the harness.
        reason = f"unexpected evaluator error: {type(error).__name__}: {error}"
        result = {
            "passed": False,
            "score": 0.0,
            "detail": {
                "checks": {name: failure(reason) for name, _ in CHECKS},
                "agent_exit_code": args.agent_exit_code,
                "artifacts_consulted": False,
            },
        }
    args.result.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
