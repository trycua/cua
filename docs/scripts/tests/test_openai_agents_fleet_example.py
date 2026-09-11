from __future__ import annotations

import ast
import os
from pathlib import Path
import secrets
from unittest.mock import patch


DOCS_ROOT = Path(__file__).resolve().parents[2]
SCRIPT = (
    DOCS_ROOT
    / "public/scripts/openai-agents-fleet/run_openai_agents_fleet.py"
)
GUIDE = (
    DOCS_ROOT
    / "content/docs/how-to-guides/sandbox/run-openai-agents-api-on-cloud-fleet.mdx"
)
META = DOCS_ROOT / "content/docs/how-to-guides/sandbox/meta.json"


def test_disposable_pool_name_ignores_existing_pool_override() -> None:
    tree = ast.parse(SCRIPT.read_text())
    function = next(node for node in tree.body if isinstance(node, ast.FunctionDef)
                    and node.name == "pool_name")
    namespace = {"os": os, "secrets": secrets}
    exec(compile(ast.Module(body=[function], type_ignores=[]), str(SCRIPT), "exec"), namespace)
    with patch.dict(os.environ, {"CUA_POOL_NAME": "existing-production-pool"}):
        first = namespace["pool_name"]()
        second = namespace["pool_name"]()
    assert first != second
    assert first.startswith("cua-openai-agents-")
    assert len(first.removeprefix("cua-openai-agents-")) == 24
    assert len(f"{first}-a84951d4-server".encode()) <= 63


def test_controller_is_valid_python() -> None:
    source = SCRIPT.read_text()
    compile(source, str(SCRIPT), "exec")
    ast.parse(source)


def test_controller_keeps_application_key_outside_the_sandbox() -> None:
    source = SCRIPT.read_text()
    assert 'application_key = required_env("OPENAI_API_KEY")' in source
    assert 'environment_key = required_env("CODEX_API_KEY")' in source
    assert 'f"CODEX_API_KEY={shlex.quote(environment_key)}' in source
    assert 'f"OPENAI_API_KEY={shlex.quote(' not in source
    assert "await sandbox.files.write_bytes(" in source
    assert "rm -f {shlex.quote(EXECUTOR_ENV_PATH)}" in source


def test_controller_covers_reconnect_artifact_and_cleanup() -> None:
    source = SCRIPT.read_text()
    assert 'wait_for_type("agent.session.environment.disconnected")' in source
    assert source.count("await start_executor(") >= 2
    assert "await sandbox.files.read_bytes(ARTIFACT_PATH)" in source
    assert "Agent-created Fleet artifact content did not match" in source
    assert 'page.get("has_more")' in source
    assert 'page.get("last_id")' in source
    assert "await agents.delete_session" in source
    assert "await pool.delete()" in source
    assert 'response.status_code == 409' in source
    assert "contextlib.suppress(Exception)" not in source
    assert 'raise ExceptionGroup("Resource cleanup failed", cleanup_errors)' in source
    assert 'last_status.startswith("READY:")' in source
    assert "sanitized log tail" in source
    assert "echo $$ > {shlex.quote(EXECUTOR_PID_PATH)}" in source
    assert "background=True" in source


def test_controller_registers_cua_driver_as_required_stdio_mcp() -> None:
    source = SCRIPT.read_text()
    assert '"server_label": "cua_driver"' in source
    assert '"type": "stdio"' in source
    assert '"command": "/usr/bin/python3"' in source
    assert '"/root/.local/bin/cua-driver"' in source
    assert '"cwd": "/workspace"' in source
    assert '"CUA_DRIVER_PERMISSION_MODE"' in source
    assert '"CUA_DRIVER_DANGEROUSLY_BYPASS_APPROVALS"' in source
    assert '"DISPLAY"' in source
    assert "export CUA_DRIVER_PERMISSION_MODE=unrestricted" in source
    assert "export CUA_DRIVER_DANGEROUSLY_BYPASS_APPROVALS=1" in source
    assert '"required": True' in source
    assert '"allowed_tools": [' in source
    assert '"launch_app"' in source
    assert 'message.get("method") == "tools/call"' in source
    assert "MCP_AUDIT_LOG_PATH" in source
    assert 'terminal_state != "CLOSED"' in source
    assert 'audit_names.count("get_window_state") < 2' in source
    assert 'audit_names.count("click") < 2' in source


def test_guide_links_the_download_and_navigation_entry() -> None:
    guide = GUIDE.read_text()
    meta = META.read_text()
    assert "https://cua.ai/scripts/openai-agents-fleet/run_openai_agents_fleet.py" in guide
    assert '"run-openai-agents-api-on-cloud-fleet"' in meta
