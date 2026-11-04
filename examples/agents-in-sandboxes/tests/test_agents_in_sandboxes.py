"""Hermetic tests for the example's pure logic (no SDK, no sandbox).

    uv run --no-project --with pytest pytest examples/agents-in-sandboxes/tests -q
"""

from __future__ import annotations

import ast
import json
import re
import subprocess
import sys
from pathlib import Path

import pytest

HERE = Path(__file__).resolve().parent
EXAMPLE = HERE.parent
REPO = EXAMPLE.parents[1]
sys.path.insert(0, str(EXAMPLE))

import agents_in_sandboxes as ais  # noqa: E402


def test_the_shipped_task_file_parses():
    tasks = ais.parse_tasks((EXAMPLE / "tasks.jsonl").read_text())
    assert [t.id for t in tasks] == ["hello", "primes"]
    assert all(t.check and t.mock for t in tasks)


def test_jsonl_and_json_array_forms():
    jsonl = '# comment\n\n{"id": "a", "prompt": "do a"}\n{"prompt": "do b", "timeout_s": 5}\n'
    arr = json.dumps([{"id": "a", "prompt": "do a"}, {"prompt": "do b", "timeout_s": 5}])
    for text in (jsonl, arr):
        a, b = ais.parse_tasks(text)
        assert (a.id, a.prompt, a.check, a.timeout_s) == ("a", "do a", None, 900)
        assert (b.id, b.timeout_s) == ("task-2", 5)


@pytest.mark.parametrize(
    "text, msg",
    [
        ("", "no tasks"),
        ('{"id": "a"}', "prompt is required"),
        ('{"id": "../x", "prompt": "p"}', "must match"),
        ('{"id": "a", "prompt": "p"}\n{"id": "a", "prompt": "q"}', "duplicate id"),
        ('{"id": "a", "prompt": "p", "extra": 1}', "unknown fields"),
        ('{"id": "a", "prompt": "p", "timeout_s": 0}', "positive"),
        ('{"id": "a", "prompt": "p"}\nnot json', "line 2"),
        ("[1]", "expected an object"),
    ],
)
def test_bad_task_files_are_rejected(text, msg):
    with pytest.raises(ValueError, match=msg):
        ais.parse_tasks(text)


def test_scripted_prompts_append_the_mock_directive_only_when_asked():
    t = ais.Task("a", "Create x", mock="shell touch x")
    assert t.prompt_for(False) == "Create x"
    assert t.prompt_for(True) == "Create x mock: shell touch x"
    assert ais.Task("b", "Create y").prompt_for(True) == "Create y"


def test_mcp_flags():
    assert ais.parse_mcp("docs=https://example.com/mcp") == {
        "name": "docs",
        "url": "https://example.com/mcp",
    }
    assert ais.parse_mcp("fs=cmd:npx -y @modelcontextprotocol/server-filesystem '/tmp/a b'") == {
        "name": "fs",
        "command": "npx",
        "args": ["-y", "@modelcontextprotocol/server-filesystem", "/tmp/a b"],
    }
    for bad in ("noequals", "=http://x", "x=", "x=ftp://h", "x=cmd:"):
        with pytest.raises(ValueError):
            ais.parse_mcp(bad)


def test_the_verifier_runs_in_the_task_directory_quoted():
    t = ais.Task("hello", "p", check="grep -qx hello hello.txt")
    cwd = ais.task_dir("/tmp/work dir/", t)
    assert cwd == "/tmp/work dir/hello"
    line = ais.check_command(cwd, t.check)
    assert line == "cd '/tmp/work dir/hello' && grep -qx hello hello.txt"


def test_the_verifier_line_really_runs(tmp_path):
    (tmp_path / "hello").mkdir()
    (tmp_path / "hello" / "hello.txt").write_text("hello\n")
    tasks = {t.id: t for t in ais.parse_tasks((EXAMPLE / "tasks.jsonl").read_text())}
    line = ais.check_command(ais.task_dir(str(tmp_path), tasks["hello"]), tasks["hello"].check)
    assert subprocess.run(["sh", "-c", line], timeout=30).returncode == 0
    (tmp_path / "hello" / "hello.txt").write_text("nope\n")
    assert subprocess.run(["sh", "-c", line], timeout=30).returncode != 0


def test_usage_is_normalized():
    u = '{"inputTokens": 43019, "outputTokens": 83, "cachedReadTokens": 0, "totalTokens": 43102}'
    assert ais.parse_usage(u) == {
        "input_tokens": 43019,
        "output_tokens": 83,
        "cached_read_tokens": 0,
        "total_tokens": 43102,
    }
    assert ais.parse_usage(None) == {}
    assert ais.parse_usage("not json") == {}
    assert ais.parse_usage("[1]") == {}
    assert ais.parse_usage('{"flag": true, "n": 2}') == {"n": 2}


def test_report_summary():
    outcomes = [
        ais.TaskOutcome("a", "run-1", "idle", usage={"input_tokens": 10, "output_tokens": 2}, verified=True),
        ais.TaskOutcome("b", "run-2", "failed", verified=False, error="boom"),
        ais.TaskOutcome("c", "run-3", "running"),
        ais.TaskOutcome("d", None, "not_started"),
        ais.TaskOutcome("e", "run-5", "idle", usage={"input_tokens": 5, "output_tokens": 1}),
    ]
    r = ais.build_report({"image": "img", "harness": "claude-code"}, outcomes)
    assert r["image"] == "img" and r["harness"] == "claude-code"
    assert r["summary"] == {
        "tasks": 5,
        "finished": 3,
        "pending": 2,
        "passed": 1,
        "failed": 1,
        "unverified": 3,
        "input_tokens": 15,
        "output_tokens": 3,
    }
    assert [t["id"] for t in r["tasks"]] == list("abcde")
    json.dumps(r)  # JSON-ready


def test_labels_round_trip():
    t = ais.Task("hello", "p")
    assert ais.task_of_label("ais", ais.run_label("ais", t)) == "hello"
    assert ais.task_of_label("ais", "other:hello") is None
    assert ais.task_of_label("ais", None) is None


def test_cli_flags_parse():
    p = ais._parser()
    a = p.parse_args(
        ["run", "--tasks", "t.jsonl", "--harness", "openai-codex", "--mcp", "d=https://x/mcp",
         "--key-var", "OPENAI_API_KEY", "--base-url", "http://proxy:8080", "--scripted"]
    )
    assert (a.cmd, a.harness, a.mcp, a.key_var, a.base_url, a.scripted, a.image) == (
        "run", "openai-codex", ["d=https://x/mcp"], ["OPENAI_API_KEY"], "http://proxy:8080", True,
        ais.DEFAULT_IMAGE,
    )
    c = p.parse_args(["collect", "--wait", "30"])
    assert (c.state, c.out, c.wait, c.tasks) == ("runs.json", "report.json", 30.0, None)


# The SDK calls in this example and in tour.py (the docs page's source) must
# exist in the generated Python binding. A static check: no native library
# is needed, so it runs everywhere.
NATIVE = REPO / "libs" / "cua" / "python" / "src" / "cua" / "_native.py"
AGENT_METHODS = {
    "agents", "run", "get", "list", "ensure", "events", "send", "interrupt", "stop",
    "result", "wait", "artifacts", "status", "run_id", "guest", "sh", "delete", "create",
    "connect", "sandboxes",
}


@pytest.mark.parametrize("script", ["agents_in_sandboxes.py", "tour.py"])
def test_sdk_calls_exist_in_the_binding(script):
    if not NATIVE.exists():
        pytest.skip("libs/cua/python is not in this checkout")
    native = NATIVE.read_text()
    tree = ast.parse((EXAMPLE / script).read_text())
    called = {
        n.func.attr
        for n in ast.walk(tree)
        if isinstance(n, ast.Call) and isinstance(n.func, ast.Attribute) and n.func.attr in AGENT_METHODS
    }
    assert called, script
    for name in called:
        assert re.search(rf"\n    (async )?def {name}\(self", native), f"{script}: {name}() not in the binding"
    # Record fields passed by keyword.
    for cls, kw in _record_kwargs(tree):
        m = re.search(rf"\nclass {cls}:.*?def __init__\(self, \*,(.*?)\):", native, re.S)
        assert m, f"{script}: cua.{cls} not in the binding"
        assert re.search(rf"\b{kw}:", m.group(1)), f"{script}: cua.{cls} has no field {kw}"


def _record_kwargs(tree):
    for n in ast.walk(tree):
        if (
            isinstance(n, ast.Call)
            and isinstance(n.func, ast.Attribute)
            and isinstance(n.func.value, ast.Name)
            and n.func.value.id == "cua"
            and n.func.attr[:1].isupper()
        ):
            for k in n.keywords:
                if k.arg:
                    yield n.func.attr, k.arg
