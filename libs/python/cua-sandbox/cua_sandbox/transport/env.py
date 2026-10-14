"""EnvTransport: the computer interfaces over cua-spacesd, via the ``cua`` SDK.

Every sandbox image the cua SDK builds ships cua-spacesd (gRPC and
gRPC-Web on port 3211). This transport maps the interface actions
(``left_click``, ``run_command``, ``read_bytes`` ...) onto the SDK's typed
``SpacesdClient`` and, for the RPCs it has no typed method for, onto
``SpacesdClient.call_json`` (any ``cua.env.v1`` RPC in proto3 JSON).

Sandboxes are daemon-agnostic, so nothing here runs at ``connect()``: the
spacesd is looked up on first use. When it is not there and the runtime
exposed an agentless path (QMP, VNC, SSH, ADB), the transport hands over to
that ``fallback``; otherwise it raises :class:`SpacesdNotAvailable`.
"""

from __future__ import annotations

import asyncio
import base64
import json
import logging
import time
from typing import TYPE_CHECKING, Any, Awaitable, Callable, Dict, Mapping, Optional

from cua_sandbox._sdk import (
    SPACESD_PORT,
    SpacesdNotAvailable,
    is_env_not_available,
    is_not_found,
    millis,
    native,
)
from cua_sandbox.transport.base import Transport, convert_screenshot

if TYPE_CHECKING:
    from cua_sandbox.interfaces.tunnel import TunnelInfo

logger = logging.getLogger(__name__)

EnvFactory = Callable[[], Awaitable[Any]]

#: Services cua-spacesd answers on; a sandbox declaring one serves the
#: HTML5 viewer at ``/viewer/``.
SPACESD_SERVICES = ("env", "viewer")

#: Legacy web display services, in preference order (the same order as
#: `cua sb view NAME --service ...`), used only for images without
#: cua-spacesd.
DISPLAY_SERVICES = ("novnc", "display", "web")


def display_service_name(services: Mapping[str, Optional[int]]) -> Optional[str]:
    """The first legacy web display service in ``services`` (name to guest
    port).

    A service on 5900-5999 is a raw RFB server, not a web page, so it is
    skipped.
    """
    for name in DISPLAY_SERVICES:
        if name not in services:
            continue
        port = services[name]
        if port is not None and 5900 <= int(port) <= 5999:
            continue
        return name
    return None


def novnc_page_url(base: str) -> str:
    """For a legacy image serving noVNC: the noVNC client page under a
    service URL, connecting at once and
    scaled to the window. A path prefix (a gateway route) and any query
    (a signed URL) are kept, and noVNC's websocket follows the same route."""
    from urllib.parse import parse_qsl, urlencode, urlsplit, urlunsplit

    parts = urlsplit(base)
    prefix = parts.path.rstrip("/")
    if prefix.endswith("/vnc.html"):
        return base
    query = parse_qsl(parts.query, keep_blank_values=True)
    params = list(query) + [("autoconnect", "1"), ("resize", "scale")]
    if prefix:
        ws_path = f"{prefix.lstrip('/')}/websockify"
        if parts.query:
            ws_path += f"?{parts.query}"
        params.append(("path", ws_path))
    return urlunsplit((parts.scheme, parts.netloc, f"{prefix}/vnc.html", urlencode(params), ""))


# Aliases the spacesd's KeySpec also accepts (cua_spacesd_client::KeySpec::parse);
# key_down/key_up go through ComputerService/Keyboard JSON, which needs the
# proto enum name, so resolve them here the same way.
_KEY_ALIASES = {
    "ctrl": "CONTROL",
    "control": "CONTROL",
    "option": "ALT",
    "opt": "ALT",
    "cmd": "META",
    "command": "META",
    "super": "META",
    "win": "META",
    "windows": "META",
    "up": "ARROW_UP",
    "down": "ARROW_DOWN",
    "left": "ARROW_LEFT",
    "right": "ARROW_RIGHT",
    "esc": "ESCAPE",
    "return": "ENTER",
    "del": "DELETE",
    "pageup": "PAGE_UP",
    "pagedown": "PAGE_DOWN",
    "backspace": "BACKSPACE",
    "capslock": "CAPS_LOCK",
}
_NAMED_KEYS = {
    "SHIFT",
    "SHIFT_LEFT",
    "SHIFT_RIGHT",
    "CONTROL",
    "CONTROL_LEFT",
    "CONTROL_RIGHT",
    "ALT",
    "ALT_LEFT",
    "ALT_RIGHT",
    "META",
    "META_LEFT",
    "META_RIGHT",
    "FN",
    "CAPS_LOCK",
    "ENTER",
    "TAB",
    "SPACE",
    "BACKSPACE",
    "DELETE",
    "ESCAPE",
    "INSERT",
    "HOME",
    "END",
    "PAGE_UP",
    "PAGE_DOWN",
    "ARROW_UP",
    "ARROW_DOWN",
    "ARROW_LEFT",
    "ARROW_RIGHT",
    "CONTEXT_MENU",
    "HELP",
    "PRINT_SCREEN",
    "SCROLL_LOCK",
    "PAUSE",
    "NUM_LOCK",
    *(f"F{n}" for n in range(1, 25)),
}
_BUTTONS = {
    "left": "MOUSE_BUTTON_LEFT",
    "right": "MOUSE_BUTTON_RIGHT",
    "middle": "MOUSE_BUTTON_MIDDLE",
    "back": "MOUSE_BUTTON_BACK",
    "forward": "MOUSE_BUTTON_FORWARD",
}


def key_input(key: str) -> dict:
    """``cua.env.v1.KeyInput`` JSON for a key name or a single character."""
    alias = _KEY_ALIASES.get(key.lower())
    upper = alias or key.upper().removeprefix("KEY_").replace("-", "_").replace(" ", "_")
    if upper in _NAMED_KEYS:
        return {"named": f"KEY_{upper}"}
    if len(key) == 1:
        return {"character": key}
    raise ValueError(f"unknown key {key!r}")


def _button(button: str) -> str:
    try:
        return _BUTTONS[button.lower()]
    except KeyError:
        raise ValueError(f"unknown mouse button {button!r}") from None


def _point(x: float, y: float) -> dict:
    return {"x": float(x), "y": float(y)}


def _output_dict(output: Any) -> Dict[str, Any]:
    """A ``ProcessOutput`` in the shape the interfaces already accept."""
    exit_info = output.exit
    code = exit_info.code
    if code is None:
        code = 0 if exit_info.success else -1
    stdout = bytes(output.stdout or output.pty or b"").decode("utf-8", "replace")
    stderr = bytes(output.stderr or b"").decode("utf-8", "replace")
    result: Dict[str, Any] = {
        "success": bool(exit_info.success),
        "stdout": stdout,
        "stderr": stderr,
        "return_code": code,
        "returncode": code,
    }
    if exit_info.timed_out:
        result["timed_out"] = True
    if exit_info.error:
        result["error"] = exit_info.error
    return result


class EnvTransport(Transport):
    """Drives a sandbox through cua-spacesd.

    Args:
        url: spacesd URL (``http://host:3211``, a relay or Fleet service URL).
            Ignored when ``env_factory`` is given.
        token: spacesd token, if the driver requires one.
        env_factory: coroutine returning a ``cua.SpacesdClient`` (used by Fleet and
            local runtimes, which know how to reach the driver).
        environment: OS hint (``linux``, ``windows``, ``mac``) used before the
            driver has been reached.
        fallback: an agentless transport (QMP/VNC/SSH/ADB) to use when the
            sandbox has no spacesd.
        ready_timeout: how long to keep retrying the spacesd probe on first
            use (a freshly booted VM starts its driver late). ``0`` probes once.
        native_sandbox: the ``cua.Sandbox`` handle, for port forwarding and
            named services.
    """

    def __init__(
        self,
        url: Optional[str] = None,
        *,
        token: Optional[str] = None,
        env_factory: Optional[EnvFactory] = None,
        environment: Optional[str] = None,
        fallback: Optional[Transport] = None,
        ready_timeout: float = 0.0,
        probe_timeout: float = 10.0,
        native_sandbox: Any = None,
        vnc_url: Optional[str] = None,
    ) -> None:
        if url is None and env_factory is None:
            raise ValueError("EnvTransport needs a spacesd url or an env_factory")
        self._url = url
        self._token = token
        self._factory = env_factory
        self._environment = environment
        self._fallback = fallback
        self._ready_timeout = ready_timeout
        self._probe_timeout = probe_timeout
        self._native_sandbox = native_sandbox
        self._vnc_url = vnc_url
        self._client: Any = None
        self._delegate: Optional[Transport] = None
        self._lock: Optional[asyncio.Lock] = None
        self._processes: Dict[int, Any] = {}
        self._forwards: Dict[int, Any] = {}
        self._connected = False

    # ── lifecycle ────────────────────────────────────────────────────────

    async def connect(self) -> None:
        # Daemon-agnostic: connecting never probes the guest.
        self._connected = True

    async def disconnect(self) -> None:
        self._connected = False
        for process in list(self._processes.values()):
            try:
                await process.detach()
            except Exception:  # noqa: BLE001 - best effort on teardown
                pass
        self._processes.clear()
        for forward in list(self._forwards.values()):
            try:
                await forward.close()
            except Exception:  # noqa: BLE001
                pass
        self._forwards.clear()
        self._client = None
        if self._delegate is not None:
            delegate, self._delegate = self._delegate, None
            await delegate.disconnect()

    @property
    def url(self) -> Optional[str]:
        return self._url

    async def _new_client(self) -> Any:
        if self._factory is not None:
            return await self._factory()
        sb = await _connect_url(self._url, self._token)
        return await sb.spacesd(millis(self._probe_timeout))

    async def spacesd(self) -> Any:
        """The raw ``cua.SpacesdClient`` (typed client escape hatch)."""
        client = await self._env_or_fallback()
        if client is None:
            raise SpacesdNotAvailable(
                "this sandbox is driven through an agentless fallback "
                f"({type(self._delegate).__name__}); it has no cua-spacesd"
            )
        return client

    async def _env_or_fallback(self) -> Any:
        """The env client, or ``None`` once the fallback transport took over."""
        if self._client is not None:
            return self._client
        if self._delegate is not None:
            return None
        if self._lock is None:
            self._lock = asyncio.Lock()
        async with self._lock:
            if self._client is not None:
                return self._client
            if self._delegate is not None:
                return None
            deadline = time.monotonic() + max(0.0, self._ready_timeout)
            delay = 0.5
            while True:
                try:
                    self._client = await self._new_client()
                    return self._client
                except Exception as error:  # noqa: BLE001 - classified below
                    # The SDK's probe failing may just mean the driver has not
                    # started yet; our own SpacesdNotAvailable is a verdict
                    # (for example no `env` service at all) and is final.
                    booting = is_env_not_available(error)
                    if not booting and not isinstance(error, SpacesdNotAvailable):
                        raise
                    if booting and time.monotonic() < deadline:
                        await asyncio.sleep(delay)
                        delay = min(delay * 2, 5.0)
                        continue
                    if self._fallback is not None:
                        logger.info(
                            "no cua-spacesd (%s); using %s",
                            error,
                            type(self._fallback).__name__,
                        )
                        await self._fallback.connect()
                        self._delegate = self._fallback
                        return None
                    raise SpacesdNotAvailable(str(error)) from error

    async def _require_env(self, action: str) -> Any:
        client = await self._env_or_fallback()
        if client is None:
            raise NotImplementedError(
                f"{action} needs cua-spacesd; this sandbox is driven through "
                f"{type(self._delegate).__name__}, which does not support it"
            )
        return client

    # ── Transport API ────────────────────────────────────────────────────

    async def send(self, action: str, **params: Any) -> Any:
        client = await self._env_or_fallback()
        if client is None:
            assert self._delegate is not None
            return await self._delegate.send(action, **params)
        handler = getattr(self, f"_do_{action}", None)
        if handler is not None:
            return await handler(client, **params)
        if "/" in action:
            # Raw RPC passthrough: send("WindowsService/ListWindows", filter={...}).
            return json.loads(await client.call_json(action, json.dumps(params)))
        raise NotImplementedError(f"cua-spacesd has no mapping for action {action!r}")

    async def screenshot(self, format: str = "png", quality: int = 95) -> bytes:
        client = await self._env_or_fallback()
        if client is None:
            assert self._delegate is not None
            return await self._delegate.screenshot(format=format, quality=quality)
        n = native()
        fmt = format.lower()
        if fmt in ("jpeg", "jpg"):
            options = n.ScreenshotOptions(format=n.ImageFormat.JPEG, quality=quality)
        else:
            options = n.ScreenshotOptions(format=n.ImageFormat.PNG)
        shot = await client.screenshot(options)
        data = bytes(shot.image)
        if fmt in ("jpeg", "jpg") and not data.startswith(b"\xff\xd8\xff"):
            # A driver that only encodes PNG still honours the requested format.
            data = convert_screenshot(data, "jpeg", quality)
        return data

    async def get_screen_size(self) -> Dict[str, int]:
        client = await self._env_or_fallback()
        if client is None:
            assert self._delegate is not None
            return await self._delegate.get_screen_size()
        try:
            displays = json.loads(await client.displays())
        except Exception:  # noqa: BLE001 - fall back to a screenshot's size
            displays = []
        if isinstance(displays, dict):
            displays = displays.get("displays", [])
        chosen = next((d for d in displays if d.get("primary")), displays[0] if displays else None)
        if chosen:
            size = chosen.get("nativeSize") or chosen.get("native_size") or {}
            bounds = chosen.get("bounds") or {}
            width = size.get("width") or bounds.get("width")
            height = size.get("height") or bounds.get("height")
            if width and height:
                return {"width": int(width), "height": int(height)}
        shot = await client.screenshot(None)
        return {"width": int(shot.width), "height": int(shot.height)}

    async def get_environment(self) -> str:
        client = await self._env_or_fallback()
        if client is None:
            assert self._delegate is not None
            return await self._delegate.get_environment()
        caps = await client.capabilities()
        family = (caps.os_family or "").lower()
        return {"macos": "mac", "darwin": "mac"}.get(family, family or "linux")

    def _declared_services(self) -> Dict[str, Optional[int]]:
        """The sandbox's named services (name to guest port, when known)."""
        if self._native_sandbox is None:
            return {}
        try:
            info = self._native_sandbox.info()
        except Exception:  # noqa: BLE001 - a handle without info() declares nothing
            return {}
        found: Dict[str, Optional[int]] = {}
        for name in getattr(info, "endpoints", None) or {}:
            found[str(name)] = None
        for name, port in (getattr(info, "services", None) or {}).items():
            found[str(name)] = int(port) if port is not None else None
        return found

    async def get_display_url(self, *, share: bool = False) -> str:
        """A browser URL for the sandbox's display.

        With cua-spacesd (an ``env`` or ``viewer`` service), a link to its
        HTML5 viewer carrying a scoped, expiring viewer ticket; the same
        link serves ``share=True``. For images without cua-spacesd, the page
        of the first declared legacy web display service (``novnc``,
        ``display``, ``web``; a raw RFB port 59xx is skipped, and
        ``share=True`` returns its expiring public link), then the agentless
        fallback's VNC address.
        """
        services = self._declared_services()
        # With no declared services the SDK handle decides whether "env" exists.
        maybe_spacesd = not services or any(name in services for name in SPACESD_SERVICES)
        if self._delegate is None and maybe_spacesd:
            try:
                handle = await self.native_handle()
                return (await handle.viewer_url(None)).url
            except NotImplementedError:
                pass
            except Exception as e:  # noqa: BLE001 - only "no spacesd" falls through
                if not (is_env_not_available(e) or is_not_found(e)):
                    raise
                logger.debug("no cua-spacesd viewer (%s); trying a web display service", e)
        name = display_service_name(services)
        if name is not None:
            service = await self.native_service(name)
            if share:
                base = (await service.public_url()).url
            else:
                base = await service.url()
            return novnc_page_url(base)
        if self._delegate is not None:
            return await self._delegate.get_display_url(share=share)
        if self._vnc_url:
            return self._vnc_url
        if self._fallback is not None:
            return await self._fallback.get_display_url(share=share)
        raise NotImplementedError(
            "this sandbox has no display to open in a browser: run an image "
            "with cua-spacesd (the canonical ghcr.io/trycua images serve the "
            'viewer on the "env" service, port 3211), or stream the display '
            "with `await sb.spacesd()` and open_media"
        )

    async def request_service(
        self,
        name: str,
        *,
        method: str,
        path: str,
        json_body: Any = None,
        headers: Any = None,
    ) -> Any:
        if self._native_sandbox is None:
            raise NotImplementedError("this sandbox exposes no named services")
        import httpx

        body = None if json_body is None else json.dumps(json_body).encode()
        pairs = list(headers.items()) if isinstance(headers, Mapping) else list(headers or [])
        if json_body is not None and not any(k.lower() == "content-type" for k, _ in pairs):
            pairs.append(("content-type", "application/json"))
        header_list = [native().HttpHeader(name=str(k), value=str(v)) for k, v in pairs]
        response = await self._native_sandbox.service(name).request(
            method, path, body, None, header_list or None
        )
        return httpx.Response(
            response.status,
            headers={h.name: h.value for h in response.headers},
            content=bytes(response.body),
            request=httpx.Request(method, f"https://service.invalid{path}"),
        )

    async def native_handle(self) -> Any:
        """The ``cua.Sandbox`` handle behind this transport (services,
        forwards, public URLs)."""
        if self._native_sandbox is None and self._factory is None and self._url:
            self._native_sandbox = await _connect_url(self._url, self._token)
        if self._native_sandbox is None:
            raise NotImplementedError("this sandbox has no cua SDK handle")
        return self._native_sandbox

    async def native_service(self, name: str) -> Any:
        if self._delegate is not None:
            return await self._delegate.native_service(name)
        if self._native_sandbox is None and self._factory is None and self._url:
            self._native_sandbox = await _connect_url(self._url, self._token)
        if self._native_sandbox is None:
            raise NotImplementedError("this sandbox exposes no named services")
        return self._native_sandbox.service(name)

    async def forward_tunnel(self, sandbox_port: int | str) -> "TunnelInfo":
        if self._delegate is not None:
            return await self._delegate.forward_tunnel(sandbox_port)
        if not isinstance(sandbox_port, int):
            raise ValueError("only numeric TCP ports can be forwarded")
        if self._native_sandbox is None and self._factory is None and self._url:
            # A url= sandbox: the SDK forwards over the spacesd's /tunnel
            # WebSocket (and says so when the driver lacks "tunnel.forward").
            self._native_sandbox = await _connect_url(self._url, self._token)
        if self._native_sandbox is None:
            raise NotImplementedError("EnvTransport without a sandbox handle cannot forward ports")
        forward = await self._native_sandbox.forward(sandbox_port)
        return self._track_forward(forward, sandbox_port)

    def _track_forward(self, forward: Any, sandbox_port: int) -> "TunnelInfo":
        """A :class:`TunnelInfo` for a native forward, closed by ``close_tunnel``."""
        from cua_sandbox.interfaces.tunnel import TunnelInfo

        local = forward.local_addr()
        url = forward.url()
        if local:
            host, _, port = local.rpartition(":")
            info = TunnelInfo(host or "127.0.0.1", int(port), sandbox_port, url=url)
        else:
            from urllib.parse import urlparse

            parsed = urlparse(url or "")
            info = TunnelInfo(
                parsed.hostname or "",
                parsed.port or (443 if parsed.scheme == "https" else 80),
                sandbox_port,
                url=url,
            )
        self._forwards[id(info)] = forward
        return info

    async def close_tunnel(self, info: "TunnelInfo") -> None:
        if self._delegate is not None:
            await self._delegate.close_tunnel(info)
            return
        forward = self._forwards.pop(id(info), None)
        if forward is not None:
            await forward.close()

    # ── PTY sessions (ProcessService) ────────────────────────────────────

    async def pty_create(
        self,
        command: Optional[str] = None,
        cols: int = 120,
        rows: int = 40,
        cwd: Optional[str] = None,
        envs: Optional[Dict[str, str]] = None,
    ) -> Dict[str, Any]:
        client = await self._env_or_fallback()
        if client is None:
            assert self._delegate is not None
            return await self._delegate.pty_create(
                command=command, cols=cols, rows=rows, cwd=cwd, envs=envs
            )
        n = native()
        windows = (self._environment or "").lower() == "windows"
        if windows:
            program = "powershell.exe"
            args = ["-NoLogo"] + (["-Command", command] if command else [])
        else:
            program = "/bin/sh"
            args = ["-c", command] if command else ["-l"]
        process = await client.spawn(
            n.SpacesdCommand(
                program=program,
                args=args,
                env=dict(envs or {}),
                cwd=cwd,
                pty=n.PtySize(cols=cols, rows=rows),
            )
        )
        pid = int(process.pid())
        self._processes[pid] = process
        return {"pid": pid, "cols": cols, "rows": rows}

    async def pty_send(self, pid: int, data: str) -> None:
        process = self._processes.get(pid)
        if process is None:
            client = await self._require_env("pty_send")
            process = await client.attach(pid, None, native().ReplayMode.NONE())
            self._processes[pid] = process
        await process.write_pty(data.encode())

    async def pty_kill(self, pid: int) -> bool:
        process = self._processes.pop(pid, None)
        if process is None:
            client = await self._require_env("pty_kill")
            try:
                process = await client.attach(pid, None, native().ReplayMode.NONE())
            except Exception as error:  # noqa: BLE001
                if is_not_found(error):
                    return False
                raise
        await process.kill()
        return True

    async def pty_info(self, pid: int) -> Optional[Dict[str, Any]]:
        client = await self._require_env("pty_info")
        listing = json.loads(await client.list_processes(False))
        processes = listing.get("processes", listing) if isinstance(listing, dict) else listing
        for proc in processes or []:
            if int(proc.get("pid", -1)) != pid:
                continue
            state = str(proc.get("state", ""))
            if "EXITED" in state.upper():
                return None
            return {"pid": pid, "running": True, "pty": bool(proc.get("pty", False))}
        return None

    # ── action handlers ──────────────────────────────────────────────────

    async def _pointer(self, client: Any, action: dict) -> None:
        await client.pointer_json(json.dumps(action))

    async def _do_left_click(self, client: Any, x: float, y: float, button: str = "left") -> None:
        if button == "left":
            await client.click(float(x), float(y))
        elif button == "right":
            await client.right_click(float(x), float(y))
        else:
            await self._pointer(
                client,
                {"click": {"position": _point(x, y), "button": _button(button), "count": 1}},
            )

    async def _do_right_click(self, client: Any, x: float, y: float) -> None:
        await client.right_click(float(x), float(y))

    async def _do_double_click(self, client: Any, x: float, y: float) -> None:
        await client.double_click(float(x), float(y))

    async def _do_move_cursor(self, client: Any, x: float, y: float) -> None:
        await client.move_to(float(x), float(y))

    async def _do_scroll(
        self,
        client: Any,
        x: float,
        y: float,
        scroll_x: float = 0,
        scroll_y: float = 3,
    ) -> None:
        # (x, y) is where to scroll; scroll_x/scroll_y are wheel notches with
        # cua-sandbox's convention (positive scroll_y scrolls up, positive
        # scroll_x scrolls right, as on QMP). The spacesd's
        # PointerScroll scrolls the content down for a positive delta_y, so
        # the vertical sign flips. computer-server read the position as the
        # deltas, so every scroll there was a jump of hundreds of lines.
        await self._pointer(
            client,
            {
                "scroll": {
                    "position": _point(x, y),
                    "deltaX": float(scroll_x),
                    "deltaY": -float(scroll_y),
                    "unit": "SCROLL_UNIT_LINE",
                }
            },
        )

    async def _do_mouse_down(self, client: Any, x: float, y: float, button: str = "left") -> None:
        await self._pointer(client, {"down": {"position": _point(x, y), "button": _button(button)}})

    async def _do_mouse_up(self, client: Any, x: float, y: float, button: str = "left") -> None:
        await self._pointer(client, {"up": {"position": _point(x, y), "button": _button(button)}})

    async def _do_drag(self, client: Any, path: list, button: str = "left", **_: Any) -> None:
        if len(path) < 2:
            raise ValueError("drag needs at least a start and an end point")
        (sx, sy), (ex, ey) = path[0], path[-1]
        if button == "left" and len(path) == 2:
            await client.drag(float(sx), float(sy), float(ex), float(ey))
            return
        await self._pointer(
            client,
            {
                "drag": {
                    "from": _point(sx, sy),
                    "to": _point(ex, ey),
                    "path": [_point(px, py) for px, py in path[1:-1]],
                    "button": _button(button),
                }
            },
        )

    async def _do_get_cursor_position(self, client: Any) -> Dict[str, float]:
        point = await client.cursor_position()
        return {"x": point.x, "y": point.y}

    async def _do_type_text(self, client: Any, text: str) -> None:
        await client.type_text(text)

    async def _do_hotkey(self, client: Any, keys: list) -> None:
        if len(keys) == 1:
            await client.press(keys[0])
        else:
            await client.hotkey(list(keys))

    async def _do_press_key(self, client: Any, key: str) -> None:
        await client.press(key)

    async def _do_key_down(self, client: Any, key: str) -> None:
        await client.keyboard_json(json.dumps({"down": {"key": key_input(key)}}))

    async def _do_key_up(self, client: Any, key: str) -> None:
        await client.keyboard_json(json.dumps({"up": {"key": key_input(key)}}))

    async def _do_copy_to_clipboard(self, client: Any) -> Dict[str, str]:
        return {"content": (await client.get_clipboard()) or ""}

    async def _do_set_clipboard(self, client: Any, text: str) -> None:
        await client.set_clipboard(text)

    async def _do_run_command(
        self, client: Any, command: str, timeout: Optional[float] = None, **_: Any
    ) -> Dict[str, Any]:
        timeout_ms = millis(timeout) if timeout else None
        return _output_dict(await client.sh(command, timeout_ms))

    async def _do_shell(self, client: Any, command: str, **params: Any) -> Dict[str, Any]:
        return await self._do_run_command(client, command, **params)

    async def _do_get_active_window_title(self, client: Any) -> str:
        listing = json.loads(await client.call_json("WindowsService/ListWindows", "{}"))
        windows = listing.get("windows", []) if isinstance(listing, dict) else []
        focused = next((w for w in windows if w.get("focused")), None)
        return (focused or {}).get("title", "") or ""

    async def _do_get_screen_size(self, client: Any) -> Dict[str, int]:
        return await self.get_screen_size()

    # files (FilesystemService)

    async def _stat(self, client: Any, path: str) -> Any:
        try:
            return await client.stat(path)
        except Exception as error:  # noqa: BLE001
            if is_not_found(error):
                return None
            raise

    async def _do_file_exists(self, client: Any, path: str) -> Dict[str, bool]:
        entry = await self._stat(client, path)
        return {"exists": entry is not None and entry.kind != "directory"}

    async def _do_directory_exists(self, client: Any, path: str) -> Dict[str, bool]:
        entry = await self._stat(client, path)
        return {"exists": entry is not None and entry.kind == "directory"}

    async def _do_get_file_size(self, client: Any, path: str) -> Dict[str, int]:
        return {"size": int((await client.stat(path)).size)}

    async def _do_list_dir(self, client: Any, path: str) -> Dict[str, Any]:
        entries = await client.list_dir(path, 1)
        return {
            "files": [
                {
                    "name": e.name,
                    "path": e.path,
                    "is_dir": e.kind == "directory",
                    "size": int(e.size),
                }
                for e in entries
                if e.path.rstrip("/") != path.rstrip("/")
            ]
        }

    async def _do_create_dir(self, client: Any, path: str) -> Dict[str, bool]:
        await client.make_dir(path)
        return {"success": True}

    async def _do_delete_dir(self, client: Any, path: str) -> Dict[str, bool]:
        await client.remove(path, True)
        return {"success": True}

    async def _do_delete_file(self, client: Any, path: str) -> Dict[str, bool]:
        await client.remove(path, False)
        return {"success": True}

    async def _do_read_text(self, client: Any, path: str, **_: Any) -> Dict[str, str]:
        data = bytes(await client.download(path))
        return {"content": data.decode("utf-8")}

    async def _do_write_text(self, client: Any, path: str, content: str, **_: Any) -> dict:
        await client.upload(path, content.encode("utf-8"), None)
        return {"success": True}

    async def _do_read_bytes(
        self, client: Any, path: str, offset: int = 0, length: Optional[int] = None, **_: Any
    ) -> Dict[str, str]:
        data = bytes(await client.download(path))
        end = None if length is None else offset + length
        return {"content_b64": base64.b64encode(data[offset:end]).decode("ascii")}

    async def _do_write_bytes(self, client: Any, path: str, content_b64: str, **_: Any) -> dict:
        await client.upload(path, base64.b64decode(content_b64), None)
        return {"success": True}


async def _connect_url(url: Optional[str], token: Optional[str]) -> Any:
    from cua_sandbox._sdk import connect_url

    assert url is not None
    return await connect_url(url, token)


def env_url(host: str, port: int = SPACESD_PORT) -> str:
    """``http://host:port`` with IPv6 hosts bracketed."""
    if ":" in host and not host.startswith("["):
        host = f"[{host}]"
    return f"http://{host}:{port}"
