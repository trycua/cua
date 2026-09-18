#!/usr/bin/env python3
"""Run a bound local perception engine and emit measure_quality.py input JSON."""

from __future__ import annotations

import argparse
import base64
import hashlib
import importlib
import json
import math
import os
import selectors
import shutil
import struct
import subprocess
import sys
import tempfile
import threading
import time
import zipfile
from pathlib import Path, PurePosixPath
from typing import Any

import measure_quality


MAX_FRAME_BYTES = 16 * 1024 * 1024
PROTOCOL = "cua-perception/1"


class RunnerError(RuntimeError):
    """The runner configuration, artifact set, or engine output is invalid."""


def _closed(value: Any, required: tuple[str, ...], optional: tuple[str, ...], where: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise RunnerError(f"{where} must be an object")
    missing = sorted(set(required) - value.keys())
    unknown = sorted(value.keys() - set(required) - set(optional))
    if missing:
        raise RunnerError(f"{where} is missing fields: {', '.join(missing)}")
    if unknown:
        raise RunnerError(f"{where} has unknown fields: {', '.join(unknown)}")
    return value


def _string(value: Any, where: str) -> str:
    if not isinstance(value, str) or not value or "\x00" in value:
        raise RunnerError(f"{where} must be a non-empty string")
    return value


def _number(value: Any, where: str, minimum: float = 0.0, maximum: float | None = None) -> float:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise RunnerError(f"{where} must be a number")
    result = float(value)
    if not math.isfinite(result) or result < minimum or (maximum is not None and result > maximum):
        raise RunnerError(f"{where} is outside its allowed range")
    return result


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def _regular_file(root: Path, value: Any, where: str) -> Path:
    name = _string(value, where)
    relative = PurePosixPath(name)
    if relative.is_absolute() or not relative.parts or any(part in {"", ".", ".."} for part in relative.parts) or "\\" in name:
        raise RunnerError(f"{where} must be a safe relative path")
    current = root
    for part in relative.parts:
        current /= part
        if current.is_symlink():
            raise RunnerError(f"{where} must not traverse a symlink")
    path = current.resolve()
    if (path.parent != root and root not in path.parents) or not path.is_file():
        raise RunnerError(f"{where} is not a confined regular file")
    return path


def _directory(root: Path, value: Any, where: str) -> Path:
    name = _string(value, where)
    relative = PurePosixPath(name)
    if relative.is_absolute() or not relative.parts or any(part in {"", ".", ".."} for part in relative.parts) or "\\" in name:
        raise RunnerError(f"{where} must be a safe relative path")
    current = root
    for part in relative.parts:
        current /= part
        if current.is_symlink():
            raise RunnerError(f"{where} must not traverse a symlink")
    path = current.resolve()
    if (path != root and root not in path.parents) or not path.is_dir():
        raise RunnerError(f"{where} is not a confined directory")
    return path


def _load_config(path: Path, engine: str) -> dict[str, Any]:
    try:
        raw, _ = measure_quality.read_json(path)
    except Exception as error:
        raise RunnerError(str(error)) from error
    top = _closed(raw, ("schema_version", "identity", "bindings", "execution"), (), "config")
    if top["schema_version"] != 1:
        raise RunnerError("config.schema_version must be 1")
    # Reuse the consumer's strict identity validation so producer and consumer cannot drift.
    try:
        identity = measure_quality._identity(top["identity"], "config.identity")
    except Exception as error:
        raise RunnerError(str(error)) from error
    bindings = _closed(top["bindings"], ("artifacts", "package_artifact_id", "installed_artifact_ids"), (), "config.bindings")
    artifacts = bindings["artifacts"]
    if not isinstance(artifacts, list) or not artifacts:
        raise RunnerError("config.bindings.artifacts must be a non-empty array")
    root = path.parent.resolve()
    checked: list[dict[str, Any]] = []
    seen: set[str] = set()
    for index, raw_artifact in enumerate(artifacts):
        where = f"config.bindings.artifacts[{index}]"
        item = _closed(raw_artifact, ("role", "id", "path", "sha256"), (), where)
        try:
            role = measure_quality._identity_string(item["role"], f"{where}.role")
            artifact_id = measure_quality._identity_string(item["id"], f"{where}.id")
        except Exception as error:
            raise RunnerError(str(error)) from error
        if role not in measure_quality.ARTIFACT_ROLES:
            raise RunnerError(f"{where}.role is unsupported")
        if artifact_id in seen:
            raise RunnerError(f"duplicate artifact id: {artifact_id}")
        seen.add(artifact_id)
        source = _regular_file(root, item["path"], f"{where}.path")
        expected = _string(item["sha256"], f"{where}.sha256")
        actual = _sha256(source)
        if expected != actual:
            raise RunnerError(f"{where}.sha256 does not match the bound file")
        checked.append({
            "role": role,
            "id": artifact_id,
            "source": source,
            "sha256": actual,
            "root": root,
            "relative_path": item["path"],
        })
    installed = bindings["installed_artifact_ids"]
    if not isinstance(installed, list) or not installed or any(not isinstance(item, str) for item in installed):
        raise RunnerError("config.bindings.installed_artifact_ids must be a non-empty string array")
    if len(installed) != len(set(installed)):
        raise RunnerError("config.bindings.installed_artifact_ids must be unique")
    package_id = _string(bindings["package_artifact_id"], "config.bindings.package_artifact_id")
    artifacts_by_id = {item["id"]: item for item in checked}
    if package_id not in artifacts_by_id or artifacts_by_id[package_id]["role"] != "package":
        raise RunnerError("config.bindings.package_artifact_id must identify a package artifact")
    unknown_installed = sorted(set(installed) - artifacts_by_id.keys())
    if unknown_installed:
        raise RunnerError(f"config.bindings.installed_artifact_ids contains unknown ids: {', '.join(unknown_installed)}")
    execution = _validate_execution(top["execution"], engine, root, artifacts_by_id, package_id)
    if engine == "rust" and execution["source_revision"] != identity["model"]["revision"]:
        raise RunnerError("model manifest.identity.source_revision must match config.identity.model.revision")
    return {
        "identity": identity,
        "artifacts": checked,
        "package_artifact_id": package_id,
        "installed_artifact_ids": installed,
        "execution": execution,
    }


def _artifact_reference(
    value: Any,
    where: str,
    artifacts: dict[str, dict[str, Any]],
    expected_role: str | None = None,
) -> Path:
    artifact_id = _string(value, where)
    if artifact_id not in artifacts:
        raise RunnerError(f"{where} identifies no bound artifact")
    if expected_role is not None and artifacts[artifact_id]["role"] != expected_role:
        raise RunnerError(f"{where} must identify a {expected_role} artifact")
    return artifacts[artifact_id]["source"]


def _artifact_binding(
    value: Any,
    where: str,
    artifacts: dict[str, dict[str, Any]],
    expected_role: str | None = None,
) -> dict[str, Any]:
    artifact_id = _string(value, where)
    if artifact_id not in artifacts:
        raise RunnerError(f"{where} identifies no bound artifact")
    artifact = artifacts[artifact_id]
    if expected_role is not None and artifact["role"] != expected_role:
        raise RunnerError(f"{where} must identify a {expected_role} artifact")
    return artifact


def _verify_bound_artifacts(artifacts: list[dict[str, Any]], where: str) -> None:
    for artifact in artifacts:
        artifact_where = f"{where} artifact {artifact['id']}"
        try:
            current = _regular_file(artifact["root"], artifact["relative_path"], artifact_where)
            actual = _sha256(current)
        except (OSError, RunnerError) as error:
            raise RunnerError(f"{artifact_where} changed after configuration validation: {error}") from error
        if current != artifact["source"] or actual != artifact["sha256"]:
            raise RunnerError(f"{artifact_where} changed after configuration validation")


def _manifest_artifact(manifest_root: Path, value: Any, where: str) -> tuple[Path, str]:
    item = _closed(value, ("path", "sha256"), (), where)
    path = _regular_file(manifest_root, item["path"], f"{where}.path")
    expected = _string(item["sha256"], f"{where}.sha256")
    if len(expected) != 64 or any(character not in "0123456789abcdef" for character in expected):
        raise RunnerError(f"{where}.sha256 must be a lowercase SHA-256 digest")
    return path, expected


def _require_manifest_binding(
    manifest_root: Path,
    value: Any,
    where: str,
    artifact: dict[str, Any],
) -> None:
    manifest_path, manifest_sha256 = _manifest_artifact(manifest_root, value, where)
    if manifest_path != artifact["source"] or manifest_sha256 != artifact["sha256"]:
        raise RunnerError(f"{where} does not match its configured bound artifact")


def _validate_conversion_record(path: Path, source: dict[str, Any], output: dict[str, Any]) -> None:
    try:
        value, _ = measure_quality.read_json(path)
        record = _closed(value, ("input", "expected_output"), ("schema_version", "status", "command", "parameters", "toolchain", "patches", "limitations"), "conversion metadata")
        record_input = _closed(record["input"], ("sha256",), ("bundle_path", "size"), "conversion metadata.input")
        record_output = _closed(record["expected_output"], ("sha256",), ("size",), "conversion metadata.expected_output")
    except Exception as error:
        raise RunnerError(f"invalid conversion metadata: {error}") from error
    if record_input["sha256"] != source["sha256"] or record_output["sha256"] != output["sha256"]:
        raise RunnerError("conversion metadata does not bind the configured source and converted detector artifacts")


def _validate_execution(
    value: Any,
    engine: str,
    root: Path,
    artifacts: dict[str, dict[str, Any]],
    package_artifact_id: str,
) -> dict[str, Any]:
    if engine == "rust":
        item = _closed(
            value,
            (
                "worker_artifact_id", "manifest_artifact_id", "onnx_runtime_artifact_id",
                "source_model_artifact_id", "detector_artifact_id", "ocr_detector_artifact_id",
                "ocr_recognizer_artifact_id", "ocr_dictionary_artifact_id", "extension_id",
                "extension_version", "timeout_seconds",
            ),
            ("conversion_metadata_artifact_id",),
            "config.execution",
        )
        timeout = _number(item["timeout_seconds"], "config.execution.timeout_seconds", 0.001)
        worker = _artifact_reference(item["worker_artifact_id"], "config.execution.worker_artifact_id", artifacts, "worker")
        if not os.access(worker, os.X_OK):
            raise RunnerError("config.execution.worker_artifact_id is not executable")
        manifest_artifact = _artifact_binding(item["manifest_artifact_id"], "config.execution.manifest_artifact_id", artifacts, "model_manifest")
        runtime_artifact = _artifact_binding(item["onnx_runtime_artifact_id"], "config.execution.onnx_runtime_artifact_id", artifacts, "runtime")
        source_artifact = _artifact_binding(item["source_model_artifact_id"], "config.execution.source_model_artifact_id", artifacts, "source_model")
        detector_artifact = _artifact_binding(item["detector_artifact_id"], "config.execution.detector_artifact_id", artifacts, "converted_model")
        ocr_detector_artifact = _artifact_binding(item["ocr_detector_artifact_id"], "config.execution.ocr_detector_artifact_id", artifacts, "ocr_model")
        ocr_recognizer_artifact = _artifact_binding(item["ocr_recognizer_artifact_id"], "config.execution.ocr_recognizer_artifact_id", artifacts, "ocr_model")
        dictionary_artifact = _artifact_binding(item["ocr_dictionary_artifact_id"], "config.execution.ocr_dictionary_artifact_id", artifacts, "ocr_dictionary")
        try:
            manifest, _ = measure_quality.read_json(manifest_artifact["source"])
            identity = _closed(manifest["identity"], ("source_revision",), ("name", "version", "source_url", "license"), "model manifest.identity")
            runtime = _closed(manifest["onnx_runtime"], ("library_sha256",), ("version", "target", "intra_threads"), "model manifest.onnx_runtime")
            detector = _closed(manifest["detector"], ("model",), tuple(key for key in manifest["detector"] if key != "model"), "model manifest.detector")
            ocr = _closed(manifest["ocr"], ("detector", "recognizer", "dictionary"), tuple(key for key in manifest["ocr"] if key not in {"detector", "recognizer", "dictionary"}), "model manifest.ocr")
            ocr_detector = _closed(ocr["detector"], ("model",), tuple(key for key in ocr["detector"] if key != "model"), "model manifest.ocr.detector")
            ocr_recognizer = _closed(ocr["recognizer"], ("model",), tuple(key for key in ocr["recognizer"] if key != "model"), "model manifest.ocr.recognizer")
        except Exception as error:
            raise RunnerError(f"invalid model manifest bindings: {error}") from error
        source_revision = _string(identity["source_revision"], "model manifest.identity.source_revision")
        _require_manifest_binding(manifest_artifact["source"].parent.resolve(), detector["model"], "model manifest.detector.model", detector_artifact)
        _require_manifest_binding(manifest_artifact["source"].parent.resolve(), ocr_detector["model"], "model manifest.ocr.detector.model", ocr_detector_artifact)
        _require_manifest_binding(manifest_artifact["source"].parent.resolve(), ocr_recognizer["model"], "model manifest.ocr.recognizer.model", ocr_recognizer_artifact)
        _require_manifest_binding(manifest_artifact["source"].parent.resolve(), ocr["dictionary"], "model manifest.ocr.dictionary", dictionary_artifact)
        if runtime["library_sha256"] != runtime_artifact["sha256"]:
            raise RunnerError("model manifest.onnx_runtime.library_sha256 does not match the bound runtime artifact")
        conversion_ids = [artifact["id"] for artifact in artifacts.values() if artifact["role"] == "conversion_metadata"]
        conversion_id = item.get("conversion_metadata_artifact_id")
        if conversion_ids and conversion_id is None:
            raise RunnerError("config.execution.conversion_metadata_artifact_id is required when conversion metadata is bound")
        if conversion_id is not None:
            conversion = _artifact_binding(conversion_id, "config.execution.conversion_metadata_artifact_id", artifacts, "conversion_metadata")
            _validate_conversion_record(conversion["source"], source_artifact, detector_artifact)
        executed_artifacts = [
            _artifact_binding(item["worker_artifact_id"], "config.execution.worker_artifact_id", artifacts, "worker"),
            manifest_artifact,
            runtime_artifact,
            detector_artifact,
            ocr_detector_artifact,
            ocr_recognizer_artifact,
            dictionary_artifact,
        ]
        return {
            "worker": worker,
            "manifest": manifest_artifact["source"],
            "runtime": runtime_artifact["source"],
            "source_revision": source_revision,
            "extension_id": _string(item["extension_id"], "config.execution.extension_id"),
            "extension_version": _string(item["extension_version"], "config.execution.extension_version"),
            "timeout": timeout,
            "executed_artifacts": executed_artifacts,
        }
    item = _closed(value, ("python_interpreter_artifact_id", "python_home", "site_packages", "model_artifact_id", "ocr_model_artifact_ids", "cache_dir", "force_device", "box_threshold", "iou_threshold", "use_ocr", "timeout_seconds"), (), "config.execution")
    if not isinstance(item["use_ocr"], bool):
        raise RunnerError("config.execution.use_ocr must be a boolean")
    python = _artifact_reference(item["python_interpreter_artifact_id"], "config.execution.python_interpreter_artifact_id", artifacts, "worker")
    if not os.access(python, os.X_OK):
        raise RunnerError("config.execution.python_interpreter_artifact_id is not executable")
    cache_dir = _directory(root, item["cache_dir"], "config.execution.cache_dir")
    model_cache = cache_dir / "model"
    if not model_cache.is_dir() or model_cache.is_symlink():
        raise RunnerError("config.execution.cache_dir must contain a regular model directory for EasyOCR")
    ocr_ids = item["ocr_model_artifact_ids"]
    if not isinstance(ocr_ids, list) or not ocr_ids or any(not isinstance(value, str) for value in ocr_ids):
        raise RunnerError("config.execution.ocr_model_artifact_ids must be a non-empty string array")
    if len(ocr_ids) != len(set(ocr_ids)):
        raise RunnerError("config.execution.ocr_model_artifact_ids must be unique")
    ocr_models = [
        _artifact_reference(value, "config.execution.ocr_model_artifact_ids", artifacts, "ocr_model")
        for value in ocr_ids
    ]
    if any(path.parent.resolve() != model_cache.resolve() for path in ocr_models):
        raise RunnerError("configured OCR model artifacts must be direct files in cache_dir/model")
    model_artifact = _artifact_binding(item["model_artifact_id"], "config.execution.model_artifact_id", artifacts, "source_model")
    python_artifact = _artifact_binding(item["python_interpreter_artifact_id"], "config.execution.python_interpreter_artifact_id", artifacts, "worker")
    package_artifact = artifacts[package_artifact_id]
    ocr_artifacts = [
        _artifact_binding(value, "config.execution.ocr_model_artifact_ids", artifacts, "ocr_model")
        for value in ocr_ids
    ]
    return {
        "python": python,
        "python_home": _directory(root, item["python_home"], "config.execution.python_home"),
        "site_packages": _directory(root, item["site_packages"], "config.execution.site_packages"),
        "package": package_artifact["source"],
        "package_sha256": package_artifact["sha256"],
        "model": model_artifact["source"],
        "ocr_models": ocr_models,
        "cache_dir": cache_dir,
        "force_device": _string(item["force_device"], "config.execution.force_device"),
        "box_threshold": _number(item["box_threshold"], "config.execution.box_threshold", 0.0, 1.0),
        "iou_threshold": _number(item["iou_threshold"], "config.execution.iou_threshold", 0.0, 1.0),
        "use_ocr": item["use_ocr"],
        "timeout": _number(item["timeout_seconds"], "config.execution.timeout_seconds", 0.001),
        "executed_artifacts": [python_artifact, package_artifact, model_artifact, *ocr_artifacts],
    }


def _copy_bindings(config: dict[str, Any], output: Path, engine: str) -> list[dict[str, str]]:
    root = output.parent.resolve()
    directory = root / f"{output.stem}.artifacts" / engine
    directory.mkdir(parents=True, exist_ok=True)
    emitted: list[dict[str, str]] = []
    for index, item in enumerate(config["artifacts"]):
        suffix = item["source"].suffix
        destination = directory / f"{index:03d}-{item['id']}{suffix}"
        if destination.exists():
            if destination.is_symlink() or not destination.is_file() or _sha256(destination) != item["sha256"]:
                raise RunnerError(f"artifact output already exists with different content: {destination}")
        else:
            with tempfile.NamedTemporaryFile(dir=directory, prefix=".copy-", delete=False) as stream:
                temporary = Path(stream.name)
                with item["source"].open("rb") as source:
                    shutil.copyfileobj(source, stream)
            try:
                if _sha256(temporary) != item["sha256"]:
                    raise RunnerError(f"copied artifact hash changed: {item['id']}")
                os.replace(temporary, destination)
            finally:
                if temporary.exists():
                    temporary.unlink()
        emitted.append({
            "role": item["role"],
            "id": item["id"],
            "path": destination.relative_to(root).as_posix(),
            "sha256": item["sha256"],
        })
    return emitted


def _request(image: dict[str, Any], image_path: Path, request_id: str) -> dict[str, Any]:
    data = image_path.read_bytes()
    return {
        "protocol": PROTOCOL,
        "request_id": request_id,
        "method": "parse",
        "params": {
            "capture_id": image["file"],
            "image": {
                "media_type": "image/png",
                "width": image["width"],
                "height": image["height"],
                "byte_length": len(data),
                "data_base64": base64.b64encode(data).decode("ascii"),
            },
        },
    }


def _validate_closed_corpus(manifest_path: Path, corpus: list[dict[str, Any]]) -> None:
    root = manifest_path.parent.resolve()
    expected = {image["file"] for image in corpus}
    image_roots = {PurePosixPath(name).parts[0] for name in expected}
    actual: set[str] = set()
    for relative_root in image_roots:
        directory = root / relative_root
        if directory.is_symlink() or not directory.is_dir():
            raise RunnerError(f"corpus image root is not a regular directory: {relative_root}")
        for candidate in directory.rglob("*"):
            relative = candidate.relative_to(root).as_posix()
            if candidate.is_symlink():
                raise RunnerError(f"corpus image tree contains a symlink: {relative}")
            if candidate.is_file() and candidate.suffix.lower() in {".png", ".jpg", ".jpeg"}:
                actual.add(relative)
    missing = sorted(expected - actual)
    extra = sorted(actual - expected)
    if missing or extra:
        raise RunnerError(f"corpus image set is not closed; missing={missing}, extra={extra}")


def _write_frame(stream: Any, value: dict[str, Any], timeout: float | None = None) -> None:
    payload = json.dumps(value, separators=(",", ":"), allow_nan=False).encode("utf-8")
    if not payload or len(payload) > MAX_FRAME_BYTES:
        raise RunnerError("outbound frame has an invalid size")
    framed = struct.pack(">I", len(payload)) + payload
    if timeout is None:
        stream.write(framed)
        stream.flush()
        return
    deadline = time.monotonic() + timeout
    descriptor = stream.fileno()
    previous = os.get_blocking(descriptor)
    selector = selectors.DefaultSelector()
    selector.register(stream, selectors.EVENT_WRITE)
    os.set_blocking(descriptor, False)
    offset = 0
    try:
        while offset < len(framed):
            remaining = deadline - time.monotonic()
            if remaining <= 0 or not selector.select(remaining):
                raise RunnerError("engine request timed out")
            try:
                offset += os.write(descriptor, framed[offset:])
            except BlockingIOError:
                continue
    finally:
        os.set_blocking(descriptor, previous)
        selector.close()


def _read_exact(stream: Any, size: int, deadline: float) -> bytes:
    data = bytearray()
    selector = selectors.DefaultSelector()
    selector.register(stream, selectors.EVENT_READ)
    try:
        while len(data) < size:
            remaining = deadline - time.monotonic()
            if remaining <= 0 or not selector.select(remaining):
                raise RunnerError("engine response timed out")
            block = os.read(stream.fileno(), size - len(data))
            if not block:
                raise RunnerError("engine returned a truncated frame")
            data.extend(block)
    finally:
        selector.close()
    return bytes(data)


def _read_frame(stream: Any, timeout: float) -> dict[str, Any]:
    deadline = time.monotonic() + timeout
    size = struct.unpack(">I", _read_exact(stream, 4, deadline))[0]
    if size == 0 or size > MAX_FRAME_BYTES:
        raise RunnerError("engine returned an invalid frame size")
    payload = _read_exact(stream, size, deadline)
    try:
        value = json.loads(payload)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise RunnerError(f"engine returned invalid JSON: {error}") from error
    if not isinstance(value, dict):
        raise RunnerError("engine response must be an object")
    return value


def _wait_for_process(process: subprocess.Popen[bytes], timeout: float) -> int:
    deadline = time.monotonic() + timeout
    if hasattr(os, "wait4"):
        while True:
            pid, status, _ = os.wait4(process.pid, os.WNOHANG)
            if pid:
                process.returncode = os.waitstatus_to_exitcode(status)
                return process.returncode
            if time.monotonic() >= deadline:
                process.kill()
                _, status, _ = os.wait4(process.pid, 0)
                process.returncode = os.waitstatus_to_exitcode(status)
                raise RunnerError("engine did not exit before the timeout")
            time.sleep(0.01)
    try:
        returncode = process.wait(timeout=max(0.001, deadline - time.monotonic()))
    except subprocess.TimeoutExpired as error:
        process.kill()
        process.wait()
        raise RunnerError("engine did not exit before the timeout") from error
    return returncode


def _rss_bytes(pid: int) -> int:
    status = Path(f"/proc/{pid}/status")
    if status.is_file():
        for line in status.read_text(encoding="ascii").splitlines():
            if line.startswith("VmRSS:"):
                return int(line.split()[1]) * 1024
        raise RunnerError("process RSS is absent from /proc status")
    if sys.platform == "darwin":
        measured = subprocess.run(
            ["ps", "-o", "rss=", "-p", str(pid)],
            check=False,
            capture_output=True,
            text=True,
        )
        if measured.returncode != 0 or not measured.stdout.strip().isdigit():
            raise RunnerError("cannot read process RSS with ps")
        return int(measured.stdout.strip()) * 1024
    try:
        import ctypes
        from ctypes import wintypes

        class Counters(ctypes.Structure):
            _fields_ = [("cb", wintypes.DWORD), ("PageFaultCount", wintypes.DWORD)] + [(name, ctypes.c_size_t) for name in ("PeakWorkingSetSize", "WorkingSetSize", "QuotaPeakPagedPoolUsage", "QuotaPagedPoolUsage", "QuotaPeakNonPagedPoolUsage", "QuotaNonPagedPoolUsage", "PagefileUsage", "PeakPagefileUsage", "PrivateUsage")]

        counters = Counters()
        counters.cb = ctypes.sizeof(counters)
        handle = ctypes.windll.kernel32.OpenProcess(0x0400 | 0x0010, False, pid)
        if not handle:
            raise OSError("OpenProcess failed")
        try:
            ok = ctypes.windll.psapi.GetProcessMemoryInfo(handle, ctypes.byref(counters), counters.cb)
        finally:
            ctypes.windll.kernel32.CloseHandle(handle)
        if not ok:
            raise OSError("GetProcessMemoryInfo failed")
        return int(counters.WorkingSetSize)
    except Exception as error:
        raise RunnerError(f"cannot measure process RSS on this platform: {error}") from error


def _measure_peak_rss(pid: int, operation: Any) -> tuple[Any, int]:
    stop = threading.Event()
    readings: list[int] = []

    def sample() -> None:
        while not stop.is_set():
            try:
                readings.append(_rss_bytes(pid))
            except Exception:
                return
            stop.wait(0.005)

    thread = threading.Thread(target=sample, name="quality-runner-rss", daemon=True)
    thread.start()
    try:
        result = operation()
    finally:
        try:
            readings.append(_rss_bytes(pid))
        except Exception:
            pass
        stop.set()
        thread.join(timeout=1.0)
    if not readings:
        raise RunnerError("process peak RSS was unavailable")
    return result, max(readings)


def _validate_rust_response(value: dict[str, Any], request_id: str, image: dict[str, Any], extension: tuple[str, str]) -> list[dict[str, Any]]:
    response = _closed(value, ("protocol", "request_id", "status", "result"), (), "worker response")
    if response["protocol"] != PROTOCOL or response["request_id"] != request_id or response["status"] != "ok":
        raise RunnerError("worker response protocol, request id, or status is invalid")
    result = _closed(response["result"], ("capture_id", "image", "coordinate_space", "regions", "runtime", "identity", "text_geometry"), (), "worker result")
    if result["capture_id"] != image["file"] or result["coordinate_space"] != "image_pixels":
        raise RunnerError("worker result capture or coordinate space is invalid")
    image_result = _closed(result["image"], ("sha256", "width", "height"), (), "worker result.image")
    if image_result != {"sha256": image["sha256"], "width": image["width"], "height": image["height"]}:
        raise RunnerError("worker result image identity does not match the corpus")
    identity = result["identity"]
    if not isinstance(identity, dict) or identity.get("extension") != {"id": extension[0], "version": extension[1]}:
        raise RunnerError("worker result extension identity does not match the config")
    regions = result["regions"]
    if not isinstance(regions, list):
        raise RunnerError("worker result.regions must be an array")
    return [_normalize_rust_region(region, index, image) for index, region in enumerate(regions)]


def _normalize_rust_region(value: Any, index: int, image: dict[str, Any]) -> dict[str, Any]:
    where = f"worker result.regions[{index}]"
    item = _closed(value, ("id", "kind", "bounds", "confidence"), ("text", "label", "class_id"), where)
    if item["kind"] not in {"text", "icon"}:
        raise RunnerError(f"{where}.kind is unsupported")
    bounds = _closed(item["bounds"], ("x", "y", "width", "height"), (), f"{where}.bounds")
    normalized_bounds = {key: _number(bounds[key], f"{where}.bounds.{key}") for key in ("x", "y", "width", "height")}
    if normalized_bounds["width"] <= 0 or normalized_bounds["height"] <= 0 or normalized_bounds["x"] + normalized_bounds["width"] > image["width"] or normalized_bounds["y"] + normalized_bounds["height"] > image["height"]:
        raise RunnerError(f"{where}.bounds is empty or outside the image")
    result: dict[str, Any] = {"id": _string(item["id"], f"{where}.id"), "kind": item["kind"], "bounds": normalized_bounds, "confidence": _number(item["confidence"], f"{where}.confidence", 0.0, 1.0)}
    if item["kind"] == "text":
        result["text"] = _string(item.get("text"), f"{where}.text")
    elif "label" in item:
        result["label"] = _string(item["label"], f"{where}.label")
    return result


def _run_rust_image(execution: dict[str, Any], image: dict[str, Any], image_path: Path) -> dict[str, Any]:
    _verify_bound_artifacts(execution["executed_artifacts"], "Rust execution")
    command = [str(execution["worker"]), "--manifest", str(execution["manifest"]), "--onnx-runtime-library", str(execution["runtime"]), "--extension-id", execution["extension_id"], "--extension-version", execution["extension_version"]]
    with tempfile.TemporaryFile() as stderr:
        start = time.perf_counter_ns()
        process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=stderr)
        assert process.stdin is not None and process.stdout is not None
        try:
            cold_id = "quality-cold"
            def cold_call() -> dict[str, Any]:
                _write_frame(process.stdin, _request(image, image_path, cold_id), execution["timeout"])
                return _read_frame(process.stdout, execution["timeout"])

            cold_response, cold_peak = _measure_peak_rss(process.pid, cold_call)
            cold_ms = (time.perf_counter_ns() - start) / 1_000_000
            regions = _validate_rust_response(cold_response, cold_id, image, (execution["extension_id"], execution["extension_version"]))
            warm_start = time.perf_counter_ns()
            warm_id = "quality-warm"
            def warm_call() -> dict[str, Any]:
                _write_frame(process.stdin, _request(image, image_path, warm_id), execution["timeout"])
                return _read_frame(process.stdout, execution["timeout"])

            warm_response, warm_peak = _measure_peak_rss(process.pid, warm_call)
            warm_ms = (time.perf_counter_ns() - warm_start) / 1_000_000
            warm_regions = _validate_rust_response(warm_response, warm_id, image, (execution["extension_id"], execution["extension_version"]))
            if warm_regions != regions:
                raise RunnerError("worker returned different cold and warm regions")
            process.stdin.close()
            process.stdout.close()
            returncode = _wait_for_process(process, execution["timeout"])
            stderr.seek(0)
            error_text = stderr.read().decode("utf-8", errors="replace").strip()
            if returncode != 0:
                raise RunnerError(f"worker exited {returncode}: {error_text}")
            return _image_result(
                image, regions, "source_pixels", image["width"], image["height"],
                cold_ms, warm_ms, cold_peak, warm_peak,
            )
        except Exception:
            if process.poll() is None:
                process.kill()
                try:
                    _wait_for_process(process, 5.0)
                except Exception:
                    pass
            raise


def _image_result(image: dict[str, Any], regions: list[dict[str, Any]], space: str, width: float, height: float, cold_ms: float, warm_ms: float, cold_peak: int, warm_peak: int) -> dict[str, Any]:
    return {
        "file": image["file"],
        "image_sha256": image["sha256"],
        "coordinate_space": {"space": space, "origin": "top_left", "bbox_format": "x_y_width_height", "width": width, "height": height},
        "regions": regions,
        "samples": [
            {"phase": "cold", "latency_ms": cold_ms, "sampled_process_peak_rss_bytes": cold_peak},
            {"phase": "warm", "latency_ms": warm_ms, "sampled_process_peak_rss_bytes": warm_peak},
        ],
    }


def _extract_bound_wheel(wheel: Path, expected_sha256: str, destination: Path) -> None:
    if _sha256(wheel) != expected_sha256:
        raise RunnerError("bound Python wheel changed after configuration validation")
    try:
        with zipfile.ZipFile(wheel) as archive:
            members = archive.infolist()
            if len(members) > 100_000 or sum(member.file_size for member in members) > 2 * 1024 * 1024 * 1024:
                raise RunnerError("bound Python wheel exceeds extraction limits")
            seen: set[str] = set()
            for member in members:
                name = member.filename
                relative = PurePosixPath(name)
                if member.flag_bits & 1 or relative.is_absolute() or not relative.parts or any(part in {"", ".", ".."} for part in relative.parts) or "\\" in name:
                    raise RunnerError("bound Python wheel contains an unsafe member")
                normalized = relative.as_posix().rstrip("/")
                if normalized in seen:
                    raise RunnerError("bound Python wheel contains duplicate members")
                seen.add(normalized)
                mode = member.external_attr >> 16
                if (mode & 0o170000) == 0o120000:
                    raise RunnerError("bound Python wheel contains a symlink")
                target = destination.joinpath(*relative.parts)
                if member.is_dir():
                    target.mkdir(parents=True, exist_ok=True)
                    continue
                target.parent.mkdir(parents=True, exist_ok=True)
                with archive.open(member) as source, target.open("xb") as output:
                    shutil.copyfileobj(source, output)
    except (OSError, zipfile.BadZipFile) as error:
        raise RunnerError(f"cannot extract bound Python wheel: {error}") from error


def _python_child_invocation(execution: dict[str, Any], options: dict[str, Any]) -> tuple[list[str], dict[str, str]]:
    environment = {key: value for key, value in os.environ.items() if not key.startswith("PYTHON")}
    environment.update({
        "PYTHONHOME": str(execution["python_home"]),
        "PYTHONNOUSERSITE": "1",
        "HF_HUB_OFFLINE": "1",
        "TRANSFORMERS_OFFLINE": "1",
        "EASYOCR_MODULE_PATH": str(execution["cache_dir"]),
    })
    command = [
        str(execution["python"]),
        "-S",
        str(Path(__file__).resolve()),
        "--python-child",
        json.dumps(options, separators=(",", ":")),
    ]
    return command, environment


def _run_python_image(execution: dict[str, Any], image: dict[str, Any], image_path: Path) -> dict[str, Any]:
    _verify_bound_artifacts(execution["executed_artifacts"], "Python execution")
    with tempfile.TemporaryDirectory(prefix="cua-quality-wheel-") as temporary, tempfile.TemporaryFile() as stderr:
        import_root = Path(temporary).resolve()
        _extract_bound_wheel(execution["package"], execution["package_sha256"], import_root)
        options = {
            "import_root": str(import_root), "site_packages": str(execution["site_packages"]),
            "model": str(execution["model"]), "cache_dir": str(execution["cache_dir"]),
            "force_device": execution["force_device"], "box_threshold": execution["box_threshold"], "iou_threshold": execution["iou_threshold"],
            "use_ocr": execution["use_ocr"], "image": str(image_path),
        }
        command, environment = _python_child_invocation(execution, options)
        cold_start = time.perf_counter_ns()
        process = subprocess.Popen(
            command,
            stdout=subprocess.PIPE,
            stderr=stderr,
            env=environment,
        )
        assert process.stdout is not None
        try:
            cold_response, cold_peak = _measure_peak_rss(
                process.pid, lambda: _read_frame(process.stdout, execution["timeout"])
            )
            cold_ms = (time.perf_counter_ns() - cold_start) / 1_000_000
            if cold_response.get("status") != "ok":
                raise RunnerError(f"Python engine failed: {cold_response.get('error', 'invalid cold response')}")
            cold = _closed(cold_response, ("status", "phase", "regions"), (), "Python cold response")
            if cold["phase"] != "cold":
                raise RunnerError("Python engine returned an invalid cold response")
            warm_start = time.perf_counter_ns()
            warm_response, warm_peak = _measure_peak_rss(
                process.pid, lambda: _read_frame(process.stdout, execution["timeout"])
            )
            warm_ms = (time.perf_counter_ns() - warm_start) / 1_000_000
            process.stdout.close()
            returncode = _wait_for_process(process, execution["timeout"])
            stderr.seek(0)
            error_text = stderr.read().decode("utf-8", errors="replace").strip()
            if returncode != 0 or warm_response.get("status") != "ok":
                raise RunnerError(f"Python engine failed: {warm_response.get('error', error_text)}")
            warm = _closed(warm_response, ("status", "phase", "regions"), (), "Python warm response")
            if warm["phase"] != "warm" or warm["regions"] != cold["regions"]:
                raise RunnerError("Python engine returned an invalid or inconsistent warm response")
            return _image_result(
                image, cold["regions"], "source_pixels", image["width"], image["height"],
                cold_ms, warm_ms, cold_peak, warm_peak,
            )
        except Exception:
            if process.poll() is None:
                process.kill()
                try:
                    _wait_for_process(process, 5.0)
                except Exception:
                    pass
            raise


def _python_regions(result: Any, width: int, height: int) -> list[dict[str, Any]]:
    elements = getattr(result, "elements", None)
    if not isinstance(elements, list):
        raise RunnerError("OmniParser result.elements must be a list")
    regions: list[dict[str, Any]] = []
    for index, element in enumerate(elements):
        where = f"OmniParser result.elements[{index}]"
        kind = getattr(element, "type", None)
        if kind not in {"text", "icon"}:
            raise RunnerError(f"{where}.type is unsupported")
        bbox = getattr(element, "bbox", None)
        coordinates = getattr(bbox, "coordinates", None)
        if not isinstance(coordinates, (list, tuple)) or len(coordinates) != 4:
            raise RunnerError(f"{where}.bbox must provide normalized xyxy coordinates")
        x1, y1, x2, y2 = (_number(value, f"{where}.bbox", 0.0, 1.0) for value in coordinates)
        if x2 <= x1 or y2 <= y1:
            raise RunnerError(f"{where}.bbox is empty or inverted")
        region: dict[str, Any] = {
            "id": str(getattr(element, "id", index + 1)),
            "kind": kind,
            "bounds": {
                "x": x1 * width,
                "y": y1 * height,
                "width": (x2 - x1) * width,
                "height": (y2 - y1) * height,
            },
            "confidence": _number(getattr(element, "confidence", None), f"{where}.confidence", 0.0, 1.0),
        }
        if kind == "text":
            region["text"] = _string(getattr(element, "content", None), f"{where}.content")
        regions.append(region)
    return regions


def _is_som_module(name: str) -> bool:
    return name == "som" or name.startswith("som.")


def _clear_som_modules() -> None:
    for name in tuple(sys.modules):
        if _is_som_module(name):
            del sys.modules[name]


def _bound_module_path(value: Any, import_root: Path, where: str, *, directory: bool = False) -> Path:
    if not isinstance(value, (str, os.PathLike)):
        raise RunnerError(f"{where} does not identify a real path")
    try:
        path = Path(value).resolve(strict=True)
    except (OSError, RuntimeError, TypeError, ValueError) as error:
        raise RunnerError(f"{where} does not identify a real path") from error
    if path != import_root and import_root not in path.parents:
        raise RunnerError(f"{where} is outside the extracted bound wheel")
    if (directory and not path.is_dir()) or (not directory and not path.is_file()):
        raise RunnerError(f"{where} does not identify a real {'directory' if directory else 'file'}")
    return path


def _validate_bound_som_modules(import_root: Path) -> None:
    names = sorted(name for name in sys.modules if _is_som_module(name))
    if "som" not in names:
        raise RunnerError("bound som package is not loaded")
    for name in names:
        module = sys.modules[name]
        if module is None or getattr(module, "__name__", None) != name:
            raise RunnerError(f"loaded {name} module has an invalid module identity")
        module_file = _bound_module_path(getattr(module, "__file__", None), import_root, f"loaded {name} module")
        spec = getattr(module, "__spec__", None)
        loader = getattr(module, "__loader__", None)
        if spec is None or spec.name != name or spec.loader is None or loader is not spec.loader:
            raise RunnerError(f"loaded {name} module has an invalid loader specification")
        origin = _bound_module_path(spec.origin, import_root, f"loaded {name} module origin")
        if origin != module_file:
            raise RunnerError(f"loaded {name} module origin does not match its file")
        get_filename = getattr(loader, "get_filename", None)
        if not callable(get_filename):
            raise RunnerError(f"loaded {name} module loader cannot prove its source file")
        try:
            loader_file = _bound_module_path(get_filename(name), import_root, f"loaded {name} module loader source")
        except (ImportError, AttributeError) as error:
            raise RunnerError(f"loaded {name} module loader cannot prove its source file") from error
        if loader_file != module_file:
            raise RunnerError(f"loaded {name} module loader source does not match its file")
        search_locations = spec.submodule_search_locations
        module_path = getattr(module, "__path__", None)
        if (search_locations is None) != (module_path is None):
            raise RunnerError(f"loaded {name} module has inconsistent package search paths")
        if search_locations is not None:
            locations = list(search_locations)
            paths = list(module_path)
            if not locations or locations != paths:
                raise RunnerError(f"loaded {name} package has invalid search paths")
            for index, location in enumerate(locations):
                _bound_module_path(location, import_root, f"loaded {name} package search path[{index}]", directory=True)


def _python_child(encoded: str) -> int:
    try:
        options = json.loads(encoded)
        os.environ.update({"HF_HUB_OFFLINE": "1", "TRANSFORMERS_OFFLINE": "1", "EASYOCR_MODULE_PATH": options["cache_dir"]})
        import socket

        class OfflineSocket(socket.socket):
            def connect(self, address: Any) -> None:
                raise OSError("network access is disabled by quality_runner")

            def connect_ex(self, address: Any) -> int:
                raise OSError("network access is disabled by quality_runner")

        socket.socket = OfflineSocket
        import_root = Path(options["import_root"]).resolve()
        site_packages = Path(options["site_packages"]).resolve()
        if not site_packages.is_dir() or site_packages.is_symlink():
            raise RunnerError("configured site_packages is not a regular directory")
        sys.path.append(str(site_packages))
        sys.path.insert(0, str(import_root))
        _clear_som_modules()
        som = importlib.import_module("som")
        _validate_bound_som_modules(import_root)
        image_path = Path(options["image"])
        image = image_path.read_bytes()
        width, height = measure_quality._png_dimensions(image_path)

        def cold_call() -> tuple[Any, Any]:
            parser_instance = som.OmniParser(model_path=options["model"], cache_dir=options["cache_dir"], force_device=options["force_device"])
            _validate_bound_som_modules(import_root)
            result = parser_instance.parse(image, box_threshold=options["box_threshold"], iou_threshold=options["iou_threshold"], use_ocr=options["use_ocr"])
            _validate_bound_som_modules(import_root)
            return parser_instance, result

        parser, cold = cold_call()
        if getattr(getattr(parser, "detector", None), "model", None) is None:
            raise RunnerError("OmniParser did not load the configured detector model")
        if options["use_ocr"] and getattr(getattr(parser, "ocr", None), "reader", None) is None:
            raise RunnerError("OmniParser did not load its OCR models from the configured cache")
        regions = _python_regions(cold, width, height)
        _write_frame(sys.stdout.buffer, {"status": "ok", "phase": "cold", "regions": regions})
        warm = parser.parse(image, box_threshold=options["box_threshold"], iou_threshold=options["iou_threshold"], use_ocr=options["use_ocr"])
        _validate_bound_som_modules(import_root)
        if _python_regions(warm, width, height) != regions:
            raise RunnerError("OmniParser returned different cold and warm regions")
        _write_frame(sys.stdout.buffer, {"status": "ok", "phase": "warm", "regions": regions})
        return 0
    except Exception as error:
        try:
            _write_frame(sys.stdout.buffer, {"status": "error", "error": str(error)})
        except Exception:
            pass
        return 1


def build_results(engine: str, config_path: Path, manifest_path: Path, output: Path) -> dict[str, Any]:
    config_path = config_path.absolute()
    manifest_path = manifest_path.absolute()
    config = _load_config(config_path, engine)
    try:
        manifest, _ = measure_quality.read_json(manifest_path)
        corpus = measure_quality.validate_manifest(manifest, manifest_path)
        _validate_closed_corpus(manifest_path, corpus)
    except Exception as error:
        raise RunnerError(str(error)) from error
    images = []
    for image in corpus:
        image_path = manifest_path.parent.resolve() / PurePosixPath(image["file"])
        if engine == "rust":
            images.append(_run_rust_image(config["execution"], image, image_path))
        else:
            images.append(_run_python_image(config["execution"], image, image_path))
    bindings = {
        "artifacts": _copy_bindings(config, output, engine),
        "package_artifact_id": config["package_artifact_id"],
        "installed_artifact_ids": config["installed_artifact_ids"],
    }
    result = {"schema_version": 1, "engine": engine, "identity": config["identity"], "bindings": bindings, "images": images}
    try:
        measure_quality.validate_results(result, engine, corpus, output)
    except Exception as error:
        raise RunnerError(f"generated results failed validation: {error}") from error
    return result


def main(argv: list[str] | None = None) -> int:
    args_list = sys.argv[1:] if argv is None else argv
    if args_list[:1] == ["--python-child"]:
        if len(args_list) != 2:
            return 2
        return _python_child(args_list[1])
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--engine", required=True, choices=("rust", "python"))
    parser.add_argument("--config", required=True, type=Path)
    parser.add_argument("--manifest", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args(args_list)
    try:
        if args.output.is_symlink() or (args.output.exists() and not args.output.is_file()):
            raise RunnerError(f"output is not a regular file: {args.output}")
        output = args.output.absolute()
        output.parent.mkdir(parents=True, exist_ok=True)
        result = build_results(args.engine, args.config, args.manifest, output)
        encoded = measure_quality.canonical_json(result)
        with tempfile.NamedTemporaryFile("w", encoding="utf-8", dir=output.parent, prefix=f".{output.name}.", delete=False) as stream:
            temporary = Path(stream.name)
            stream.write(encoded)
        try:
            os.replace(temporary, output)
        finally:
            if temporary.exists():
                temporary.unlink()
        return 0
    except (RunnerError, OSError) as error:
        print(f"quality_runner: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
