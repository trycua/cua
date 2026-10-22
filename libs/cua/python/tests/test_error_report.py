"""Errors carry a stable code and a one-sentence hint; tracebacks are short;
`on_progress` and `enable_output` are the opt-in progress."""

import asyncio
import traceback

import pytest

import cua


def test_every_error_has_a_code_and_a_hint():
    e = cua.CuaError.NotFound("sandbox nope")
    assert e.code == "not_found"
    assert "cua sb ls" in e.hint
    assert e.report.message == "sandbox nope not found"
    # The raw text is untouched.
    assert str(e) == "sandbox nope"


def test_already_exists_is_its_own_error():
    e = cua.CuaError.AlreadyExists("a sandbox named 'x' already exists")
    assert (e.code, e.report.message) == ("already_exists", "a sandbox named 'x' already exists")
    assert "cua sb rm x" in e.hint


def test_layers_and_structs_are_cut_from_the_message():
    raw = "[local] container engine: pull linux: Docker responded with status code 404: pull access denied for linux, repository does not exist"
    e = cua.CuaError.Runtime(raw)
    assert e.code == "image_not_found"
    assert e.report.message == 'no image named "linux"'
    assert "container engine" not in e.report.message
    assert e.report.detail  # the raw text is kept for -v style output


def test_a_plan_limit_keeps_its_own_code():
    e = cua.CuaError.Http("You're using 5 of 5 live sessions on Pro. Close one or upgrade to Teams.")
    assert e.plan_limit.code == "plan_limit"
    assert e.code == "http"


def test_a_failing_call_has_a_short_traceback_and_no_context():
    async def go():
        c = cua.embedded()
        await c.sandboxes().get("no-such-sandbox-xyz")

    with pytest.raises(cua.CuaError) as info:
        asyncio.run(go())
    frames = traceback.extract_tb(info.value.__traceback__)
    assert all("_native.py" not in f.filename for f in frames), frames
    assert info.value.__suppress_context__
    assert info.value.code == "not_found"


def test_on_progress_is_passed_to_the_create_and_its_errors_do_not_leak():
    seen = []

    class Fake:
        async def create_with_progress(self, options, listener):
            listener.on_progress("one")
            listener.on_progress("two")
            return "sandbox"

    async def go():
        def callback(p):
            seen.append(p)
            raise RuntimeError("a bad callback never fails a create")

        return await cua._native.Sandboxes.create(Fake(), "opts", on_progress=callback)

    assert asyncio.run(go()) == "sandbox"
    assert seen == ["one", "two"]


def test_output_is_off_until_asked_for():
    assert callable(cua.enable_output) and callable(cua.disable_output)
    cua.enable_output()
    cua.disable_output()
