"""The agent-desktop driver's derived tools, pins and ready checks, offline: recorded ``--help`` output for the
pinned binary, a fake for the CLI and stand-in executables, and no Mac."""

from __future__ import annotations

import asyncio
import hashlib
import json
from pathlib import Path

import pytest
from agentenv_protocol.computer.platform import Platform

#: ``--help`` of every command of agent-desktop 1.0.2, recorded from the pinned binary, keyed by command words.
HELP_1_0_2 = json.loads(
    (Path(__file__).parent / "fixtures" / "agent-desktop-1.0.2-help.json").read_text()
)
#: The tools derived for 1.0.2, in the order the CLI lists its commands. A change here is a change of the catalog.
TOOLS_1_0_2 = [
    "desktop_snapshot",
    "desktop_find",
    "desktop_screenshot",
    "desktop_get",
    "desktop_is",
    "desktop_click",
    "desktop_double_click",
    "desktop_triple_click",
    "desktop_right_click",
    "desktop_type",
    "desktop_set_value",
    "desktop_clear",
    "desktop_focus",
    "desktop_select",
    "desktop_toggle",
    "desktop_check",
    "desktop_uncheck",
    "desktop_expand",
    "desktop_collapse",
    "desktop_scroll",
    "desktop_scroll_to",
    "desktop_press",
    "desktop_key_down",
    "desktop_key_up",
    "desktop_hover",
    "desktop_drag",
    "desktop_mouse_move",
    "desktop_mouse_click",
    "desktop_mouse_down",
    "desktop_mouse_up",
    "desktop_mouse_wheel",
    "desktop_launch",
    "desktop_close_app",
    "desktop_list_windows",
    "desktop_list_displays",
    "desktop_list_apps",
    "desktop_focus_window",
    "desktop_resize_window",
    "desktop_move_window",
    "desktop_minimize",
    "desktop_maximize",
    "desktop_restore",
    "desktop_list_surfaces",
    "desktop_open_system_surface",
    "desktop_list_notifications",
    "desktop_dismiss_notification",
    "desktop_dismiss_all_notifications",
    "desktop_notification_action",
    "desktop_clipboard_get",
    "desktop_clipboard_set",
    "desktop_clipboard_clear",
    "desktop_wait",
    "desktop_status",
    "desktop_permissions",
    "desktop_version",
    "desktop_batch",
    "desktop_skills_list",
    "desktop_skills_get",
    "desktop_skills_path",
    "desktop_session_start",
    "desktop_session_end",
    "desktop_session_list",
    "desktop_session_gc",
    "desktop_cursor_overlay_enable",
    "desktop_cursor_overlay_disable",
    "desktop_trace_show",
    "desktop_trace_export",
]


def derived(d, help_map=None):
    helps = HELP_1_0_2 if help_map is None else help_map

    async def help_for(words):
        return helps[" ".join(words)]

    return asyncio.run(d.catalog.derive(help_for))


def tool(d, name):
    return next(t for t in d.pinned_document()["tools"] if t["name"] == name)


def test_it_declares_macos_on_apple_silicon_and_serves_each_derived_tool(agent_desktop):
    card = agent_desktop.AgentDesktopMacOS.declaration(Platform(os="macos", arch="arm64"))
    names = [t.name for t in card.capabilities.tools]
    assert set(TOOLS_1_0_2) <= set(names) and "computer_screenshot" in names


def test_the_pinned_release_is_1_0_2_for_apple_silicon(agent_desktop):
    d = agent_desktop
    assert d.VERSION == "1.0.2" and d.ARCHIVE == "agent-desktop-v1.0.2-aarch64-apple-darwin.tar.gz"
    assert d.ARCHIVE_URL.startswith(
        "https://github.com/vercel-labs/agent-desktop/releases/download/v1.0.2/"
    )
    assert len(d.ARCHIVE_SHA256) == len(d.EXECUTABLE_SHA256) == len(d.HELPER_SHA256) == 64


def test_the_derived_catalog_is_the_snapshot_for_1_0_2(agent_desktop):
    tools = derived(agent_desktop)
    assert [t["name"] for t in tools] == TOOLS_1_0_2 and len(tools) == 67
    assert all(t["name"].startswith("desktop_") for t in tools)
    # a command with subcommands is not a tool; each of its leaves is
    assert "desktop_skills" not in TOOLS_1_0_2 and "desktop_skills_get" in TOOLS_1_0_2
    assert "desktop_cursor_overlay_enable" in TOOLS_1_0_2 and "desktop_double_click" in TOOLS_1_0_2


def test_tools_json_pins_exactly_what_the_binary_derives(agent_desktop):
    d = agent_desktop
    document = d.pinned_document()
    assert document["version"] == d.VERSION and document["executable_sha256"] == d.EXECUTABLE_SHA256
    assert document["tools"] == derived(d)
    assert d.catalog_check(document, derived(d)).ok


def test_the_shared_options_are_the_drivers_not_the_models(agent_desktop):
    def props(name):
        return set(tool(agent_desktop, name)["inputSchema"]["properties"])

    everything = set().union(*(props(n) for n in TOOLS_1_0_2))
    for hidden in (
        "verbose",
        "debug",
        "screenshot",
        "session",
        "agent_id",
        "trace",
        "trace_strict",
    ):
        assert hidden not in everything
    # delivery and wait flags show up on a ref action, and not on a command that has no ref
    assert {"headed", "wait_for", "wait_for_gone", "wait_timeout"} <= props("desktop_click")
    assert "headed" in props("desktop_mouse_click") and "wait_for" not in props("desktop_version")
    assert props("desktop_status") == set()


def test_schemas_carry_types_required_enums_defaults_and_help(agent_desktop):
    click = tool(agent_desktop, "desktop_click")["inputSchema"]
    assert click["required"] == ["ref"] and click["additionalProperties"] is False
    assert click["properties"]["timeout_ms"]["type"] == "integer"
    assert click["properties"]["timeout_ms"]["default"] == 5000
    assert "Qualified ref" in click["properties"]["ref"]["description"]
    snapshot = tool(agent_desktop, "desktop_snapshot")["inputSchema"]["properties"]
    assert snapshot["interactive_only"]["type"] == "boolean"
    assert "window" in snapshot["surface"]["enum"] and snapshot["surface"]["default"] == "window"
    assert snapshot["max_depth"] == {
        "type": "integer",
        "description": "Maximum tree depth (default 10)",
        "default": 10,
    }
    find = tool(agent_desktop, "desktop_find")["inputSchema"]["properties"]
    assert find["state"]["type"] == "array" and find["state"]["items"] == {"type": "string"}
    launch = tool(agent_desktop, "desktop_launch")["inputSchema"]
    assert launch["required"] == ["app"]
    assert launch["properties"]["cdp"]["type"] == ["integer", "boolean"]
    assert launch["properties"]["timeout"]["type"] == "integer"
    wait = tool(agent_desktop, "desktop_wait")["inputSchema"]["properties"]
    assert wait["ms"]["type"] == "integer"
    overlay = tool(agent_desktop, "desktop_cursor_overlay_enable")["inputSchema"]["properties"]
    assert overlay["size"]["type"] == "number"


def test_every_schema_is_valid_and_every_name_unique(agent_desktop):
    tools = agent_desktop.pinned_document()["tools"]
    assert len({t["name"] for t in tools}) == len(tools)
    for t in tools:
        schema = t["inputSchema"]
        assert schema["type"] == "object" and schema["additionalProperties"] is False
        assert set(schema.get("required", [])) <= set(schema["properties"])
        assert t["description"]


def test_a_call_becomes_the_command_flags_a_separator_then_positionals(agent_desktop):
    argv = agent_desktop.catalog.build_argv

    def t(name):
        return tool(agent_desktop, name)

    assert argv(t("desktop_click"), {"ref": "@s1:e2", "timeout_ms": 900}) == [
        "click",
        "--timeout-ms=900",
        "--",
        "@s1:e2",
    ]
    # text that starts with a dash is a positional, never a flag
    assert argv(t("desktop_type"), {"ref": "@s1:e2", "text": "-rf"})[-3:] == ["--", "@s1:e2", "-rf"]
    snap = {"app": "Finder", "interactive_only": True, "compact": False}
    assert argv(t("desktop_snapshot"), snap) == ["snapshot", "--app=Finder", "--interactive-only"]
    assert argv(t("desktop_find"), {"state": ["enabled", "focused=false"]}) == [
        "find",
        "--state=enabled",
        "--state=focused=false",
    ]
    assert argv(t("desktop_wait"), {"ms": 250}) == ["wait", "--", "250"]
    # subcommands are command words, a negative number is joined to its flag, a bare optional-value flag stands alone
    assert argv(t("desktop_skills_get"), {"name": "desktop", "reference": "workflows"}) == [
        "skills",
        "get",
        "--",
        "desktop",
        "workflows",
    ]
    assert argv(t("desktop_mouse_wheel"), {"x": -5, "y": 3, "dy": -2}) == [
        "mouse-wheel",
        "--x=-5",
        "--y=3",
        "--dy=-2",
    ]
    assert argv(t("desktop_launch"), {"app": "Notes", "cdp": True, "arg": ["-a", "b"]}) == [
        "launch",
        "--arg=-a",
        "--arg=b",
        "--cdp",
        "--",
        "Notes",
    ]
    assert argv(t("desktop_launch"), {"app": "Notes", "cdp": 9222})[1] == "--cdp=9222"
    assert argv(t("desktop_drag"), {"from": "@a:e1", "to": "@a:e2", "headed": True}) == [
        "drag",
        "--from=@a:e1",
        "--to=@a:e2",
        "--headed",
    ]
    assert argv(t("desktop_status"), {}) == ["status"]


def test_a_bad_call_is_named(agent_desktop):
    argv = agent_desktop.catalog.build_argv
    click = tool(agent_desktop, "desktop_click")
    with pytest.raises(ValueError, match="needs ref"):
        argv(click, {})
    with pytest.raises(ValueError, match="no argument named shell"):
        argv(click, {"ref": "@a:e1", "shell": "x"})
    with pytest.raises(ValueError, match="timeout_ms must be of type integer"):
        argv(click, {"ref": "@a:e1", "timeout_ms": "soon"})
    with pytest.raises(ValueError, match="must be one of"):
        argv(tool(agent_desktop, "desktop_snapshot"), {"surface": "attic"})
    # an optional positional can't be skipped for a later one
    skills_get = tool(agent_desktop, "desktop_skills_get")
    with pytest.raises(ValueError, match="needs name"):
        argv(skills_get, {"reference": "workflows"})


def test_global_flags_go_before_the_separator_and_never_twice(agent_desktop):
    d = agent_desktop
    assert d.with_globals(["click", "--", "@a:e1"], ("--headed",)) == [
        "click",
        "--headed",
        "--",
        "@a:e1",
    ]
    assert d.with_globals(["click", "--headed"], ("--headed",)) == ["click", "--headed"]
    assert d.with_globals(["status"], ()) == ["status"]


def test_a_tool_call_has_its_own_time_limit(agent_desktop):
    d = agent_desktop
    click, batch = tool(d, "desktop_click"), tool(d, "desktop_batch")
    assert d.call_timeout(click, {}) == d.DEFAULT_TIMEOUT_S
    assert d.call_timeout(click, {"timeout_ms": 1000}) == 1 + d.TIMEOUT_MARGIN_S
    assert d.call_timeout(click, {"timeout_ms": 10**9}) == d.MAX_TIMEOUT_S
    assert d.call_timeout(tool(d, "desktop_wait"), {"ms": 90_000}) == 90 + d.TIMEOUT_MARGIN_S
    # a command whose own default is long (the batch's whole-run budget) keeps it
    assert d.call_timeout(batch, {}) > d.DEFAULT_TIMEOUT_S


def test_a_binary_whose_commands_changed_diverges_from_the_pin(agent_desktop):
    d = agent_desktop
    pinned = d.pinned_document()
    flag = "      --timeout-ms <TIMEOUT_MS>"
    assert flag in HELP_1_0_2["click"]
    changed = {
        **HELP_1_0_2,
        "click": HELP_1_0_2["click"].replace(flag, "      --retries <N>   More\n" + flag),
    }
    check = d.catalog_check(pinned, derived(d, changed))
    assert not check.ok and "changed: desktop_click" in check.detail
    assert "--write-manifest" in check.detail
    # a command that is gone, and one that is new
    root = HELP_1_0_2[""]
    renamed = {
        **HELP_1_0_2,
        "": root.replace("  drag    ", "  drag2   "),
        "drag2": HELP_1_0_2["drag"],
    }
    check = d.catalog_check(pinned, derived(d, renamed))
    assert "missing from the binary: desktop_drag" in check.detail
    assert "not in the pinned manifest: desktop_drag2" in check.detail
    assert d.catalog_check(pinned, derived(d)).ok


def test_a_manifest_of_another_version_fails_the_catalog_check(agent_desktop):
    d = agent_desktop
    other = {**d.pinned_document(), "version": "9.9.9"}
    check = d.catalog_check(other, derived(d))
    assert not check.ok and "pins 9.9.9" in check.detail


def test_the_parser_reads_wrapped_help_and_optional_values(agent_desktop):
    parsed = agent_desktop.catalog.parse_help(
        "Do a thing\nover two lines\n\nUsage: x [OPTIONS] <A> [B]...\n\nArguments:\n"
        "  <A>     The first\n  [B]...  The rest\n\nOptions:\n"
        "      --level <N>   How much, from\n                    one to nine [default: 3]\n"
        "      --port [<PORT>]   Maybe a port\n  -q, --quiet   Hush\n  -h, --help   Print help\n"
    )
    assert parsed["summary"] == "Do a thing over two lines"
    assert [a["name"] for a in parsed["arguments"]] == ["A", "B"] and parsed["arguments"][1][
        "variadic"
    ]
    level = parsed["options"][0]
    assert level["help"] == "How much, from one to nine [default: 3]" and level["value"] == "<N>"
    assert parsed["options"][1]["value"] == "[<PORT>]" and parsed["options"][2]["value"] is None


@pytest.mark.parametrize(
    "state, ok",
    [
        ({"accessibility": "granted", "screen_recording": "granted"}, True),
        ({"accessibility": "denied", "screen_recording": "granted"}, False),
        ({"accessibility": "granted"}, False),
    ],
)
def test_the_permissions_check_wants_accessibility_and_screen_recording(agent_desktop, state, ok):
    status = {"data": {"permissions": {k: {"state": v} for k, v in state.items()}}}
    check = agent_desktop.permissions_check(status)
    assert check.ok is ok
    if not ok:
        assert "not granted" in check.detail
    assert not agent_desktop.permissions_check({}).ok


def test_the_envelope_comes_back_verbatim_and_a_failure_adds_the_exit_code_and_stderr(
    agent_desktop,
):
    envelope = '{"ok": true, "data": {"n": 1}}\n'
    ok = agent_desktop.envelope_result(0, envelope, "")
    assert not ok.isError and [c.text for c in ok.content] == [envelope.strip()]
    bad = agent_desktop.envelope_result(1, '{"ok": false, "error": {"code": "STALE_REF"}}', "boom")
    assert bad.isError and "STALE_REF" in bad.content[0].text
    assert bad.content[1].text == "exit code: 1\nstderr: boom"
    # exit zero with ok:false is still an error, and no output at all still says something
    assert agent_desktop.envelope_result(0, '{"ok": false}', "").isError
    silent = agent_desktop.envelope_result(2, "", "", command="click")
    assert silent.isError and json.loads(silent.content[0].text)["error"]["code"] == "NO_OUTPUT"
    # non-JSON output on a non-zero exit is still passed on as it was printed
    text = agent_desktop.envelope_result(3, "plain words", "e")
    assert text.isError and text.content[0].text == "plain words"


class FakeCli:
    """Answers each agent-desktop command from a table, and ``--help`` from the recorded help."""

    def __init__(self, answers, helps=None):
        self.answers, self.calls, self.helps = answers, [], helps or HELP_1_0_2

    async def __call__(self, argv, *, timeout_s):
        self.calls.append(argv)
        if argv[-1] == "--help":
            return 0, self.helps[" ".join(argv[:-1])], ""
        return self.answers.get(argv[0], (0, "{}", ""))


def good_status(**states):
    perms = {"accessibility": "granted", "screen_recording": "granted", **states}
    return json.dumps(
        {"ok": True, "data": {"permissions": {k: {"state": v} for k, v in perms.items()}}}
    )


def pinned_binary(d, monkeypatch, tmp_path, content=b"x"):
    """A stand-in file for the executable that the driver takes as the pinned one, and a cache folder of its own."""
    binary = tmp_path / "agent-desktop"
    binary.write_bytes(content)
    monkeypatch.setattr(d, "EXECUTABLE", binary)
    monkeypatch.setattr(d, "EXECUTABLE_SHA256", hashlib.sha256(content).hexdigest())
    monkeypatch.setattr(d, "CACHE_DIR", tmp_path / "cache")
    pinned = d.pinned_document()
    monkeypatch.setattr(
        d, "pinned_document", lambda: {**pinned, "executable_sha256": d.EXECUTABLE_SHA256}
    )
    return binary


def answers(**extra):
    return {
        "version": (0, json.dumps({"data": {"version": "1.0.2"}}), ""),
        "status": (0, good_status(), ""),
        **extra,
    }


async def test_ready_checks_pass_for_the_pinned_binary_its_tools_and_its_grants(
    agent_desktop, monkeypatch, tmp_path
):
    d = agent_desktop
    pinned_binary(d, monkeypatch, tmp_path)
    driver = d.AgentDesktopMacOS()
    driver.run_cli = FakeCli(answers())
    checks = await driver.ready_checks()
    assert [c.name for c in checks] == ["agent-desktop", "agent-desktop tools", "macOS permissions"]
    assert all(c.ok for c in checks) and "67 tools" in checks[1].detail


async def test_the_catalog_is_derived_once_for_each_binary_and_cached_by_its_sha256(
    agent_desktop, monkeypatch, tmp_path
):
    d = agent_desktop
    pinned_binary(d, monkeypatch, tmp_path)
    first = d.AgentDesktopMacOS()
    first.run_cli = cli = FakeCli(answers())
    assert all(c.ok for c in await first.ready_checks())
    assert sum(argv[-1] == "--help" for argv in cli.calls) == 72
    assert [p.name for p in (tmp_path / "cache").iterdir()] == [
        f"catalog-{d.EXECUTABLE_SHA256}.json"
    ]
    # a new driver process finds the cache and runs no help at all
    second = d.AgentDesktopMacOS()
    second.run_cli = again = FakeCli(answers())
    assert all(c.ok for c in await second.ready_checks())
    assert not any(argv[-1] == "--help" for argv in again.calls)
    # a cache that can't be read is derived again, never trusted
    (tmp_path / "cache" / f"catalog-{d.EXECUTABLE_SHA256}.json").write_text("{not json")
    third = d.AgentDesktopMacOS()
    third.run_cli = redo = FakeCli(answers())
    assert all(c.ok for c in await third.ready_checks())
    assert any(argv[-1] == "--help" for argv in redo.calls)


async def test_a_binary_whose_tools_drifted_fails_the_ready_check(
    agent_desktop, monkeypatch, tmp_path
):
    d = agent_desktop
    pinned_binary(d, monkeypatch, tmp_path)
    helps = {**HELP_1_0_2, "": HELP_1_0_2[""].replace("  drag  ", "  dragx ")}
    helps["dragx"] = HELP_1_0_2["drag"]
    driver = d.AgentDesktopMacOS()
    driver.run_cli = FakeCli(answers(), helps)
    checks = await driver.ready_checks()
    assert checks[0].ok and not checks[1].ok and "desktop_drag" in checks[1].detail


async def test_a_help_that_fails_fails_the_tools_check_and_names_the_command(
    agent_desktop, monkeypatch, tmp_path
):
    d = agent_desktop
    pinned_binary(d, monkeypatch, tmp_path)
    driver = d.AgentDesktopMacOS()
    fake = FakeCli(answers())

    async def run_cli(argv, *, timeout_s):
        if argv == ["click", "--help"]:
            return 1, "", "crashed"
        return await fake(argv, timeout_s=timeout_s)

    driver.run_cli = run_cli
    checks = await driver.ready_checks()
    assert not checks[1].ok and "click --help" in checks[1].detail and "crashed" in checks[1].detail


async def test_a_lost_grant_fails_the_ready_check_after_a_reset(
    agent_desktop, monkeypatch, tmp_path
):
    d = agent_desktop
    pinned_binary(d, monkeypatch, tmp_path)
    driver = d.AgentDesktopMacOS()
    cli = driver.run_cli = FakeCli(answers())
    assert all(c.ok for c in await driver.ready_checks())
    cli.answers["status"] = (0, good_status(screen_recording="denied"), "")
    checks = await driver.ready_checks()
    assert not checks[-1].ok and "screen_recording" in checks[-1].detail


async def test_a_wrong_binary_fails_with_both_digests_and_derives_nothing(
    agent_desktop, monkeypatch, tmp_path
):
    d = agent_desktop
    binary = tmp_path / "agent-desktop"
    binary.write_bytes(b"other")
    monkeypatch.setattr(d, "EXECUTABLE", binary)
    monkeypatch.setattr(d, "CACHE_DIR", tmp_path / "cache")
    driver = d.AgentDesktopMacOS()
    driver.run_cli = cli = FakeCli({"version": (0, json.dumps({"data": {"version": "9.9.9"}}), "")})
    first, second = (await driver.ready_checks())[:2]
    assert not first.ok and "9.9.9" in first.detail and d.EXECUTABLE_SHA256 in first.detail
    assert not second.ok and not any(argv[-1] == "--help" for argv in cli.calls)


async def test_a_missing_binary_says_so(agent_desktop, monkeypatch, tmp_path):
    monkeypatch.setattr(agent_desktop, "EXECUTABLE", tmp_path / "nope")
    driver = agent_desktop.AgentDesktopMacOS()
    driver.run_cli = FakeCli({"status": (127, "", "")})
    first = (await driver.ready_checks())[0]
    assert not first.ok and "isn't installed" in first.detail


async def test_a_tool_runs_its_command_and_returns_the_envelope(agent_desktop, monkeypatch):
    monkeypatch.setattr(agent_desktop, "GLOBAL_FLAGS", ())
    driver = agent_desktop.AgentDesktopMacOS()
    cli = driver.run_cli = FakeCli({"click": (0, '{"ok": true, "command": "click"}', "")})
    tools = {t.tool.name: t for t in driver.forwarded_tools()}
    assert len(tools) == 67 and tools["desktop_click"].tool.inputSchema["required"] == ["ref"]
    result = await tools["desktop_click"].call(ref="@s1:e2")
    assert cli.calls == [["click", "--", "@s1:e2"]] and not result.isError
    assert result.content[0].text == '{"ok": true, "command": "click"}'
    bad = await tools["desktop_click"].call()
    assert bad.isError and json.loads(bad.content[0].text)["error"]["code"] == "INVALID_ARGS"
    assert "needs ref" in bad.content[0].text and len(cli.calls) == 1


async def test_driver_level_global_flags_apply_to_every_call(agent_desktop, monkeypatch):
    monkeypatch.setattr(agent_desktop, "GLOBAL_FLAGS", ("--headed", "--trace-strict"))
    driver = agent_desktop.AgentDesktopMacOS()
    cli = driver.run_cli = FakeCli({})
    tools = {t.tool.name: t for t in driver.forwarded_tools()}
    await tools["desktop_double_click"].call(ref="@s1:e2")
    await tools["desktop_list_apps"].call()
    assert cli.calls == [
        ["double-click", "--headed", "--trace-strict", "--", "@s1:e2"],
        ["list-apps", "--headed", "--trace-strict"],
    ]


def script(tmp_path, body):
    path = tmp_path / "agent-desktop"
    path.write_text("#!/bin/sh\n" + body)
    path.chmod(0o755)
    return path


async def test_a_call_runs_the_binary_as_a_child_process_with_the_mapped_argv(
    agent_desktop, monkeypatch, tmp_path
):
    d = agent_desktop
    monkeypatch.setattr(
        d, "EXECUTABLE", script(tmp_path, 'printf \'{"ok":true,"argv":"%s"}\' "$*"\n')
    )
    monkeypatch.setattr(d, "GLOBAL_FLAGS", ())
    driver = d.AgentDesktopMacOS()
    tools = {t.tool.name: t for t in driver.forwarded_tools()}
    result = await tools["desktop_type"].call(ref="@s1:e2", text="-hi there", timeout_ms=2000)
    assert not result.isError
    assert (
        json.loads(result.content[0].text)["argv"] == "type --timeout-ms=2000 -- @s1:e2 -hi there"
    )


async def test_a_nonzero_exit_returns_the_envelope_the_exit_code_and_stderr(
    agent_desktop, monkeypatch, tmp_path
):
    d = agent_desktop
    monkeypatch.setattr(
        d,
        "EXECUTABLE",
        script(
            tmp_path, 'echo \'{"ok":false,"error":{"code":"STALE_REF"}}\'\necho oops >&2\nexit 7\n'
        ),
    )
    result = await d.AgentDesktopMacOS().run_cli(["click"], timeout_s=5)
    assert result[0] == 7
    tools = {t.tool.name: t for t in d.AgentDesktopMacOS().forwarded_tools()}
    called = await tools["desktop_click"].call(ref="@s1:e2")
    assert called.isError and "STALE_REF" in called.content[0].text
    assert called.content[1].text == "exit code: 7\nstderr: oops"


async def test_a_call_that_outlives_its_limit_is_killed_with_an_error_envelope(
    agent_desktop, monkeypatch, tmp_path
):
    d = agent_desktop
    monkeypatch.setattr(d, "EXECUTABLE", script(tmp_path, "sleep 30\n"))
    code, out, err = await d.AgentDesktopMacOS().run_cli(["wait", "--", "1"], timeout_s=0.3)
    assert code == 124 and json.loads(out)["error"]["code"] == "TOOL_TIMEOUT" and "0 s" in err
    result = d.envelope_result(code, out, err, command="wait")
    assert result.isError and "TOOL_TIMEOUT" in result.content[0].text


async def test_a_binary_that_cannot_start_is_an_error_envelope(
    agent_desktop, monkeypatch, tmp_path
):
    d = agent_desktop
    monkeypatch.setattr(d, "EXECUTABLE", tmp_path / "missing")
    code, out, _ = await d.AgentDesktopMacOS().run_cli(["status"], timeout_s=5)
    assert code == 127 and json.loads(out)["error"]["code"] == "SPAWN_FAILED"


def test_the_skill_front_matter_is_split_from_its_body(agent_desktop):
    text = "---\nname: x\ndescription: >\n  First line\n  second line.\ntags: a\n---\n\n# Body\n"
    assert agent_desktop.split_skill(text) == ("First line second line.", "# Body\n")


async def test_the_skill_is_the_binaries_own_with_its_references(agent_desktop):
    core = "---\nname: agent-desktop\ndescription: Operate apps.\n---\n\n# agent-desktop\n"

    def answer(argv, *, timeout_s):
        assert argv[:2] == ["skills", "get"]
        return (
            0,
            json.dumps(
                {"data": {"content": core if argv[2:] == ["desktop"] else f"ref {argv[2:]}"}}
            ),
            "",
        )

    driver = agent_desktop.AgentDesktopMacOS()

    async def run_cli(argv, *, timeout_s):
        return answer(argv, timeout_s=timeout_s)

    driver.run_cli = run_cli
    description, body, files = await driver.bundled_skill()
    assert description == "Operate apps." and body.startswith("# agent-desktop")
    assert sorted(files) == [
        "commands-interaction.md",
        "commands-observation.md",
        "commands-system.md",
        "platform.md",
        "workflows.md",
    ]
