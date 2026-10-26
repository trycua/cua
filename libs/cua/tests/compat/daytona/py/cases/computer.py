"""Computer use on Cua desktops: the mock Space answers screenshot, pointer,
keyboard, displays, windows and accessibility like a spacesd would; the
`space` cases drive a real desktop.

Derived from daytona 0.225.0 `_sync/computer_use.py`, `common/computer_use.py`
and the toolbox client's `computer_use_api.py` and models."""

import base64

from harness import case, err_obs

PNG = b"\x89PNG\r\n\x1a\n"


@case(
    "computer.start_status_and_processes",
    needs="mock",
    ref="_sync/computer_use.py start/stop/get_status/get_process_status/restart_process/get_process_logs/get_process_errors",
)
async def processes(c):
    sb = await c.main()
    cu = sb.computer_use
    started = await c.w(cu.start())
    names = sorted(started.status)
    assert names == ["novnc", "x11vnc", "xfce4", "xvfb"], names
    assert all(s.running for s in started.status.values()), started.status
    assert (await c.w(cu.get_status())).status == "active"
    st = await c.w(cu.get_process_status("xvfb"))
    assert (st.process_name, st.running) == ("xvfb", True), st
    rs = await c.w(cu.restart_process("novnc"))
    assert rs.process_name == "novnc"
    assert (await c.w(cu.get_process_logs("x11vnc"))).process_name == "x11vnc"
    assert (await c.w(cu.get_process_errors("xfce4"))).errors == ""
    stopped = await c.w(cu.stop())
    assert all(
        s.running for s in stopped.status.values()
    ), "the desktop belongs to the Space and keeps running"
    e = await c.raises(lambda: cu.get_process_status("no-such-process"))
    assert e and e["type"] == "DaytonaProcessNotFoundError" and e["status_code"] == 404, e
    return {"processes": names, "status": "active", "stopKeepsRunning": True, "unknown": err_obs(e)}


@case("computer.mouse", needs="mock", ref="_sync/computer_use.py Mouse")
async def mouse(c):
    sb = await c.main()
    m = sb.computer_use.mouse
    p = await c.w(m.move(120, 80))
    assert (p.x, p.y) == (120, 80), p
    assert (await c.w(m.get_position())).x == 120
    r = await c.w(m.click(200, 150))
    assert (r.x, r.y) == (200, 150)
    await c.w(m.click(210, 160, button="right"))
    await c.w(m.click(220, 170, double=True))
    await c.w(m.click(230, 180, clicks=3, modifiers=["ctrl", "shift"]))
    d = await c.w(m.drag(10, 10, 300, 200))
    assert (d.x, d.y) == (300, 200), d
    assert await c.w(m.scroll(100, 100, "down", 5)) in (True, None)
    await c.w(m.scroll(100, 100, "up", 2))
    await c.w(m.down(5, 6, "left"))
    up = await c.w(m.up(7, 8, "left"))
    assert (up.x, up.y) == (7, 8), up
    e = await c.raises(lambda: m.click(1, 1, button="laser"))
    assert e and e["status_code"] == 400, e
    e2 = await c.raises(lambda: m.scroll(1, 1, "sideways", 1))
    assert e2 and e2["status_code"] == 400, e2
    return {
        "moved": [120, 80],
        "clicked": [200, 150],
        "dragEnd": [300, 200],
        "up": [7, 8],
        "badButton": err_obs(e),
        "badDirection": err_obs(e2),
    }


@case("computer.keyboard", needs="mock", ref="_sync/computer_use.py Keyboard")
async def keyboard(c):
    sb = await c.main()
    k = sb.computer_use.keyboard
    await c.w(k.type("hello world"))
    await c.w(k.type("slow", delay=10))
    await c.w(k.press("Return"))
    await c.w(k.press("a", modifiers=["ctrl"]))
    await c.w(k.hotkey("ctrl+shift+t"))
    await c.w(k.down("shift"))
    await c.w(k.up("shift"))
    e = await c.raises(lambda: k.press("no-such-key"))
    assert e and e["status_code"] == 400, e
    e2 = await c.raises(lambda: k.hotkey(""))
    assert e2 and e2["status_code"] == 400, e2
    return {"typed": True, "badKey": err_obs(e), "emptyHotkey": err_obs(e2)}


@case("computer.screenshots", needs="mock", ref="_sync/computer_use.py Screenshot")
async def screenshots(c):
    sb = await c.main()
    s = sb.computer_use.screenshot
    sdk = c.sdk
    full = await c.w(s.take_full_screen(show_cursor=True))
    assert base64.b64decode(full.screenshot).startswith(PNG), "a PNG"
    assert full.size_bytes == len(base64.b64decode(full.screenshot))
    assert full.cursor_position is not None
    reg = await c.w(s.take_region(sdk.ScreenshotRegion(x=10, y=20, width=300, height=200)))
    assert base64.b64decode(reg.screenshot).startswith(PNG)
    comp = await c.w(s.take_compressed(sdk.ScreenshotOptions(fmt="jpeg", quality=80, scale=0.5)))
    assert comp.screenshot
    creg = await c.w(
        s.take_compressed_region(
            sdk.ScreenshotRegion(x=0, y=0, width=100, height=100), sdk.ScreenshotOptions(fmt="webp")
        )
    )
    assert creg.screenshot
    bad = await c.raises(lambda: s.take_compressed(sdk.ScreenshotOptions(fmt="bmp")))
    assert bad and bad["status_code"] == 400, bad
    return {"png": True, "region": True, "compressed": True, "badFormat": err_obs(bad)}


@case("computer.display_and_windows", needs="mock", ref="_sync/computer_use.py Display")
async def display(c):
    sb = await c.main()
    d = sb.computer_use.display
    info = await c.w(d.get_info())
    assert len(info.displays) >= 1 and info.displays[0].is_active, info
    assert (info.displays[0].width, info.displays[0].height) == (1280, 800), info.displays[0]
    ws = await c.w(d.get_windows())
    assert isinstance(ws.windows, list), ws
    return {"display": [1280, 800], "active": True, "windows": isinstance(ws.windows, list)}


@case("computer.accessibility", needs="mock", ref="_sync/computer_use.py Accessibility")
async def accessibility(c):
    sb = await c.main()
    a = sb.computer_use.accessibility
    tree = await c.w(a.get_tree())
    assert tree.root is not None, tree
    found = await c.w(a.find_nodes(role="button"))
    assert found.matches is not None
    if found.matches:
        nid = found.matches[0].id
        await c.w(a.focus_node(nid))
        await c.w(a.invoke_node(nid))
        await c.w(a.invoke_node(nid, action="press"))
    gone = await c.raises(lambda: a.focus_node("no-such:node"))
    assert gone and gone["status_code"] in (404, 503), gone
    bad_re = await c.raises(lambda: a.find_nodes(name="(", name_match="regex"))
    assert bad_re and bad_re["status_code"] == 400, bad_re
    return {"treeRoot": True, "unknownNode": gone["status_code"], "badRegex": err_obs(bad_re)}


@case(
    "computer.recording", needs="space", ref="_sync/computer_use.py RecordingService", timeout=240.0
)
async def recording(c):
    import os
    import tempfile
    import time

    sb = await c.main()
    rec = sb.computer_use.recording
    e0 = await c.raises(lambda: rec.start("probe"))
    if e0 and e0["code"] == "RECORDING_FFMPEG_NOT_FOUND":
        assert e0["status_code"] == 503 and e0["type"] == "DaytonaRecordingFfmpegNotFoundError", e0
        return {"skipped": "no ffmpeg on the gateway host"}
    if e0:
        raise AssertionError(e0)
    first = [r for r in (await c.w(rec.list())).recordings if r.status == "recording"][0]
    time.sleep(2.5) if not c.is_async else await __import__("asyncio").sleep(2.5)
    busy = await c.raises(lambda: rec.delete(first.id))
    assert busy and busy["code"] == "RECORDING_STILL_ACTIVE" and busy["status_code"] == 409, busy
    done = await c.w(rec.stop(first.id))
    assert done.status == "completed" and done.size_bytes and done.duration_seconds >= 2, done
    got = await c.w(rec.get(first.id))
    assert got.id == first.id and got.file_name.endswith(".mp4"), got
    with tempfile.TemporaryDirectory() as d:
        dest = os.path.join(d, "out.mp4")
        await c.w(rec.download(first.id, dest))
        head = open(dest, "rb").read(12)
        assert head[4:8] == b"ftyp", head
    await c.w(rec.delete(first.id))
    gone = await c.raises(lambda: rec.get(first.id))
    assert gone and gone["status_code"] == 404, gone
    return {"mp4": True, "stillActive": err_obs(busy), "afterDelete": err_obs(gone)}


@case(
    "computer.real_desktop",
    needs="space",
    ref="_sync/computer_use.py (a real Cua desktop)",
    timeout=240.0,
)
async def real_desktop(c):
    sb = await c.main()
    cu = sb.computer_use
    await c.w(cu.start())
    info = (await c.w(cu.display.get_info())).displays[0]
    shot = await c.w(cu.screenshot.take_full_screen())
    raw = base64.b64decode(shot.screenshot)
    assert raw.startswith(PNG)
    w, h = int.from_bytes(raw[16:20], "big"), int.from_bytes(raw[20:24], "big")
    assert (w, h) == (info.width, info.height), ((w, h), (info.width, info.height))
    await c.w(cu.mouse.move(info.width // 2, info.height // 2))
    pos = await c.w(cu.mouse.get_position())
    assert abs(pos.x - info.width // 2) <= 2 and abs(pos.y - info.height // 2) <= 2, pos
    jpeg = await c.w(cu.screenshot.take_compressed(c.sdk.ScreenshotOptions(fmt="jpeg", quality=60)))
    assert base64.b64decode(jpeg.screenshot)[:2] == b"\xff\xd8"
    return {"screenshotMatchesDisplay": True, "pointerMoved": True, "jpeg": True}
