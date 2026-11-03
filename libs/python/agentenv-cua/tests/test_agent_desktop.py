"""The agent-desktop driver's tools, pins and ready checks, offline: a fake for the CLI, and no Mac."""

from __future__ import annotations

import json

import pytest
from agentenv_protocol.computer.platform import Platform

HELP = """Commands:
  snapshot                   Capture accessibility tree
  find                       Search elements
""" + "".join(
    f"  {c}   x\n"
    for c in (
        "screenshot get is click double-click triple-click right-click type set-value clear focus select toggle check "
        "uncheck expand collapse scroll scroll-to press hover drag launch close-app list-apps list-windows focus-window "
        "wait clipboard-get clipboard-set"
    ).split()
)


def test_it_declares_macos_on_apple_silicon_and_serves_each_tool(agent_desktop):
    card = agent_desktop.AgentDesktopMacOS.declaration(Platform(os="macos", arch="arm64"))
    names = {t.name for t in card.capabilities.tools}
    assert {t.name for t in agent_desktop.TOOLS} <= names
    assert len(agent_desktop.TOOLS) == 32 and "computer_screenshot" in names


def test_the_pinned_release_is_1_0_2_for_apple_silicon(agent_desktop):
    d = agent_desktop
    assert d.VERSION == "1.0.2" and d.ARCHIVE == "agent-desktop-v1.0.2-aarch64-apple-darwin.tar.gz"
    assert d.ARCHIVE_URL.startswith(
        "https://github.com/vercel-labs/agent-desktop/releases/download/v1.0.2/"
    )
    assert len(d.ARCHIVE_SHA256) == len(d.EXECUTABLE_SHA256) == len(d.HELPER_SHA256) == 64


def test_the_tool_manifest_is_the_pinned_tools_json(agent_desktop):
    assert json.loads(agent_desktop.MANIFEST.read_text()) == agent_desktop.manifest()


def test_every_tool_has_a_valid_schema_and_a_unique_name(agent_desktop):
    names = [t.name for t in agent_desktop.TOOLS]
    assert len(set(names)) == len(names)
    for tool in agent_desktop.TOOLS:
        schema = tool.schema()
        assert schema["type"] == "object" and schema["additionalProperties"] is False
        assert set(schema.get("required", [])) <= set(schema["properties"])


def test_a_call_becomes_flags_then_a_separator_then_positionals(agent_desktop):
    by = agent_desktop.BY_NAME
    assert agent_desktop.argv_for(by["click"], {"ref": "@s1:e2", "timeout_ms": 900}) == [
        "click",
        "--timeout-ms=900",
        "--",
        "@s1:e2",
    ]
    # text that starts with a dash is a positional, never a flag
    assert agent_desktop.argv_for(by["type"], {"ref": "@s1:e2", "text": "-rf"})[-2:] == [
        "@s1:e2",
        "-rf",
    ]
    assert agent_desktop.argv_for(
        by["snapshot"], {"app": "Finder", "interactive_only": True, "compact": False}
    ) == [
        "snapshot",
        "--app=Finder",
        "--interactive-only",
    ]
    assert agent_desktop.argv_for(by["find"], {"state": ["enabled", "focused=false"]}) == [
        "find",
        "--state=enabled",
        "--state=focused=false",
    ]
    assert agent_desktop.argv_for(by["wait"], {"ms": 250}) == ["wait", "--", "250"]


def test_physical_input_tools_always_pass_headed(agent_desktop):
    by = agent_desktop.BY_NAME
    assert "--headed" in agent_desktop.argv_for(by["hover"], {})
    assert "--headed" in agent_desktop.argv_for(
        by["drag"], {"from_ref": "@a:e1", "to_ref": "@a:e2"}
    )
    for name in ("double_click", "triple_click"):
        assert "--headed" in agent_desktop.argv_for(by[name], {"ref": "@a:e1"})
    assert "--headed" not in agent_desktop.argv_for(by["click"], {"ref": "@a:e1"})


def test_a_bad_call_is_named(agent_desktop):
    with pytest.raises(ValueError, match="needs ref"):
        agent_desktop.argv_for(agent_desktop.BY_NAME["click"], {})
    with pytest.raises(ValueError, match="no argument named shell"):
        agent_desktop.argv_for(agent_desktop.BY_NAME["click"], {"ref": "@a:e1", "shell": "x"})


def test_the_commands_check_names_a_missing_command(agent_desktop):
    assert agent_desktop.commands_check(HELP).ok
    check = agent_desktop.commands_check(HELP.replace("  drag   x\n", ""))
    assert not check.ok and "drag" in check.detail


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


def test_a_failed_envelope_is_an_error_result(agent_desktop):
    ok = agent_desktop.envelope_result(0, '{"ok": true, "data": {}}', "")
    bad = agent_desktop.envelope_result(1, '{"ok": false, "error": {"code": "STALE_REF"}}', "")
    dead = agent_desktop.envelope_result(127, "", "can't start")
    assert not ok.isError and bad.isError and dead.isError
    assert "STALE_REF" in bad.content[0].text and "can't start" in dead.content[0].text


class FakeCli:
    """Answers each agent-desktop command from a table."""

    def __init__(self, answers):
        self.answers, self.calls = answers, []

    async def __call__(self, argv, *, timeout_s):
        self.calls.append(argv)
        return self.answers.get(argv[0], (0, "{}", ""))


def good_status(**states):
    perms = {"accessibility": "granted", "screen_recording": "granted", **states}
    return json.dumps(
        {"ok": True, "data": {"permissions": {k: {"state": v} for k, v in perms.items()}}}
    )


async def test_ready_checks_pass_for_the_pinned_binary_with_its_grants(
    agent_desktop, monkeypatch, tmp_path
):
    d = agent_desktop
    binary = tmp_path / "agent-desktop"
    binary.write_bytes(b"x")
    monkeypatch.setattr(d, "EXECUTABLE", binary)
    monkeypatch.setattr(d, "EXECUTABLE_SHA256", __import__("hashlib").sha256(b"x").hexdigest())
    driver = d.AgentDesktopMacOS()
    driver.run_cli = FakeCli(
        {
            "version": (0, json.dumps({"data": {"version": "1.0.2"}}), ""),
            "--help": (0, HELP, ""),
            "status": (0, good_status(), ""),
        }
    )
    checks = await driver.ready_checks()
    assert [c.name for c in checks] == [
        "agent-desktop",
        "agent-desktop commands",
        "macOS permissions",
    ]
    assert all(c.ok for c in checks)


async def test_a_lost_grant_fails_the_ready_check_after_a_reset(
    agent_desktop, monkeypatch, tmp_path
):
    d = agent_desktop
    binary = tmp_path / "agent-desktop"
    binary.write_bytes(b"x")
    monkeypatch.setattr(d, "EXECUTABLE", binary)
    monkeypatch.setattr(d, "EXECUTABLE_SHA256", __import__("hashlib").sha256(b"x").hexdigest())
    driver = d.AgentDesktopMacOS()
    cli = driver.run_cli = FakeCli(
        {
            "version": (0, json.dumps({"data": {"version": "1.0.2"}}), ""),
            "--help": (0, HELP, ""),
            "status": (0, good_status(), ""),
        }
    )
    assert all(c.ok for c in await driver.ready_checks())
    cli.answers["status"] = (0, good_status(screen_recording="denied"), "")
    checks = await driver.ready_checks()
    assert not checks[-1].ok and "screen_recording" in checks[-1].detail


async def test_a_wrong_binary_fails_with_both_digests(agent_desktop, monkeypatch, tmp_path):
    binary = tmp_path / "agent-desktop"
    binary.write_bytes(b"other")
    monkeypatch.setattr(agent_desktop, "EXECUTABLE", binary)
    driver = agent_desktop.AgentDesktopMacOS()
    driver.run_cli = FakeCli(
        {"version": (0, json.dumps({"data": {"version": "9.9.9"}}), ""), "--help": (0, HELP, "")}
    )
    first = (await driver.ready_checks())[0]
    assert (
        not first.ok and "9.9.9" in first.detail and agent_desktop.EXECUTABLE_SHA256 in first.detail
    )


async def test_a_missing_binary_says_so(agent_desktop, monkeypatch, tmp_path):
    monkeypatch.setattr(agent_desktop, "EXECUTABLE", tmp_path / "nope")
    driver = agent_desktop.AgentDesktopMacOS()
    driver.run_cli = FakeCli({"status": (127, "", "")})
    first = (await driver.ready_checks())[0]
    assert not first.ok and "isn't installed" in first.detail


async def test_a_tool_runs_its_command_and_returns_the_envelope(agent_desktop):
    driver = agent_desktop.AgentDesktopMacOS()
    cli = driver.run_cli = FakeCli({"click": (0, '{"ok": true, "command": "click"}', "")})
    tool = next(t for t in driver.forwarded_tools() if t.tool.name == "click")
    result = await tool.call(ref="@s1:e2")
    assert cli.calls == [["click", "--", "@s1:e2"]] and not result.isError
    bad = await tool.call()
    assert bad.isError and "needs ref" in bad.content[0].text


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
