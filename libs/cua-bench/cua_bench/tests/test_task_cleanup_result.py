"""Regression for preserving failed task results through cleanup errors."""
import asyncio
import json
from pathlib import Path

import pytest

from cua_bench.runner import task_runner as module


@pytest.mark.asyncio
async def test_cleanup_exception_preserves_timeout_result_and_receipt(tmp_path, monkeypatch):
    runner = module.TaskRunner()
    events = []

    async def create_network(*args, **kwargs):
        return None

    async def start_agent(**kwargs):
        return None

    async def timeout(*args, **kwargs):
        raise asyncio.TimeoutError()

    async def logs(*args, **kwargs):
        return "logs"

    async def fail_cleanup(task_id):
        events.append("cleanup")
        raise RuntimeError("injected image removal failure")

    async def receipt(task_id, output_dir):
        events.append("receipt")
        (Path(output_dir) / "cleanup-result.json").write_text(
            json.dumps({"verified": True}), encoding="utf-8"
        )

    monkeypatch.setattr(module, "create_network", create_network)
    monkeypatch.setattr(module, "wait_for_container", timeout)
    monkeypatch.setattr(module, "get_container_logs", logs)
    monkeypatch.setattr(runner, "_start_agent_container", start_agent)
    monkeypatch.setattr(runner, "_cleanup_task", fail_cleanup)
    monkeypatch.setattr(runner, "_write_cleanup_receipt", receipt)

    result = await runner.run_task(
        env_path=tmp_path, task_index=0, env_type="linux-docker",
        provider_type="simulated", cleanup_before=False, output_dir=str(tmp_path),
    )
    assert result.success is False
    assert "Task timed out" in result.error
    assert "Task cleanup failed: RuntimeError" in result.error
    assert events == ["cleanup", "receipt"]


@pytest.mark.asyncio
async def test_receipt_write_failure_keeps_original_timeout_reason(tmp_path, monkeypatch):
    runner = module.TaskRunner()

    async def noop(*args, **kwargs):
        return None

    async def timeout(*args, **kwargs):
        raise asyncio.TimeoutError()

    async def logs(*args, **kwargs):
        return "logs"

    async def broken_receipt(*args, **kwargs):
        raise OSError("injected disk full")

    monkeypatch.setattr(module, "create_network", noop)
    monkeypatch.setattr(module, "wait_for_container", timeout)
    monkeypatch.setattr(module, "get_container_logs", logs)
    monkeypatch.setattr(runner, "_start_agent_container", noop)
    monkeypatch.setattr(runner, "_cleanup_task", noop)
    monkeypatch.setattr(runner, "_write_cleanup_receipt", broken_receipt)

    result = await runner.run_task(
        env_path=tmp_path, task_index=0, env_type="linux-docker",
        provider_type="simulated", cleanup_before=False, output_dir=str(tmp_path),
    )
    assert result.success is False
    assert "Task timed out" in result.error
    assert "Unable to verify Docker cleanup: OSError" in result.error


@pytest.mark.asyncio
async def test_partial_overlay_failure_triggers_registered_cleanup(tmp_path, monkeypatch):
    runner = module.TaskRunner()
    seen = []
    monkeypatch.setattr(module, "generate_task_id", lambda: "overlay-test")
    monkeypatch.setitem(module.ENV_CONFIGS, "overlay-test-env", {"use_overlays": True})
    monkeypatch.setattr(module, "get_overlays_path", lambda: tmp_path)

    def partial_overlay(task_id):
        (tmp_path / task_id).mkdir()
        raise OSError("injected overlay creation failure")

    async def cleanup(task_id):
        tracked = runner._running_tasks.pop(task_id)
        seen.append(tracked["overlay_path"])
        tracked["overlay_path"].rmdir()

    async def receipt(task_id, output_dir):
        seen.append("receipt")

    monkeypatch.setattr(runner, "_create_task_overlay", partial_overlay)
    monkeypatch.setattr(runner, "_cleanup_task", cleanup)
    monkeypatch.setattr(runner, "_write_cleanup_receipt", receipt)

    with pytest.raises(OSError, match="injected overlay creation failure"):
        await runner.run_task(
            env_path=tmp_path, task_index=0, env_type="overlay-test-env",
            cleanup_before=False, output_dir=str(tmp_path),
        )
    assert seen == [tmp_path / "overlay-test", "receipt"]
    assert not (tmp_path / "overlay-test").exists()
