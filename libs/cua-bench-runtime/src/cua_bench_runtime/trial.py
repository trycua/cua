"""Immutable trial input materialization."""

from __future__ import annotations

import json
import os
import shutil
from pathlib import Path
from typing import Any

from cua_bench_runtime.canon import canonical_json, digest_file, digest_json
from cua_bench_runtime.errors import UsageFailure, ValidationFailure
from cua_bench_runtime.schemas import task_artifacts


def _safe_source(root: Path, value: str) -> Path:
    source = (root / value).resolve()
    if not source.is_relative_to(root.resolve()):
        raise ValidationFailure(f"artifact path escapes task directory: {value}")
    return source


def materialize_trial(
    out: Path,
    trial_id: str,
    task_path: Path,
    task: dict[str, Any],
    config: dict[str, Any],
    agent_command: Path,
    additional_inputs: tuple[tuple[Path, str], ...] = (),
    agent_brief: bytes | None = None,
) -> tuple[Path, str, str, Path, Path]:
    out.mkdir(parents=True, exist_ok=True)
    trial_dir = out / trial_id
    try:
        trial_dir.mkdir()
    except FileExistsError as error:
        raise UsageFailure(f"trial directory already exists: {trial_dir}") from error

    inputs = trial_dir / "inputs"
    artifacts = trial_dir / "artifacts"
    harness_workspace = trial_dir / "harness-workspace"
    inputs.mkdir()
    artifacts.mkdir()
    harness_workspace.mkdir()

    config_digest = digest_json(config)
    config_path = trial_dir / "config.json"
    with config_path.open("xb") as handle:
        handle.write(canonical_json(config) + b"\n")
        handle.flush()
        os.fsync(handle.fileno())
    try:
        config_path.chmod(0o444)
    except OSError:
        pass

    copied: dict[str, str] = {}
    task_target = inputs / "task.cuabench.json"
    shutil.copy2(task_path, task_target)
    task_target.chmod(0o444)
    copied["task.cuabench.json"] = digest_file(task_target)

    for declaration in task_artifacts(task):
        relative = Path(declaration["path"])
        source = _safe_source(task_path.parent, declaration["path"])
        target = inputs / "artifacts" / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        if target.exists():
            continue
        shutil.copy2(source, target)
        target.chmod(target.stat().st_mode & ~0o222)
        copied[target.relative_to(inputs).as_posix()] = digest_file(target)

    agent_target = inputs / "runtime" / "agent" / agent_command.name
    agent_target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(agent_command, agent_target)
    agent_target.chmod(agent_target.stat().st_mode & ~0o222)
    copied[agent_target.relative_to(inputs).as_posix()] = digest_file(agent_target)

    for source, relative_name in additional_inputs:
        source = source.resolve()
        target = (inputs / relative_name).resolve()
        if not target.is_relative_to(inputs.resolve()):
            raise ValidationFailure(
                f"additional input path escapes input directory: {relative_name}"
            )
        if not source.is_file():
            raise ValidationFailure(f"additional input is missing: {source.name}")
        target.parent.mkdir(parents=True, exist_ok=True)
        if target.exists():
            if digest_file(target) != digest_file(source):
                raise ValidationFailure(f"additional input collision: {relative_name}")
            continue
        shutil.copy2(source, target)
        target.chmod(target.stat().st_mode & ~0o222)
        copied[target.relative_to(inputs.resolve()).as_posix()] = digest_file(target)

    if agent_brief is not None:
        brief_target = inputs / "apparatus" / "agent-brief.md"
        brief_target.parent.mkdir(parents=True, exist_ok=True)
        with brief_target.open("xb") as handle:
            handle.write(agent_brief)
            handle.flush()
            os.fsync(handle.fileno())
        brief_target.chmod(0o444)
        copied[brief_target.relative_to(inputs).as_posix()] = digest_file(brief_target)

    evaluator_target = inputs / "artifacts" / task["evaluator"]["entrypoint"]

    manifest = {
        "schema_version": task["schema_version"],
        "files": dict(sorted(copied.items())),
    }
    manifest_path = trial_dir / "inputs.manifest.json"
    with manifest_path.open("xb") as handle:
        handle.write(canonical_json(manifest) + b"\n")
        handle.flush()
        os.fsync(handle.fileno())
    try:
        manifest_path.chmod(0o444)
    except OSError:
        pass
    return (
        trial_dir,
        config_digest,
        digest_file(manifest_path),
        agent_target,
        evaluator_target,
    )


def write_result(path: Path, result: dict[str, Any]) -> None:
    temporary = path.with_suffix(".json.tmp")
    with temporary.open("xb") as handle:
        handle.write(canonical_json(result) + b"\n")
        handle.flush()
        os.fsync(handle.fileno())
    os.replace(temporary, path)


def read_config(path: Path) -> dict[str, Any]:
    return json.loads(path.read_text(encoding="utf-8"))
