"""The browser-use macOS harness driver's tools, pins and ready checks, offline: a fake for the CLI, and no Mac."""

from __future__ import annotations

import base64
import hashlib
import json
from pathlib import Path

import pytest
from agentenv_protocol.computer.platform import Platform
from conftest import DRIVERS, load

HELP = "macos-harness <tool> [--flag value ...]\n\n" + "".join(
    f"  macos-harness {n} --app X\n"
    for n in "list_apps get_app_state click perform_secondary_action set_value select_text scroll drag press_key type_text paste".split()
)


def tool(harness, name):
    return next(t for t in harness.pinned()["tools"] if t["name"] == name)


def test_it_declares_macos_on_apple_silicon_and_serves_the_eleven_tools(harness):
    card = harness.MacosHarness.declaration(Platform(os="macos", arch="arm64"))
    names = {t.name for t in card.capabilities.tools}
    assert len(harness.pinned()["tools"]) == 11
    assert {t["name"] for t in harness.pinned()["tools"]} <= names and "computer_click" in names


def test_the_pins_are_0_2_1_and_the_skill_it_embeds(harness):
    assert harness.VERSION == "0.2.1" and harness.pinned()["version"] == "0.2.1"
    assert harness.pinned()["skill_sha256"] == harness.SKILL_SHA256
    assert "macos-harness==0.2.1" in (DRIVERS / "macos-harness" / "requirements.txt").read_text()


def test_the_tools_json_is_what_the_installed_harness_says(harness):
    """Drift in either the pin or the package shows here, where the package is installed (it is macOS-only)."""
    tools = pytest.importorskip("macos_harness.tools")
    protocol = pytest.importorskip("macos_harness.protocol")
    assert protocol.VERSION == harness.VERSION
    pinned = {t["name"]: t for t in harness.pinned()["tools"]}
    assert set(pinned) == set(tools.TOOLS)
    for name, ours in pinned.items():
        assert ours["description"] == tools.TOOLS[name].description
        assert set(ours["inputSchema"]["properties"]) == set(tools.TOOLS[name].params)
        for flag, prop in ours["inputSchema"]["properties"].items():
            assert prop["description"] == tools.PARAMS[flag][1]


def test_a_call_becomes_equals_flags(harness):
    argv, stdin = harness.argv_for(
        tool(harness, "click"), {"app": "TextEdit", "element_index": 12, "mouse_button": "right"}
    )
    assert (
        argv == ["click", "--app=TextEdit", "--element_index=12", "--mouse_button=right"]
        and stdin is None
    )


def test_a_boolean_flag_is_bare_and_false_is_absent(harness):
    argv, _ = harness.argv_for(
        tool(harness, "get_app_state"), {"app": "Finder", "keep": True, "disableDiff": False}
    )
    assert argv == ["get_app_state", "--app=Finder", "--keep"]


def test_text_with_a_dash_stays_a_value_and_a_lone_dash_reads_stdin(harness):
    spec = tool(harness, "type_text")
    assert harness.argv_for(spec, {"app": "A", "text": "-x"})[0] == [
        "type_text",
        "--app=A",
        "--text=-x",
    ]
    assert harness.argv_for(spec, {"app": "A", "text": "-"}) == (
        ["type_text", "--app=A", "--text"],
        "-",
    )


def test_a_bad_call_is_named(harness):
    with pytest.raises(ValueError, match="needs app"):
        harness.argv_for(tool(harness, "click"), {"element_index": 1})
    with pytest.raises(ValueError, match="no argument named shell"):
        harness.argv_for(tool(harness, "click"), {"app": "A", "shell": "x"})


@pytest.mark.parametrize(
    "granted, ok",
    [
        (
            {
                "accessibility": True,
                "screen_recording": True,
                "post_events": False,
                "input_monitoring": False,
            },
            True,
        ),
        ({"accessibility": False, "screen_recording": True}, False),
        ({}, False),
    ],
)
def test_the_doctor_check_wants_the_two_required_grants_only(harness, granted, ok):
    check = harness.doctor_check({"app": "Terminal", "granted": granted})
    assert check.ok is ok
    if not ok:
        assert "Terminal" in check.detail


def test_the_tools_check_names_every_tool_that_differs(harness):
    pinned = harness.pinned()["tools"]
    assert harness.tools_check(HELP, pinned).ok
    check = harness.tools_check(HELP + "  macos-harness zoom --app X\n", pinned)
    assert not check.ok and "zoom" in check.detail


def test_a_reads_screenshot_comes_back_as_an_image(harness, tmp_path):
    shot = tmp_path / "Finder Screenshot 2026-11-03 at 10.00.00 abc123.jpeg"
    shot.write_bytes(b"\xff\xd8jpeg")
    text = f'Window: "Downloads", App: Finder.\n\t1 button Back\nScreenshot (1280x800 px): {shot}'
    content = harness.attach_screenshot(text, Path.read_bytes)
    assert [c.type for c in content] == ["text", "image"]
    assert (
        content[1].mimeType == "image/jpeg" and base64.b64decode(content[1].data) == b"\xff\xd8jpeg"
    )
    assert [
        c.type for c in harness.attach_screenshot("Screenshot: none. no window", Path.read_bytes)
    ] == ["text"]
    assert [
        c.type
        for c in harness.attach_screenshot(
            f"Screenshot (1x1 px): {tmp_path}/gone.jpeg", Path.read_bytes
        )
    ] == ["text"]


class FakeCli:
    def __init__(self, answers):
        self.answers, self.calls = answers, []

    async def __call__(self, argv, *, stdin=None, timeout_s=0):
        self.calls.append((argv, stdin))
        return self.answers.get(argv[0], (0, "", ""))


def install(harness, monkeypatch, tmp_path, skill="---\nname: x\ndescription: d\n---\nbody\n"):
    binary = tmp_path / "macos-harness"
    binary.write_text("x")
    monkeypatch.setattr(harness, "EXECUTABLE", binary)
    monkeypatch.setattr(harness, "SKILL_SHA256", hashlib.sha256(skill.encode()).hexdigest())
    granted = {"app": "python", "granted": {"accessibility": True, "screen_recording": True}}
    return skill, granted


async def test_ready_checks_pass_for_the_pinned_version_with_its_grants(
    harness, monkeypatch, tmp_path
):
    skill, report = install(harness, monkeypatch, tmp_path)
    driver = harness.MacosHarness()
    driver.run_cli = FakeCli(
        {
            "--version": (0, "0.2.1\n", ""),
            "skill": (0, skill, ""),
            "--help": (0, HELP, ""),
            "doctor": (0, json.dumps(report), ""),
        }
    )
    checks = await driver.ready_checks()
    assert [c.name for c in checks] == ["macos-harness", "macos-harness tools", "macOS permissions"]
    assert all(c.ok for c in checks)


async def test_a_lost_grant_fails_the_ready_check_after_a_reset(harness, monkeypatch, tmp_path):
    skill, report = install(harness, monkeypatch, tmp_path)
    driver = harness.MacosHarness()
    cli = driver.run_cli = FakeCli(
        {
            "--version": (0, "0.2.1\n", ""),
            "skill": (0, skill, ""),
            "--help": (0, HELP, ""),
            "doctor": (0, json.dumps(report), ""),
        }
    )
    assert all(c.ok for c in await driver.ready_checks())
    cli.answers["doctor"] = (
        0,
        json.dumps(
            {"app": "python", "granted": {"accessibility": False, "screen_recording": True}}
        ),
        "",
    )
    last = (await driver.ready_checks())[-1]
    assert not last.ok and "accessibility" in last.detail


async def test_a_different_version_or_skill_fails_with_both(harness, monkeypatch, tmp_path):
    skill, _ = install(harness, monkeypatch, tmp_path)
    driver = harness.MacosHarness()
    driver.run_cli = FakeCli(
        {"--version": (0, "0.3.0\n", ""), "skill": (0, "other", ""), "--help": (0, HELP, "")}
    )
    first = (await driver.ready_checks())[0]
    assert not first.ok and "0.3.0" in first.detail and harness.VERSION in first.detail


async def test_a_tool_runs_its_command_and_returns_its_text(harness):
    driver = harness.MacosHarness()
    cli = driver.run_cli = FakeCli({"click": (0, "Done. Call `get_app_state`.", "")})
    forwarded = next(t for t in driver.forwarded_tools() if t.tool.name == "click")
    result = await forwarded.call(app="TextEdit", element_index=3)
    assert cli.calls == [(["click", "--app=TextEdit", "--element_index=3"], None)]
    assert not result.isError and result.content[0].text.startswith("Done.")
    failed = await (next(t for t in driver.forwarded_tools() if t.tool.name == "paste")).call(
        app="A", text="x"
    )
    assert not failed.isError
    driver.run_cli = FakeCli({"click": (1, "", "No element has the ID 3")})
    assert (await forwarded.call(app="TextEdit", element_index=3)).isError


async def test_the_skill_is_the_harnesss_own(harness):
    driver = harness.MacosHarness()
    driver.run_cli = FakeCli(
        {
            "skill": (
                0,
                "---\nname: macos-harness\ndescription: Read and operate apps.\n---\n\n# macOS Harness\n",
                "",
            )
        }
    )
    description, body, files = await driver.bundled_skill()
    assert (
        description == "Read and operate apps."
        and body.startswith("# macOS Harness")
        and files == {}
    )


def test_telemetry_is_off_in_the_harnesss_environment(harness):
    assert harness._env()["DO_NOT_TRACK"] == "1"


def test_the_manifest_script_loads_without_the_package_when_not_run():
    assert load(DRIVERS / "macos-harness", "driver.py").MANIFEST.name == "tools.json"
