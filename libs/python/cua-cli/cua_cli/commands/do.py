"""cua do — one-shot VM automation commands for agents.

Output is always a single line starting with ✅ (success) or ❌ (failure),
followed by a context line:  💻 vm-name\t🪟 Window Title  (or Desktop).

Target VM is persisted in ~/.cua/do_target.json.
Zoom state (bbox + display scale) is also persisted there.
"""

from __future__ import annotations

import argparse
import base64
import io
import json
import os
import sys
import tempfile
from datetime import datetime
from pathlib import Path
from typing import Any

# Ensure stdout/stderr can handle Unicode emojis on Windows
if sys.platform == "win32":
    import io as _io

    if hasattr(sys.stdout, "buffer"):
        sys.stdout = _io.TextIOWrapper(sys.stdout.buffer, encoding="utf-8", errors="replace")
    if hasattr(sys.stderr, "buffer"):
        sys.stderr = _io.TextIOWrapper(sys.stderr.buffer, encoding="utf-8", errors="replace")

MAX_LENGTH = 1200  # max pixel dimension for screenshot output

# ── state ─────────────────────────────────────────────────────────────────────

_STATE_FILE = Path.home() / ".cua" / "do_target.json"
_HOST_CONSENT_FILE = Path.home() / ".cua" / "host_consented"

# Window titles that are internal OS/browser helper windows and should be hidden
_SKIP_WINDOW_TITLES = {
    "Chrome Legacy Window",
}

PROVIDERS = ("cloud", "cloudv2", "lume", "lumier", "docker", "winsandbox", "host")
_REMOTE_PROVIDERS = ("cloud", "cloudv2", "lume", "lumier", "docker", "winsandbox")


def _load_state() -> dict:
    if _STATE_FILE.exists():
        try:
            return json.loads(_STATE_FILE.read_text())
        except Exception:
            pass
    return {}


def _save_state(state: dict) -> None:
    _STATE_FILE.parent.mkdir(parents=True, exist_ok=True)
    _STATE_FILE.write_text(json.dumps(state, indent=2))


def _update_state(**kwargs) -> dict:
    state = _load_state()
    state.update(kwargs)
    _save_state(state)
    return state


def _host_consented() -> bool:
    return _HOST_CONSENT_FILE.exists()


# ── output helpers ────────────────────────────────────────────────────────────


def _ok(msg: str) -> int:
    print(f"✅ {msg}")
    return 0


def _fail(msg: str) -> int:
    print(f"❌ {msg}", file=sys.stderr)
    return 1


# ── provider / connection helpers ─────────────────────────────────────────────


async def _get_api_url(provider_type: str, name: str) -> str:
    """Resolve the computer-server API URL for the target VM."""
    from computer.providers.base import VMProviderType
    from computer.providers.factory import VMProviderFactory

    if provider_type == "host":
        return "http://localhost:8000"

    if provider_type in ("cloud", "cloudv2"):
        from cua_cli.auth.store import get_api_key

        api_key = get_api_key()
        if not api_key:
            raise ValueError("Not authenticated. Run 'cua auth login' first")
        ptype = VMProviderType.CLOUDV2 if provider_type == "cloudv2" else VMProviderType.CLOUD
        provider = VMProviderFactory.create_provider(ptype, api_key=api_key)
        async with provider:
            vm = await provider.get_vm(name)
            if not vm:
                raise ValueError(f"VM not found: {name}")
            url = vm.get("api_url") or vm.get("server_url")
            if not url:
                raise ValueError(f"VM '{name}' has no API URL (is it running?)")
            return url

    if provider_type == "docker":
        provider = VMProviderFactory.create_provider(VMProviderType.DOCKER)
        async with provider:
            vm = await provider.get_vm(name)
            if vm.get("status") == "not_found":
                raise ValueError(f"Docker container not found: {name}")
            ports = vm.get("ports", {})
            for key in ("8000/tcp", "5000/tcp"):
                if key in ports:
                    return f"http://localhost:{ports[key]}"
            return "http://localhost:8000"

    if provider_type in ("lume", "lumier"):
        ptype = VMProviderType.LUME if provider_type == "lume" else VMProviderType.LUMIER
        provider = VMProviderFactory.create_provider(ptype)
        async with provider:
            vm = await provider.get_vm(name)
            ip = vm.get("ip_address") or "localhost"
            return f"http://{ip}:8000"

    if provider_type == "winsandbox":
        return "http://localhost:8000"

    raise ValueError(f"Unknown provider: {provider_type}")


async def _host_dispatch(command: str, params: dict) -> dict:
    """Dispatch a computer-server command to the local host via cua_auto."""
    try:
        import cua_auto.keyboard as _kb
        import cua_auto.mouse as _mouse
        import cua_auto.screen as _screen
        import cua_auto.shell as _shell
        import cua_auto.window as _win
    except ImportError as e:
        return {
            "success": False,
            "error": f"cua-auto not installed: {e}. Run: pip install cua-auto",
        }

    try:
        if command == "screenshot":
            b64 = _screen.screenshot_b64()
            return {"success": True, "image_data": b64}

        elif command == "get_screen_size":
            w, h = _screen.screen_size()
            return {"success": True, "size": {"width": w, "height": h}}

        elif command == "get_cursor_position":
            x, y = _screen.cursor_position()
            return {"success": True, "position": {"x": x, "y": y}}

        elif command == "get_current_window_id":
            handle = _win.get_active_window_handle()
            if not handle:
                return {"success": False, "error": "No active window"}
            return {"success": True, "window_id": handle}

        elif command == "get_window_name":
            title = _win.get_window_name(params.get("window_id", ""))
            if title is None:
                return {"success": False, "error": "Window not found"}
            return {"success": True, "name": title}

        elif command == "get_application_windows":
            handles = _win.get_windows_with_title(params.get("app", ""))
            return {"success": True, "windows": handles}

        elif command == "get_window_size":
            result = _win.get_window_size(params.get("window_id", ""))
            if result is None:
                return {"success": False, "error": "Window not found"}
            return {"success": True, "size": [result[0], result[1]]}

        elif command == "get_window_position":
            result = _win.get_window_position(params.get("window_id", ""))
            if result is None:
                return {"success": False, "error": "Window not found"}
            return {"success": True, "position": [result[0], result[1]]}

        elif command == "left_click":
            _mouse.click(int(params["x"]), int(params["y"]))
            return {"success": True}

        elif command == "right_click":
            _mouse.right_click(int(params["x"]), int(params["y"]))
            return {"success": True}

        elif command == "middle_click":
            _mouse.click(int(params["x"]), int(params["y"]), "middle")
            return {"success": True}

        elif command == "double_click":
            _mouse.double_click(int(params["x"]), int(params["y"]))
            return {"success": True}

        elif command == "move_cursor":
            _mouse.move_to(int(params["x"]), int(params["y"]))
            return {"success": True}

        elif command == "mouse_down":
            x = params.get("x")
            y = params.get("y")
            _mouse.mouse_down(
                int(x) if x is not None else None,
                int(y) if y is not None else None,
                params.get("button", "left"),
            )
            return {"success": True}

        elif command == "mouse_up":
            x = params.get("x")
            y = params.get("y")
            _mouse.mouse_up(
                int(x) if x is not None else None,
                int(y) if y is not None else None,
                params.get("button", "left"),
            )
            return {"success": True}

        elif command == "scroll_direction":
            direction = params.get("direction", "down")
            clicks = int(params.get("clicks", 3))
            if direction == "up":
                _mouse.scroll_up(clicks)
            elif direction == "down":
                _mouse.scroll_down(clicks)
            elif direction == "left":
                _mouse.scroll_left(clicks)
            elif direction == "right":
                _mouse.scroll_right(clicks)
            return {"success": True}

        elif command == "drag_to":
            _mouse.drag(
                int(params["start_x"]),
                int(params["start_y"]),
                int(params["end_x"]),
                int(params["end_y"]),
            )
            return {"success": True}

        elif command == "type_text":
            _kb.type_text(params.get("text", ""))
            return {"success": True}

        elif command == "press_key":
            _kb.press_key(params.get("key", ""))
            return {"success": True}

        elif command == "key_down":
            _kb.key_down(params.get("key", ""))
            return {"success": True}

        elif command == "key_up":
            _kb.key_up(params.get("key", ""))
            return {"success": True}

        elif command == "hotkey":
            keys = params.get("keys", [])
            if isinstance(keys, str):
                keys = [k.strip() for k in keys.replace("-", "+").split("+") if k.strip()]
            _kb.hotkey(keys)
            return {"success": True}

        elif command == "run_command":
            result = _shell.run(params.get("command", ""))
            return {
                "success": True,
                "stdout": result.stdout,
                "stderr": result.stderr,
                "returncode": result.returncode,
            }

        elif command == "open":
            _win.open(params.get("path") or params.get("target", ""))
            return {"success": True}

        elif command == "launch":
            pid = _win.launch(params.get("app", ""), params.get("args"))
            return {"success": True, "pid": pid}

        elif command == "activate_window":
            ok = _win.activate_window(params.get("window_id", ""))
            return {"success": bool(ok)}

        elif command == "minimize_window":
            ok = _win.minimize_window(params.get("window_id", ""))
            return {"success": bool(ok)}

        elif command == "maximize_window":
            ok = _win.maximize_window(params.get("window_id", ""))
            return {"success": bool(ok)}

        elif command == "close_window":
            ok = _win.close_window(params.get("window_id", ""))
            return {"success": bool(ok)}

        elif command == "set_window_size":
            ok = _win.set_window_size(
                params["window_id"], int(params["width"]), int(params["height"])
            )
            return {"success": bool(ok)}

        elif command == "set_window_position":
            ok = _win.set_window_position(params["window_id"], int(params["x"]), int(params["y"]))
            return {"success": bool(ok)}

        elif command == "deactivate_window":
            return {"success": False, "error": "deactivate_window not supported on host"}

        else:
            return {"success": False, "error": f"Unknown host command: {command}"}

    except Exception as e:
        return {"success": False, "error": str(e)}


async def _send(provider_type: str, name: str, command: str, params: dict) -> dict:
    """Send a command to the computer-server and return the parsed response."""
    if provider_type == "host":
        return await _host_dispatch(command, params)

    import aiohttp

    api_url = await _get_api_url(provider_type, name)
    headers = {"Content-Type": "application/json"}

    if provider_type in ("cloud", "cloudv2"):
        from cua_cli.auth.store import get_api_key

        api_key = get_api_key()
        if api_key:
            headers["X-API-Key"] = api_key
        if name:
            headers["X-Container-Name"] = name

    async with aiohttp.ClientSession() as session:
        async with session.post(
            f"{api_url}/cmd",
            json={"command": command, "params": params},
            headers=headers,
            timeout=aiohttp.ClientTimeout(total=30),
        ) as resp:
            text = await resp.text()
            for line in text.splitlines():
                if line.startswith("data: "):
                    return json.loads(line[6:])
            try:
                return json.loads(text)
            except Exception:
                return {"success": False, "error": f"Unexpected response: {text[:200]}"}


# ── zoom / screenshot helpers ─────────────────────────────────────────────────


async def _resolve_zoom_bbox_by_id(provider_type: str, name: str, window_id: str) -> dict | None:
    """Get the bounding box of a window by its native handle/id."""
    pos_r = await _send(provider_type, name, "get_window_position", {"window_id": window_id})
    size_r = await _send(provider_type, name, "get_window_size", {"window_id": window_id})

    pos = pos_r.get("position") or pos_r.get("data")
    size = size_r.get("size") or size_r.get("data")
    if not pos or not size:
        return None

    if isinstance(pos, (list, tuple)):
        x, y = int(pos[0]), int(pos[1])
    else:
        x, y = int(pos.get("x", 0)), int(pos.get("y", 0))

    if isinstance(size, (list, tuple)):
        w, h = int(size[0]), int(size[1])
    else:
        w, h = int(size.get("width", 0)), int(size.get("height", 0))

    return {"x": x, "y": y, "width": w, "height": h}


async def _resolve_zoom_bbox(provider_type: str, name: str, window_name: str) -> dict | None:
    """Get the bounding box of a window by app/window name.

    Filters out internal helper windows (e.g. 'Chrome Legacy Window') so the
    correct top-level window is always selected.
    """
    wins_r = await _send(provider_type, name, "get_application_windows", {"app": window_name})
    windows = wins_r.get("windows") or wins_r.get("data") or []
    if not windows:
        return None

    # Pick the first window whose title is not an internal helper
    window_id = None
    for wid in windows:
        try:
            name_r = await _send(provider_type, name, "get_window_name", {"window_id": wid})
            title = name_r.get("name") or ""
            if title not in _SKIP_WINDOW_TITLES:
                window_id = wid
                break
        except Exception:
            pass
    if window_id is None:
        window_id = windows[0]

    return await _resolve_zoom_bbox_by_id(provider_type, name, window_id)


async def _take_screenshot_data(
    provider_type: str, name: str, state: dict
) -> tuple[bytes, float, dict | None]:
    """Take screenshot, apply zoom crop and max-length scaling.

    Returns (image_bytes, display_scale, zoom_bbox).
    display_scale < 1.0 means the image was shrunk; bbox is None if not zoomed.
    """
    from PIL import Image

    result = await _send(provider_type, name, "screenshot", {})
    img_b64 = result.get("image_data") or result.get("data")
    if not img_b64:
        raise RuntimeError(result.get("error", "no image data returned"))
    img = Image.open(io.BytesIO(base64.b64decode(img_b64)))

    zoom_window = state.get("zoom_window")
    zoom_window_id = state.get("zoom_window_id")
    bbox: dict | None = None

    if zoom_window:
        try:
            if zoom_window_id:
                bbox = await _resolve_zoom_bbox_by_id(provider_type, name, str(zoom_window_id))
            if not bbox:
                bbox = await _resolve_zoom_bbox(provider_type, name, zoom_window)
            if bbox:
                img = img.crop(
                    (
                        bbox["x"],
                        bbox["y"],
                        bbox["x"] + bbox["width"],
                        bbox["y"] + bbox["height"],
                    )
                )
        except Exception:
            bbox = None  # fall back to full screen

    w, h = img.size
    scale = 1.0
    if max(w, h) > MAX_LENGTH:
        scale = MAX_LENGTH / max(w, h)
        img = img.resize((int(w * scale), int(h * scale)), Image.LANCZOS)

    buf = io.BytesIO()
    img.save(buf, format="PNG")
    return buf.getvalue(), scale, bbox


def _coords(x: int, y: int, state: dict) -> tuple[int, int]:
    """Translate image-space (x, y) to screen-space coordinates.

    Accounts for:
    - display_scale: image was scaled for output (divide to get original pixels)
    - zoom_bbox: image was cropped, add offset to get screen coords
    """
    zoom_scale: float = state.get("zoom_scale", 1.0) or 1.0
    zoom_bbox: dict | None = state.get("zoom_bbox")

    sx = round(x / zoom_scale)
    sy = round(y / zoom_scale)

    if zoom_bbox:
        sx += zoom_bbox["x"]
        sy += zoom_bbox["y"]

    return sx, sy


async def _maybe_focus_zoom(provider_type: str, name: str, state: dict) -> None:
    """If a zoom window is tracked, bring it to focus before performing an action."""
    window_id = state.get("zoom_window_id")
    if window_id:
        try:
            await _send(provider_type, name, "activate_window", {"window_id": window_id})
        except Exception:
            pass


async def _print_context(provider_type: str, name: str, state: dict | None = None) -> None:
    """Print the VM + zoom context line after a command."""
    vm_label = name if name else provider_type
    if state is None:
        state = _load_state()
    zoom_window = state.get("zoom_window")
    zoom_window_id = state.get("zoom_window_id")
    if zoom_window:
        zoom_info = (
            f"zoom: {zoom_window} ({zoom_window_id})" if zoom_window_id else f"zoom: {zoom_window}"
        )
    else:
        zoom_info = "zoom: off"
    print(f"💻 {vm_label}\t🔍 {zoom_info}")


async def _take_screenshot_for_recording(
    provider_type: str, name: str, state: dict
) -> bytes | None:
    """Silently take a screenshot for trajectory recording. Returns None on failure."""
    try:
        img_bytes, _, _ = await _take_screenshot_data(provider_type, name, state)
        return img_bytes
    except Exception:
        return None


def _maybe_record_turn(
    args: argparse.Namespace,
    state: dict,
    action_type: str,
    action_params: dict,
    screenshot_bytes: bytes | None = None,
) -> None:
    """Record a trajectory turn if recording is enabled. Never raises."""
    if getattr(args, "no_record", False):
        return
    try:
        from cua_cli.utils.trajectory_recorder import ensure_session, record_turn

        session_dir = ensure_session(state)
        record_turn(session_dir, action_type, action_params, screenshot_bytes)
    except Exception:
        pass  # Never interfere with the primary command


def _require_target() -> dict | None:
    state = _load_state()
    if not state.get("provider"):
        _fail("No VM selected. Run: cua do switch <provider> [name]")
        return None
    return state


# ── subcommand handlers ───────────────────────────────────────────────────────


def _cmd_switch(args: argparse.Namespace) -> int:
    provider = args.provider.lower()
    old_state = _load_state()
    had_zoom = bool(old_state.get("zoom_window"))

    if provider == "host":
        if not _host_consented():
            print(
                "❌ Warning: you are about to allow an AI to control your host PC directly.\n"
                "   This grants full keyboard, mouse, and screen access to your local desktop.\n"
                "   To continue, please run: cua do-host-consent",
                file=sys.stderr,
            )
            return 1
        _save_state(
            {
                "provider": "host",
                "name": "",
                "zoom_window": None,
                "zoom_window_id": None,
                "zoom_bbox": None,
                "zoom_scale": 1.0,
            }
        )
        try:
            from cua_cli.utils.trajectory_recorder import reset_session

            reset_session(_load_state())
        except Exception:
            pass
        msg = "Switched to host (local PC)"
        if had_zoom:
            msg += " — zoom reset"
        return _ok(msg)

    if provider not in PROVIDERS:
        return _fail(f"Unknown provider '{provider}'. Choose from: {', '.join(PROVIDERS)}")

    name = args.name or ""
    _save_state(
        {
            "provider": provider,
            "name": name,
            "zoom_window": None,
            "zoom_window_id": None,
            "zoom_bbox": None,
            "zoom_scale": 1.0,
        }
    )
    try:
        from cua_cli.utils.trajectory_recorder import reset_session

        reset_session(_load_state())
    except Exception:
        pass
    label = f"{provider}/{name}" if name else provider
    msg = f"Switched to {label}"
    if had_zoom:
        msg += " — zoom reset"
    return _ok(msg)


def _cmd_status(args: argparse.Namespace) -> int:
    state = _load_state()
    provider = state.get("provider")
    if not provider:
        return _fail("No VM selected. Run: cua do switch <provider> [name]")
    name = state.get("name", "")
    label = f"{provider}/{name}" if name else provider
    zoom = state.get("zoom_window")
    zoom_info = f" [zoomed to '{zoom}']" if zoom else ""
    return _ok(f"Current target: {label}{zoom_info}")


def _cmd_ls(args: argparse.Namespace) -> int:
    from cua_cli.utils.async_utils import run_async

    explicit_provider = getattr(args, "provider", None)

    async def _list_one(ptype: str) -> int:
        from computer.providers.base import VMProviderType
        from computer.providers.factory import VMProviderFactory

        if ptype == "host":
            print("  host  [local]")
            return 0

        kwargs: dict[str, Any] = {}
        if ptype in ("cloud", "cloudv2"):
            from cua_cli.auth.store import get_api_key

            api_key = get_api_key()
            if not api_key:
                return _fail("Not authenticated. Run 'cua auth login' first")
            kwargs["api_key"] = api_key

        try:
            enum_type = VMProviderType(ptype)
        except ValueError:
            return _fail(f"Unknown provider: {ptype}")

        try:
            provider = VMProviderFactory.create_provider(enum_type, **kwargs)
            async with provider:
                vms = await provider.list_vms()
        except Exception as e:
            return _fail(str(e))

        if not vms:
            print(f"No VMs found for provider '{ptype}'")
            return 0

        for vm in vms:
            print(f"  {vm.get('name', '?')}  [{vm.get('status', '?')}]")
        return 0

    async def _list_all() -> int:
        from computer.providers.base import VMProviderType
        from computer.providers.factory import VMProviderFactory
        from cua_cli.auth.store import get_api_key

        print("  host  [local]")

        api_key = get_api_key()
        if api_key:
            try:
                provider = VMProviderFactory.create_provider(
                    VMProviderType.CLOUDV2, api_key=api_key
                )
                async with provider:
                    vms = await provider.list_vms()
                for vm in vms:
                    print(f"  {vm.get('name', '?')}  [{vm.get('status', '?')}]  cloudv2")
            except Exception:
                pass

        return 0

    if explicit_provider:
        return run_async(_list_one(explicit_provider.lower()))
    else:
        return run_async(_list_all())


def _cmd_zoom(args: argparse.Namespace) -> int:
    from cua_cli.utils.async_utils import run_async

    t = _require_target()
    if not t:
        return 1
    window_name = args.window_name

    async def _run() -> int:
        state = _load_state()
        p, n = state["provider"], state.get("name", "")
        try:
            wins_r = await _send(p, n, "get_application_windows", {"app": window_name})
            windows = wins_r.get("windows") or wins_r.get("data") or []
            if not windows:
                return _fail(f"No windows found matching '{window_name}'")
            # Fetch titles and filter out internal helper windows
            candidates = []
            for wid in windows:
                name_r = await _send(p, n, "get_window_name", {"window_id": wid})
                title = name_r.get("name") or str(wid)
                if title not in _SKIP_WINDOW_TITLES:
                    candidates.append((wid, title))
            if not candidates:
                return _fail(f"No windows found matching '{window_name}'")
            if len(candidates) > 1:
                labels = [f"{wid} ({title})" for wid, title in candidates]
                return _fail(
                    f"Multiple windows matched '{window_name}' — be more specific. "
                    f"Found: {', '.join(labels)}"
                )
            wid, matched_title = candidates[0]
        except Exception:
            wid, matched_title = None, window_name  # resolution failed, still set zoom

        _update_state(zoom_window=window_name, zoom_window_id=wid, zoom_bbox=None, zoom_scale=1.0)
        id_suffix = f" ({wid})" if wid else ""
        return _ok(f"Zoomed to '{matched_title}'{id_suffix}")

    return run_async(_run())


def _cmd_unzoom(args: argparse.Namespace) -> int:
    _update_state(zoom_window=None, zoom_window_id=None, zoom_bbox=None, zoom_scale=1.0)
    return _ok("Unzoomed — full screen")


def _cmd_screenshot(args: argparse.Namespace) -> int:
    from cua_cli.utils.async_utils import run_async

    t = _require_target()
    if not t:
        return 1

    async def _run() -> int:
        state = _load_state()
        try:
            img_bytes, scale, bbox = await _take_screenshot_data(
                state["provider"], state.get("name", ""), state
            )
        except Exception as e:
            await _print_context(state["provider"], state.get("name", ""), state)
            return _fail(str(e))

        # Persist zoom state for coordinate translation
        _update_state(zoom_scale=scale, zoom_bbox=bbox)

        save_path = getattr(args, "save", None)
        if not save_path:
            ts = datetime.now().strftime("%Y%m%d_%H%M%S")
            save_path = f"{tempfile.gettempdir()}/cua_screenshot_{ts}.png"

        with open(save_path, "wb") as f:
            f.write(img_bytes)

        _maybe_record_turn(args, state, "screenshot", {}, img_bytes)

        rc = _ok(f"screenshot saved to {save_path}")
        await _print_context(state["provider"], state.get("name", ""), state)
        return rc

    return run_async(_run())


def _cmd_snapshot(args: argparse.Namespace) -> int:
    from cua_cli.utils.async_utils import run_async

    t = _require_target()
    if not t:
        return 1

    extra = " ".join(args.instructions) if args.instructions else ""

    async def _run() -> int:
        state = _load_state()
        try:
            img_bytes, scale, bbox = await _take_screenshot_data(
                state["provider"], state.get("name", ""), state
            )
        except Exception as e:
            await _print_context(state["provider"], state.get("name", ""), state)
            return _fail(str(e))

        _update_state(zoom_scale=scale, zoom_bbox=bbox)

        ts = datetime.now().strftime("%Y%m%d_%H%M%S")
        save_path = f"{tempfile.gettempdir()}/cua_snapshot_{ts}.png"
        with open(save_path, "wb") as f:
            f.write(img_bytes)

        api_key = os.environ.get("ANTHROPIC_API_KEY")
        if not api_key:
            await _print_context(state["provider"], state.get("name", ""), state)
            return _fail("ANTHROPIC_API_KEY not set")

        try:
            import anthropic as _anthropic

            client = _anthropic.Anthropic(api_key=api_key)
            prompt = (
                "You are analyzing a screenshot for an AI agent.\n"
                "1. Write a 1-2 sentence summary of what is currently on screen.\n"
                "2. List every interactive element visible (buttons, links, inputs, "
                "menus, checkboxes, dropdowns, etc.) with its center coordinates "
                "in image pixels (origin = top-left). Be precise.\n\n"
                "Respond in this exact JSON format:\n"
                '{"summary": "...", "elements": [{"name": "...", "type": "...", "x": N, "y": N}, ...]}\n'
            )
            if extra:
                prompt += f"\nAdditional instructions: {extra}"

            img_b64 = base64.b64encode(img_bytes).decode()
            response = client.messages.create(
                model="claude-haiku-4-5-20251001",
                max_tokens=1024,
                messages=[
                    {
                        "role": "user",
                        "content": [
                            {
                                "type": "image",
                                "source": {
                                    "type": "base64",
                                    "media_type": "image/png",
                                    "data": img_b64,
                                },
                            },
                            {"type": "text", "text": prompt},
                        ],
                    }
                ],
            )

            raw = response.content[0].text.strip()
            # Try to parse JSON; fall back to raw text
            try:
                parsed = json.loads(raw)
                summary = parsed.get("summary", "")
                elements = parsed.get("elements", [])
                print(f"✅ snapshot — {save_path}")
                print()
                print(summary)
                if elements:
                    print()
                    print("Interactive elements:")
                    for el in elements:
                        print(
                            f"  • {el.get('name','?')} [{el.get('type','?')}]  ({el.get('x','?')}, {el.get('y','?')})"
                        )
            except json.JSONDecodeError:
                print(f"✅ snapshot — {save_path}")
                print()
                print(raw)

        except ImportError:
            await _print_context(state["provider"], state.get("name", ""), state)
            return _fail("'anthropic' package not installed. Run: pip install anthropic")
        except Exception as e:
            await _print_context(state["provider"], state.get("name", ""), state)
            return _fail(f"AI analysis failed: {e}")

        await _print_context(state["provider"], state.get("name", ""), state)
        return 0

    return run_async(_run())


def _cmd_click(args: argparse.Namespace) -> int:
    from cua_cli.utils.async_utils import run_async

    t = _require_target()
    if not t:
        return 1

    button = getattr(args, "button", "left") or "left"
    cmd_map = {"left": "left_click", "right": "right_click", "middle": "middle_click"}
    cmd = cmd_map.get(button, "left_click")

    async def _run() -> int:
        state = _load_state()
        p, n = state["provider"], state.get("name", "")
        await _maybe_focus_zoom(p, n, state)
        sx, sy = _coords(args.x, args.y, state)
        try:
            result = await _send(p, n, cmd, {"x": sx, "y": sy})
        except Exception as e:
            await _print_context(p, n, state)
            return _fail(str(e))
        if not result.get("success", True):
            await _print_context(p, n, state)
            return _fail(result.get("error", "click failed"))
        _scr = await _take_screenshot_for_recording(p, n, state)
        _maybe_record_turn(args, state, "click", {"x": args.x, "y": args.y}, _scr)
        rc = _ok(f"clicked ({args.x}, {args.y}) [{button}]")
        await _print_context(p, n, state)
        return rc

    return run_async(_run())


def _cmd_dclick(args: argparse.Namespace) -> int:
    from cua_cli.utils.async_utils import run_async

    t = _require_target()
    if not t:
        return 1

    async def _run() -> int:
        state = _load_state()
        p, n = state["provider"], state.get("name", "")
        await _maybe_focus_zoom(p, n, state)
        sx, sy = _coords(args.x, args.y, state)
        try:
            result = await _send(p, n, "double_click", {"x": sx, "y": sy})
        except Exception as e:
            await _print_context(p, n, state)
            return _fail(str(e))
        if not result.get("success", True):
            await _print_context(p, n, state)
            return _fail(result.get("error", "double-click failed"))
        _scr = await _take_screenshot_for_recording(p, n, state)
        _maybe_record_turn(args, state, "double_click", {"x": args.x, "y": args.y}, _scr)
        rc = _ok(f"double-clicked ({args.x}, {args.y})")
        await _print_context(p, n, state)
        return rc

    return run_async(_run())


def _cmd_move(args: argparse.Namespace) -> int:
    from cua_cli.utils.async_utils import run_async

    t = _require_target()
    if not t:
        return 1

    async def _run() -> int:
        state = _load_state()
        p, n = state["provider"], state.get("name", "")
        await _maybe_focus_zoom(p, n, state)
        sx, sy = _coords(args.x, args.y, state)
        try:
            result = await _send(p, n, "move_cursor", {"x": sx, "y": sy})
        except Exception as e:
            await _print_context(p, n, state)
            return _fail(str(e))
        if not result.get("success", True):
            await _print_context(p, n, state)
            return _fail(result.get("error", "move failed"))
        _maybe_record_turn(args, state, "move", {"x": args.x, "y": args.y})
        rc = _ok(f"cursor moved to ({args.x}, {args.y})")
        await _print_context(p, n, state)
        return rc

    return run_async(_run())


def _cmd_type(args: argparse.Namespace) -> int:
    from cua_cli.utils.async_utils import run_async

    t = _require_target()
    if not t:
        return 1

    async def _run() -> int:
        state = _load_state()
        p, n = state["provider"], state.get("name", "")
        await _maybe_focus_zoom(p, n, state)
        try:
            result = await _send(p, n, "type_text", {"text": args.text})
        except Exception as e:
            await _print_context(p, n, state)
            return _fail(str(e))
        if not result.get("success", True):
            await _print_context(p, n, state)
            return _fail(result.get("error", "type failed"))
        _scr = await _take_screenshot_for_recording(p, n, state)
        _maybe_record_turn(args, state, "type", {"text": args.text}, _scr)
        preview = args.text[:40] + ("…" if len(args.text) > 40 else "")
        rc = _ok(f"typed: {preview!r}")
        await _print_context(p, n, state)
        return rc

    return run_async(_run())


def _cmd_key(args: argparse.Namespace) -> int:
    from cua_cli.utils.async_utils import run_async

    t = _require_target()
    if not t:
        return 1

    async def _run() -> int:
        state = _load_state()
        p, n = state["provider"], state.get("name", "")
        await _maybe_focus_zoom(p, n, state)
        try:
            result = await _send(p, n, "press_key", {"key": args.key})
        except Exception as e:
            await _print_context(p, n, state)
            return _fail(str(e))
        if not result.get("success", True):
            await _print_context(p, n, state)
            return _fail(result.get("error", "key press failed"))
        _scr = await _take_screenshot_for_recording(p, n, state)
        _maybe_record_turn(args, state, "keypress", {"keys": [args.key]}, _scr)
        rc = _ok(f"pressed key: {args.key}")
        await _print_context(p, n, state)
        return rc

    return run_async(_run())


def _cmd_hotkey(args: argparse.Namespace) -> int:
    from cua_cli.utils.async_utils import run_async

    t = _require_target()
    if not t:
        return 1

    keys = [k.strip() for k in args.keys.replace("-", "+").split("+") if k.strip()]

    async def _run() -> int:
        state = _load_state()
        p, n = state["provider"], state.get("name", "")
        await _maybe_focus_zoom(p, n, state)
        try:
            result = await _send(p, n, "hotkey", {"keys": keys})
        except Exception as e:
            await _print_context(p, n, state)
            return _fail(str(e))
        if not result.get("success", True):
            await _print_context(p, n, state)
            return _fail(result.get("error", "hotkey failed"))
        _scr = await _take_screenshot_for_recording(p, n, state)
        _maybe_record_turn(args, state, "hotkey", {"keys": keys}, _scr)
        rc = _ok(f"hotkey: {'+'.join(keys)}")
        await _print_context(p, n, state)
        return rc

    return run_async(_run())


def _cmd_scroll(args: argparse.Namespace) -> int:
    from cua_cli.utils.async_utils import run_async

    t = _require_target()
    if not t:
        return 1

    async def _run() -> int:
        state = _load_state()
        p, n = state["provider"], state.get("name", "")
        await _maybe_focus_zoom(p, n, state)
        try:
            result = await _send(
                p, n, "scroll_direction", {"direction": args.direction, "clicks": args.amount}
            )
        except Exception as e:
            await _print_context(p, n, state)
            return _fail(str(e))
        if not result.get("success", True):
            await _print_context(p, n, state)
            return _fail(result.get("error", "scroll failed"))
        _scr = await _take_screenshot_for_recording(p, n, state)
        _maybe_record_turn(
            args,
            state,
            "scroll",
            {"scroll_direction": args.direction, "scroll_amount": args.amount},
            _scr,
        )
        rc = _ok(f"scrolled {args.direction} {args.amount}x")
        await _print_context(p, n, state)
        return rc

    return run_async(_run())


def _cmd_drag(args: argparse.Namespace) -> int:
    from cua_cli.utils.async_utils import run_async

    t = _require_target()
    if not t:
        return 1

    async def _run() -> int:
        state = _load_state()
        p, n = state["provider"], state.get("name", "")
        await _maybe_focus_zoom(p, n, state)
        sx1, sy1 = _coords(args.x1, args.y1, state)
        sx2, sy2 = _coords(args.x2, args.y2, state)
        try:
            result = await _send(
                p,
                n,
                "drag_to",
                {"start_x": sx1, "start_y": sy1, "end_x": sx2, "end_y": sy2},
            )
        except Exception as e:
            await _print_context(p, n, state)
            return _fail(str(e))
        if not result.get("success", True):
            await _print_context(p, n, state)
            return _fail(result.get("error", "drag failed"))
        _scr = await _take_screenshot_for_recording(p, n, state)
        _maybe_record_turn(
            args,
            state,
            "drag",
            {"start_x": args.x1, "start_y": args.y1, "end_x": args.x2, "end_y": args.y2},
            _scr,
        )
        rc = _ok(f"dragged ({args.x1},{args.y1}) → ({args.x2},{args.y2})")
        await _print_context(p, n, state)
        return rc

    return run_async(_run())


def _cmd_shell(args: argparse.Namespace) -> int:
    from cua_cli.utils.async_utils import run_async

    t = _require_target()
    if not t:
        return 1

    command = " ".join(args.command)
    if not command.strip():
        return _fail("No command provided")

    async def _run() -> int:
        state = _load_state()
        try:
            result = await _send(
                state["provider"], state.get("name", ""), "run_command", {"command": command}
            )
        except Exception as e:
            await _print_context(state["provider"], state.get("name", ""), state)
            return _fail(str(e))
        if not result.get("success", True):
            await _print_context(state["provider"], state.get("name", ""), state)
            return _fail(result.get("error", "shell command failed"))
        stdout = result.get("stdout", "").strip()
        rc_code = result.get("returncode", 0)
        if rc_code != 0:
            stderr = result.get("stderr", "").strip()
            await _print_context(state["provider"], state.get("name", ""), state)
            return _fail(f"exit {rc_code}: {stderr or stdout}")
        _maybe_record_turn(args, state, "shell", {"command": command})
        preview = (stdout[:80] + "…") if len(stdout) > 80 else stdout
        rc = _ok(preview if preview else "done")
        await _print_context(state["provider"], state.get("name", ""), state)
        return rc

    return run_async(_run())


def _cmd_open(args: argparse.Namespace) -> int:
    from cua_cli.utils.async_utils import run_async

    t = _require_target()
    if not t:
        return 1

    async def _run() -> int:
        state = _load_state()
        try:
            result = await _send(
                state["provider"], state.get("name", ""), "open", {"path": args.path}
            )
        except Exception as e:
            await _print_context(state["provider"], state.get("name", ""), state)
            return _fail(str(e))
        if not result.get("success", True):
            await _print_context(state["provider"], state.get("name", ""), state)
            return _fail(result.get("error", "open failed"))
        _maybe_record_turn(args, state, "open", {"path": args.path})
        rc = _ok(f"opened: {args.path}")
        await _print_context(state["provider"], state.get("name", ""), state)
        return rc

    return run_async(_run())


# ── window subcommands ────────────────────────────────────────────────────────


def _cmd_window(args: argparse.Namespace) -> int:
    from cua_cli.utils.async_utils import run_async

    t = _require_target()
    if not t:
        return 1

    action = args.window_action

    async def _run() -> int:
        state = _load_state()
        p, n = state["provider"], state.get("name", "")

        if action == "ls":
            app = getattr(args, "app", "") or ""
            try:
                result = await _send(p, n, "get_application_windows", {"app": app})
            except Exception as e:
                return _fail(str(e))
            windows = result.get("windows") or result.get("data") or []
            if not isinstance(windows, list):
                print(result)
                return 0
            for wid in windows:
                try:
                    name_r = await _send(p, n, "get_window_name", {"window_id": wid})
                    title = name_r.get("name") or "?"
                    if title in _SKIP_WINDOW_TITLES:
                        continue
                    pos_r = await _send(p, n, "get_window_position", {"window_id": wid})
                    size_r = await _send(p, n, "get_window_size", {"window_id": wid})
                    pos = pos_r.get("position") or pos_r.get("data")
                    size = size_r.get("size") or size_r.get("data")
                    if pos and size:
                        if isinstance(pos, (list, tuple)):
                            x, y = int(pos[0]), int(pos[1])
                        else:
                            x, y = int(pos.get("x", 0)), int(pos.get("y", 0))
                        if isinstance(size, (list, tuple)):
                            w, h = int(size[0]), int(size[1])
                        else:
                            w, h = int(size.get("width", 0)), int(size.get("height", 0))
                        bbox = f"{x},{y},{w},{h}"
                    else:
                        bbox = "?"
                    print(f"  {wid}  {title}  [{bbox}]")
                except Exception:
                    print(f"  {wid}")
            return 0

        if action == "unfocus":
            try:
                result = await _send(p, n, "deactivate_window", {})
            except Exception:
                result = {"success": False}
            if not result.get("success", True):
                try:
                    result = await _send(p, n, "press_key", {"key": "escape"})
                except Exception as e:
                    await _print_context(p, n)
                    return _fail(str(e))
            rc = _ok("unfocused window")
            await _print_context(p, n)
            return rc

        # Commands requiring window_id
        wid = args.window_id

        cmd_map = {
            "focus": ("activate_window", {"window_id": wid}),
            "activate": ("activate_window", {"window_id": wid}),
            "minimize": ("minimize_window", {"window_id": wid}),
            "maximize": ("maximize_window", {"window_id": wid}),
            "close": ("close_window", {"window_id": wid}),
        }

        if action in cmd_map:
            cmd, params = cmd_map[action]
            try:
                result = await _send(p, n, cmd, params)
            except Exception as e:
                await _print_context(p, n)
                return _fail(str(e))
            if not result.get("success", True):
                await _print_context(p, n)
                return _fail(result.get("error", f"{action} failed"))
            rc = _ok(f"{action}d window {wid}")
            await _print_context(p, n)
            return rc

        if action == "resize":
            try:
                result = await _send(
                    p,
                    n,
                    "set_window_size",
                    {"window_id": wid, "width": args.width, "height": args.height},
                )
            except Exception as e:
                await _print_context(p, n)
                return _fail(str(e))
            if not result.get("success", True):
                await _print_context(p, n)
                return _fail(result.get("error", "resize failed"))
            rc = _ok(f"resized window {wid} to {args.width}x{args.height}")
            await _print_context(p, n)
            return rc

        if action == "move":
            try:
                result = await _send(
                    p,
                    n,
                    "set_window_position",
                    {"window_id": wid, "x": args.x, "y": args.y},
                )
            except Exception as e:
                await _print_context(p, n)
                return _fail(str(e))
            if not result.get("success", True):
                await _print_context(p, n)
                return _fail(result.get("error", "move failed"))
            rc = _ok(f"moved window {wid} to ({args.x},{args.y})")
            await _print_context(p, n)
            return rc

        if action == "info":
            try:
                size_r = await _send(p, n, "get_window_size", {"window_id": wid})
                pos_r = await _send(p, n, "get_window_position", {"window_id": wid})
                name_r = await _send(p, n, "get_window_name", {"window_id": wid})
            except Exception as e:
                return _fail(str(e))
            print(
                json.dumps(
                    {
                        "id": wid,
                        "title": name_r.get("name") or name_r.get("data") or "",
                        "size": size_r.get("size") or size_r.get("data"),
                        "position": pos_r.get("position") or pos_r.get("data"),
                    },
                    indent=2,
                )
            )
            return 0

        return _fail(f"Unknown window action: {action}")

    return run_async(_run())


# ── host consent command (registered separately as cua do-host-consent) ───────


def register_host_consent_parser(subparsers: argparse._SubParsersAction) -> None:
    subparsers.add_parser(
        "do-host-consent",
        help="Grant consent to control the host PC with cua do",
        description=(
            "Grants permission for 'cua do switch host', allowing AI agents to control\n"
            "your local keyboard, mouse, and screen via a local computer-server."
        ),
    )


def execute_host_consent(args: argparse.Namespace) -> int:
    _HOST_CONSENT_FILE.parent.mkdir(parents=True, exist_ok=True)
    _HOST_CONSENT_FILE.write_text("consented")
    _update_state(provider="host", name="")
    return _ok("consented — target switched to host")


# ── parser registration ───────────────────────────────────────────────────────


def register_parser(subparsers: argparse._SubParsersAction) -> None:
    p = subparsers.add_parser(
        "do",
        help="Send one-shot automation commands to a VM",
        description=(
            "Send automation commands to a target VM. "
            "Output is a single line starting with ✅ or ❌.\n"
            "A second line shows the current VM + focused window context.\n"
            "Coordinates are in screenshot-image space; zoom/max-length translation is applied automatically."
        ),
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Examples:
  cua do switch docker my-container   Select target VM
  cua do switch host                  Control the local PC (requires consent)
  cua do-host-consent                 Grant consent for host control
  cua do ls                           List VMs for current provider
  cua do ls docker
  cua do status
  cua do zoom "Chrome"                Crop all screenshots to Chrome window
  cua do unzoom
  cua do screenshot
  cua do screenshot --save /tmp/s.png
  cua do snapshot                     Screenshot + AI summary (needs ANTHROPIC_API_KEY)
  cua do snapshot "focus on the form"
  cua do click 100 200
  cua do click 100 200 right
  cua do dclick 100 200
  cua do move 500 300
  cua do type "hello world"
  cua do key enter
  cua do hotkey cmd+c
  cua do scroll down 5
  cua do drag 100 200 400 500
  cua do shell "ls -la"
  cua do open https://example.com
  cua do window ls
  cua do window ls Terminal
  cua do window focus <id>
  cua do window unfocus
  cua do window minimize/maximize/close <id>
  cua do window resize <id> 1280 800
  cua do window move <id> 0 0
  cua do window info <id>
""",
    )

    p.add_argument(
        "--no-record",
        action="store_true",
        default=False,
        help="Disable trajectory recording for this command",
    )

    sub = p.add_subparsers(dest="do_action", metavar="action")
    sub.required = True

    # switch
    sw = sub.add_parser("switch", help="Select target VM (use 'host' for local PC)")
    sw.add_argument("provider", choices=PROVIDERS)
    sw.add_argument("name", nargs="?", default="")

    # status
    sub.add_parser("status", help="Show current target and zoom state")

    # ls
    ls = sub.add_parser("ls", help="List VMs for a provider")
    ls.add_argument("provider", nargs="?", choices=PROVIDERS)

    # zoom / unzoom
    zm = sub.add_parser("zoom", help="Crop screenshots + translate coords to a window")
    zm.add_argument("window_name", help="App/window name to zoom to")
    sub.add_parser("unzoom", help="Return to full-screen screenshots")

    # screenshot
    ss = sub.add_parser("screenshot", help="Take a screenshot (saved to temp dir)")
    ss.add_argument("--save", "-s", metavar="PATH")

    # snapshot
    sn = sub.add_parser(
        "snapshot",
        help="Screenshot + AI screen summary and interactive elements (needs ANTHROPIC_API_KEY)",
    )
    sn.add_argument("instructions", nargs="*", help="Extra instructions for the AI")

    # click
    cl = sub.add_parser("click", help="Click at coordinates (image space)")
    cl.add_argument("x", type=int)
    cl.add_argument("y", type=int)
    cl.add_argument("button", nargs="?", default="left", choices=["left", "right", "middle"])

    # dclick
    dc = sub.add_parser("dclick", help="Double-click at coordinates (image space)")
    dc.add_argument("x", type=int)
    dc.add_argument("y", type=int)

    # move
    mv = sub.add_parser("move", help="Move cursor (image space)")
    mv.add_argument("x", type=int)
    mv.add_argument("y", type=int)

    # type
    ty = sub.add_parser("type", help="Type text")
    ty.add_argument("text")

    # key
    ky = sub.add_parser("key", help="Press a key (enter, escape, tab, …)")
    ky.add_argument("key")

    # hotkey
    hk = sub.add_parser("hotkey", help="Keyboard shortcut (e.g. cmd+c, ctrl+shift+s)")
    hk.add_argument("keys")

    # scroll
    sc = sub.add_parser("scroll", help="Scroll in a direction")
    sc.add_argument("direction", choices=["up", "down", "left", "right"])
    sc.add_argument("amount", nargs="?", type=int, default=3)

    # drag
    dr = sub.add_parser("drag", help="Drag (image space)")
    dr.add_argument("x1", type=int)
    dr.add_argument("y1", type=int)
    dr.add_argument("x2", type=int)
    dr.add_argument("y2", type=int)

    # shell
    sh = sub.add_parser("shell", help="Run a shell command in the VM")
    sh.add_argument("command", nargs=argparse.REMAINDER)

    # open
    op = sub.add_parser("open", help="Open a file or URL")
    op.add_argument("path")

    # window
    win = sub.add_parser("window", help="Window management")
    win_sub = win.add_subparsers(dest="window_action", metavar="action")
    win_sub.required = True

    wls = win_sub.add_parser("ls", help="List windows")
    wls.add_argument("app", nargs="?", default="")

    win_sub.add_parser("unfocus", help="Remove focus from the current window")

    for act in ("focus", "activate", "minimize", "maximize", "close"):
        wp = win_sub.add_parser(act)
        wp.add_argument("window_id")

    wr = win_sub.add_parser("resize")
    wr.add_argument("window_id")
    wr.add_argument("width", type=int)
    wr.add_argument("height", type=int)

    wm = win_sub.add_parser("move")
    wm.add_argument("window_id")
    wm.add_argument("x", type=int)
    wm.add_argument("y", type=int)

    wi = win_sub.add_parser("info")
    wi.add_argument("window_id")


# ── dispatch ──────────────────────────────────────────────────────────────────


def execute(args: argparse.Namespace) -> int:
    dispatch = {
        "switch": _cmd_switch,
        "status": _cmd_status,
        "ls": _cmd_ls,
        "zoom": _cmd_zoom,
        "unzoom": _cmd_unzoom,
        "screenshot": _cmd_screenshot,
        "snapshot": _cmd_snapshot,
        "click": _cmd_click,
        "dclick": _cmd_dclick,
        "move": _cmd_move,
        "type": _cmd_type,
        "key": _cmd_key,
        "hotkey": _cmd_hotkey,
        "scroll": _cmd_scroll,
        "drag": _cmd_drag,
        "shell": _cmd_shell,
        "open": _cmd_open,
        "window": _cmd_window,
    }
    handler = dispatch.get(args.do_action)
    if not handler:
        return _fail(f"Unknown action: {args.do_action}")
    return handler(args)
