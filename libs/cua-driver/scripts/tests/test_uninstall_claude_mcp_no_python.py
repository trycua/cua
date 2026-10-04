"""Missing Python must fail before release or configuration mutation."""

import json


def test_claude_config_requires_python_before_release_is_mutated(release_install):
    f = release_install
    original = {"mcpServers": {"cua-computer-use": {"command": str(f.launcher), "args": ["mcp"]}}}
    f.write_config(original)
    f.env["PATH"] = str(f.bin)
    result = f.run()
    assert result.returncode != 0
    assert "python3 is required to safely inspect Claude MCP ownership" in result.stderr
    assert json.loads(f.config.read_text()) == original
    assert f.release.exists()
    assert f.launcher.is_symlink()
