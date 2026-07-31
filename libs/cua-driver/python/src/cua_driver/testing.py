"""Background-safe UI testing primitives built on Cua Driver.

The API deliberately exposes only window-scoped inspection and background
delivery. Tests that need foreground input should use a VM or a different test
runner instead of silently interrupting the user's desktop.
"""

from __future__ import annotations

import asyncio
import base64
import json
import re
from dataclasses import dataclass, replace
from datetime import datetime, timezone
from pathlib import Path
from types import TracebackType
from typing import Any, Literal, Protocol, Sequence
from uuid import uuid4


class _ImageLike(Protocol):
    mime_type: str
    data_base64: str


class _ToolResultLike(Protocol):
    text: str
    images: Sequence[_ImageLike]
    structured_json: str | None
    is_error: bool
    error_code: str | None


class DriverLike(Protocol):
    """The part of ``CuaDriver`` used by the test layer."""

    async def call_tool(self, name: str, arguments_json: str) -> _ToolResultLike: ...


class _OwnedDriverLike(Protocol):
    async def shutdown(self) -> None: ...


class CuaTestError(RuntimeError):
    """Base error for the UI testing layer."""


class CuaToolError(CuaTestError):
    """A Cua Driver tool refused or failed an operation."""

    def __init__(self, tool: str, message: str, error_code: str | None = None) -> None:
        self.tool = tool
        self.error_code = error_code
        code = f" ({error_code})" if error_code else ""
        super().__init__(f"{tool}{code}: {message}")


class CuaQueryError(CuaTestError):
    """An element query returned no unique match."""


class CuaWaitTimeout(CuaTestError):
    """A UI condition did not become true before its deadline."""


class CuaForegroundViolation(CuaTestError):
    """The target application became foreground during a background action."""


class CuaWindowOrderViolation(CuaForegroundViolation):
    """The target window rose above the user's active application."""


@dataclass(frozen=True)
class Frame:
    x: float
    y: float
    width: float
    height: float


@dataclass(frozen=True)
class ElementSnapshot:
    element_index: int
    element_token: str | None
    role: str
    label: str | None
    value: str | None
    identifier: str | None
    enabled: bool | None
    selected: bool | None
    frame: Frame | None
    raw: dict[str, Any]


@dataclass(frozen=True)
class WindowSnapshot:
    pid: int
    window_id: int
    title: str
    app_name: str
    is_on_screen: bool | None
    frame: Frame | None
    z_index: int | None


@dataclass(frozen=True)
class DisplaySnapshot:
    display_id: int
    frame: Frame
    scale_factor: float
    is_main: bool
    is_builtin: bool
    is_mirrored: bool


@dataclass(frozen=True)
class AppSnapshot:
    elements: tuple[ElementSnapshot, ...]
    tree: str
    structured: dict[str, Any]


@dataclass(frozen=True)
class ArtifactBundle:
    directory: Path
    tree: Path
    structured: Path
    screenshot: Path | None


_INDEX_RE = re.compile(r"^\s*-\s*\[(\d+)]\s+")
_IDENTIFIER_RE = re.compile(r"(?:^|[\s\[])id=([^\s\]]+)")
_SAFE_NAME_RE = re.compile(r"[^a-zA-Z0-9._-]+")
PlacementPolicy = Literal["secondary", "unchanged"]


def _slug(value: str) -> str:
    cleaned = _SAFE_NAME_RE.sub("-", value).strip("-._")
    return cleaned[:80] or "capture"


def _structured(result: _ToolResultLike) -> dict[str, Any]:
    if not result.structured_json:
        return {}
    value = json.loads(result.structured_json)
    if not isinstance(value, dict):
        raise CuaTestError("Cua Driver returned non-object structured content")
    return value


def _identifier_map(tree: str) -> dict[int, str]:
    identifiers: dict[int, str] = {}
    for line in tree.splitlines():
        index_match = _INDEX_RE.match(line)
        identifier_match = _IDENTIFIER_RE.search(line)
        if index_match and identifier_match:
            identifiers[int(index_match.group(1))] = identifier_match.group(1)
    return identifiers


def _role_key(role: str) -> str:
    value = role.casefold().removeprefix("ax")
    return "".join(character for character in value if character.isalnum())


_ROLE_GROUPS: dict[str, frozenset[str]] = {
    "buttons": frozenset({"button", "pushbutton", "togglebutton", "splitbutton"}),
    "checkboxes": frozenset({"checkbox", "checkbutton"}),
    "groups": frozenset({"group", "pane", "section"}),
    "links": frozenset({"link", "hyperlink"}),
    "menus": frozenset({"menu", "menubar"}),
    "menu_items": frozenset({"menuitem", "menuitemcheckbox", "menuitemradio"}),
    "radio_buttons": frozenset({"radiobutton", "radio"}),
    "rows": frozenset({"row", "tablerow", "listitem"}),
    "sliders": frozenset({"slider"}),
    "tables": frozenset({"table", "grid", "list"}),
    "text_areas": frozenset({"textarea", "documenttext"}),
    "text_fields": frozenset({"textfield", "edit", "entry", "editabletext"}),
}


@dataclass(frozen=True)
class _Selector:
    roles: frozenset[str] | None = None
    identifier: str | None = None
    label: str | None = None
    value: str | None = None
    name: str | None = None

    def matches(self, element: ElementSnapshot) -> bool:
        if self.roles is not None and _role_key(element.role) not in self.roles:
            return False
        if self.identifier is not None and element.identifier != self.identifier:
            return False
        if self.label is not None and element.label != self.label:
            return False
        if self.value is not None and element.value != self.value:
            return False
        if self.name is not None and self.name not in {element.identifier, element.label}:
            return False
        return True

    def describe(self) -> str:
        parts = []
        if self.roles is not None:
            parts.append(f"roles={sorted(self.roles)!r}")
        for key in ("identifier", "label", "value", "name"):
            value = getattr(self, key)
            if value is not None:
                parts.append(f"{key}={value!r}")
        return ", ".join(parts) or "any element"


class ElementCollection:
    """A role-filtered collection such as ``app.buttons``."""

    def __init__(self, app: CuaApplication, roles: frozenset[str] | None) -> None:
        self._app = app
        self._roles = roles

    def __getitem__(self, name: str) -> CuaElement:
        """Match an accessibility identifier first, or an exact label."""

        return CuaElement(self._app, _Selector(roles=self._roles, name=name))

    def by_id(self, identifier: str) -> CuaElement:
        return CuaElement(
            self._app,
            _Selector(roles=self._roles, identifier=identifier),
        )

    def labeled(self, label: str) -> CuaElement:
        return CuaElement(self._app, _Selector(roles=self._roles, label=label))

    def with_value(self, value: str) -> CuaElement:
        return CuaElement(self._app, _Selector(roles=self._roles, value=value))


class CuaElement:
    """A live element query that re-snapshots before every action."""

    def __init__(self, app: CuaApplication, selector: _Selector) -> None:
        self._app = app
        self._selector = selector

    async def exists(self) -> bool:
        snapshot = await self._app.snapshot()
        return bool(self._matches(snapshot))

    async def wait_for_exists(
        self,
        timeout: float = 5.0,
        poll_interval: float = 0.1,
    ) -> ElementSnapshot:
        deadline = asyncio.get_running_loop().time() + timeout
        while True:
            snapshot = await self._app.snapshot()
            matches = self._matches(snapshot)
            if len(matches) == 1:
                return matches[0]
            if len(matches) > 1:
                raise self._ambiguous(matches)
            if asyncio.get_running_loop().time() >= deadline:
                artifacts = await self._app.capture_failure("wait-for-element")
                suffix = f"; artifacts: {artifacts.directory}" if artifacts else ""
                raise CuaWaitTimeout(
                    f"element did not appear within {timeout:.2f}s: "
                    f"{self._selector.describe()}{suffix}"
                )
            await asyncio.sleep(poll_interval)

    async def wait_for_disappearance(
        self,
        timeout: float = 5.0,
        poll_interval: float = 0.1,
    ) -> None:
        deadline = asyncio.get_running_loop().time() + timeout
        while True:
            snapshot = await self._app.snapshot()
            if not self._matches(snapshot):
                return
            if asyncio.get_running_loop().time() >= deadline:
                artifacts = await self._app.capture_failure("wait-for-disappearance")
                suffix = f"; artifacts: {artifacts.directory}" if artifacts else ""
                raise CuaWaitTimeout(
                    f"element still exists after {timeout:.2f}s: "
                    f"{self._selector.describe()}{suffix}"
                )
            await asyncio.sleep(poll_interval)

    async def value(self, timeout: float = 0.0) -> str | None:
        return (await self._resolve(timeout)).value

    async def tap(self, timeout: float = 5.0) -> None:
        element = await self._resolve(timeout)
        await self._app._element_action("click", element)

    async def type_text(self, text: str, timeout: float = 5.0) -> None:
        element = await self._resolve(timeout)
        await self._app._element_action("type_text", element, text=text)

    async def set_value(self, value: str, timeout: float = 5.0) -> None:
        element = await self._resolve(timeout)
        await self._app._element_action("set_value", element, value=value)

    async def press_key(
        self,
        key: str,
        modifiers: Sequence[str] = (),
        timeout: float = 5.0,
    ) -> None:
        element = await self._resolve(timeout)
        await self._app._element_action(
            "press_key",
            element,
            key=key,
            modifiers=list(modifiers),
        )

    async def wait_for_value(
        self,
        expected: str,
        timeout: float = 5.0,
        poll_interval: float = 0.1,
    ) -> ElementSnapshot:
        deadline = asyncio.get_running_loop().time() + timeout
        while True:
            snapshot = await self._app.snapshot()
            matches = self._matches(snapshot)
            if len(matches) > 1:
                raise self._ambiguous(matches)
            if matches and matches[0].value == expected:
                return matches[0]
            if asyncio.get_running_loop().time() >= deadline:
                actual = matches[0].value if matches else None
                artifacts = await self._app.capture_failure("wait-for-value")
                suffix = f"; artifacts: {artifacts.directory}" if artifacts else ""
                raise CuaWaitTimeout(
                    f"element value did not become {expected!r} within {timeout:.2f}s; "
                    f"actual={actual!r}; {self._selector.describe()}{suffix}"
                )
            await asyncio.sleep(poll_interval)

    async def _resolve(self, timeout: float) -> ElementSnapshot:
        if timeout > 0:
            return await self.wait_for_exists(timeout)
        snapshot = await self._app.snapshot()
        matches = self._matches(snapshot)
        if not matches:
            raise CuaQueryError(f"no match for {self._selector.describe()}")
        if len(matches) > 1:
            raise self._ambiguous(matches)
        return matches[0]

    def _matches(self, snapshot: AppSnapshot) -> list[ElementSnapshot]:
        matches = [element for element in snapshot.elements if self._selector.matches(element)]
        if self._selector.name is not None:
            identifier_matches = [
                element for element in matches if element.identifier == self._selector.name
            ]
            if identifier_matches:
                return identifier_matches
        return matches

    def _ambiguous(self, matches: Sequence[ElementSnapshot]) -> CuaQueryError:
        identities = [
            {
                "role": match.role,
                "identifier": match.identifier,
                "label": match.label,
                "value": match.value,
            }
            for match in matches
        ]
        return CuaQueryError(
            f"query matched {len(matches)} elements: {self._selector.describe()}; "
            f"matches={identities!r}"
        )


class CuaApplication:
    """A PID/window-scoped application under background-safe test control."""

    def __init__(
        self,
        session: CuaTestSession,
        window: WindowSnapshot,
        *,
        owns_process: bool,
        launch_metadata: dict[str, Any] | None = None,
    ) -> None:
        self._session = session
        self.window = window
        self.launch_metadata = launch_metadata or {}
        self._owns_process = owns_process
        self._closed = False

    @property
    def pid(self) -> int:
        return self.window.pid

    @property
    def window_id(self) -> int:
        return self.window.window_id

    @property
    def elements(self) -> ElementCollection:
        return ElementCollection(self, None)

    @property
    def buttons(self) -> ElementCollection:
        return self._collection("buttons")

    @property
    def checkboxes(self) -> ElementCollection:
        return self._collection("checkboxes")

    @property
    def groups(self) -> ElementCollection:
        return self._collection("groups")

    @property
    def links(self) -> ElementCollection:
        return self._collection("links")

    @property
    def menus(self) -> ElementCollection:
        return self._collection("menus")

    @property
    def menu_items(self) -> ElementCollection:
        return self._collection("menu_items")

    @property
    def radio_buttons(self) -> ElementCollection:
        return self._collection("radio_buttons")

    @property
    def rows(self) -> ElementCollection:
        return self._collection("rows")

    @property
    def sliders(self) -> ElementCollection:
        return self._collection("sliders")

    @property
    def tables(self) -> ElementCollection:
        return self._collection("tables")

    @property
    def text_areas(self) -> ElementCollection:
        return self._collection("text_areas")

    @property
    def text_fields(self) -> ElementCollection:
        return self._collection("text_fields")

    def _collection(self, name: str) -> ElementCollection:
        return ElementCollection(self, _ROLE_GROUPS[name])

    async def snapshot(
        self,
        *,
        include_screenshot: bool = False,
        screenshot_out_file: Path | None = None,
    ) -> AppSnapshot:
        arguments: dict[str, Any] = {
            "pid": self.pid,
            "window_id": self.window_id,
            "include_screenshot": include_screenshot,
        }
        if screenshot_out_file is not None:
            arguments["screenshot_out_file"] = str(screenshot_out_file)
        result = await self._session._call("get_window_state", arguments)
        data = _structured(result)
        tree = str(data.get("tree_markdown", result.text))
        identifiers = _identifier_map(tree)
        elements = []
        raw_elements = data.get("elements", [])
        if not isinstance(raw_elements, list):
            raise CuaTestError("get_window_state returned a non-array elements field")
        for raw in raw_elements:
            if not isinstance(raw, dict) or "element_index" not in raw:
                continue
            index = int(raw["element_index"])
            frame = raw.get("frame")
            parsed_frame = None
            if isinstance(frame, dict):
                parsed_frame = Frame(
                    x=float(frame.get("x", 0)),
                    y=float(frame.get("y", 0)),
                    width=float(frame.get("w", frame.get("width", 0))),
                    height=float(frame.get("h", frame.get("height", 0))),
                )
            elements.append(
                ElementSnapshot(
                    element_index=index,
                    element_token=_optional_string(raw.get("element_token")),
                    role=str(raw.get("role", "")),
                    label=_optional_string(raw.get("label")),
                    value=_optional_string(raw.get("value")),
                    identifier=_optional_string(raw.get("identifier")) or identifiers.get(index),
                    enabled=_optional_bool(raw.get("enabled")),
                    selected=_optional_bool(raw.get("selected")),
                    frame=parsed_frame,
                    raw=raw,
                )
            )
        return AppSnapshot(tuple(elements), tree, data)

    async def contains_text(self, text: str) -> bool:
        return text in (await self.snapshot()).tree

    async def wait_for_text(
        self,
        text: str,
        timeout: float = 5.0,
        poll_interval: float = 0.1,
    ) -> None:
        deadline = asyncio.get_running_loop().time() + timeout
        while True:
            if await self.contains_text(text):
                return
            if asyncio.get_running_loop().time() >= deadline:
                artifacts = await self.capture_failure("wait-for-text")
                suffix = f"; artifacts: {artifacts.directory}" if artifacts else ""
                raise CuaWaitTimeout(f"text did not appear within {timeout:.2f}s: {text!r}{suffix}")
            await asyncio.sleep(poll_interval)

    async def wait_for_text_to_disappear(
        self,
        text: str,
        timeout: float = 5.0,
        poll_interval: float = 0.1,
    ) -> None:
        deadline = asyncio.get_running_loop().time() + timeout
        while True:
            if not await self.contains_text(text):
                return
            if asyncio.get_running_loop().time() >= deadline:
                artifacts = await self.capture_failure("wait-for-text-disappearance")
                suffix = f"; artifacts: {artifacts.directory}" if artifacts else ""
                raise CuaWaitTimeout(f"text still exists after {timeout:.2f}s: {text!r}{suffix}")
            await asyncio.sleep(poll_interval)

    async def capture_failure(self, name: str) -> ArtifactBundle | None:
        return await self._session._capture(self, name)

    async def close(self) -> None:
        if self._closed:
            return
        if self._owns_process:
            try:
                await self._session._call("kill_app", {"pid": self.pid})
            except CuaToolError as error:
                if "No such process" not in str(error):
                    raise
        self._closed = True

    async def _element_action(
        self,
        tool: str,
        element: ElementSnapshot,
        **arguments: Any,
    ) -> None:
        payload: dict[str, Any] = {
            "pid": self.pid,
            "window_id": self.window_id,
            "element_index": element.element_index,
            **arguments,
        }
        if element.element_token:
            payload["element_token"] = element.element_token
        if tool in {"click", "type_text", "press_key"}:
            payload["delivery_mode"] = "background"
        try:
            await self._session._require_background_window(
                self.window,
                operation=f"{tool} preflight",
            )
        except CuaForegroundViolation as error:
            artifacts = await self.capture_failure("target-already-foreground")
            suffix = f"; artifacts: {artifacts.directory}" if artifacts else ""
            raise type(error)(f"{error}{suffix}") from None
        try:
            await self._session._call(tool, payload)
        except CuaToolError:
            await self.capture_failure(tool)
            raise
        try:
            self.window = await self._session._require_background_window(
                self.window,
                operation=tool,
            )
        except CuaForegroundViolation as error:
            artifacts = await self.capture_failure("foreground-violation")
            suffix = f"; artifacts: {artifacts.directory}" if artifacts else ""
            raise type(error)(f"{error}{suffix}") from None


class CuaTestSession:
    """Owns a driver connection and applications launched by one UI test."""

    def __init__(
        self,
        driver: DriverLike,
        *,
        artifacts_dir: str | Path = "test-results/cua-ui",
        owns_driver: bool = False,
        placement: PlacementPolicy = "secondary",
        _owner_driver: _OwnedDriverLike | None = None,
        _bound_session: Any | None = None,
        _public_session: str | None = None,
    ) -> None:
        if placement not in {"secondary", "unchanged"}:
            raise ValueError("placement must be 'secondary' or 'unchanged'")
        self._driver = driver
        self._artifacts_dir = Path(artifacts_dir)
        self._owner_driver = _owner_driver or (driver if owns_driver else None)
        self._bound_session = _bound_session
        self._public_session = _public_session
        self._session_started = False
        self._placement = placement
        self._apps: list[CuaApplication] = []
        self._closed = False

    @classmethod
    def create(
        cls,
        *,
        artifacts_dir: str | Path = "test-results/cua-ui",
        placement: PlacementPolicy = "secondary",
    ) -> CuaTestSession:
        """Create a same-process driver and bind one isolated test session."""

        from . import (
            ConfiguredDriverOptions,
            CuaDriver,
            RuntimeAuthorizationOptions,
            SessionPermissionMode,
        )

        return cls._bind_native_driver(
            CuaDriver.create_configured(
                ConfiguredDriverOptions(
                    claude_code_compatibility=False,
                    authorization=RuntimeAuthorizationOptions(
                        allowed_modes=[SessionPermissionMode.STANDARD],
                        compatibility_mode=SessionPermissionMode.STANDARD,
                        compatibility_bounded_manifest_path=None,
                        unrestricted_acknowledged=False,
                        max_session_ttl_seconds=60 * 60,
                        max_idle_ttl_seconds=10 * 60,
                    ),
                )
            ),
            artifacts_dir=artifacts_dir,
            placement=placement,
        )

    @classmethod
    def _bind_native_driver(
        cls,
        owner: _OwnedDriverLike,
        *,
        artifacts_dir: str | Path,
        placement: PlacementPolicy,
    ) -> CuaTestSession:
        from . import (
            SessionPermissionMode,
            TrustedSessionOptions,
            create_trusted_session,
        )

        public_session = f"cua-ui-{uuid4().hex}"
        bound = create_trusted_session(
            owner,
            TrustedSessionOptions(
                public_session=public_session,
                mode=SessionPermissionMode.STANDARD,
                ttl_seconds=60 * 60,
                idle_ttl_seconds=10 * 60,
                bounded_manifest_path=None,
            ),
        )
        return cls(
            bound,
            artifacts_dir=artifacts_dir,
            placement=placement,
            _owner_driver=owner,
            _bound_session=bound,
            _public_session=public_session,
        )

    async def __aenter__(self) -> CuaTestSession:
        return self

    async def __aexit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        traceback: TracebackType | None,
    ) -> None:
        await self.close()

    async def launch(
        self,
        *,
        bundle_id: str | None = None,
        name: str | None = None,
        window_title: str | None = None,
        arguments: Sequence[str] = (),
        urls: Sequence[str] = (),
        timeout: float = 10.0,
    ) -> CuaApplication:
        if not bundle_id and not name:
            raise ValueError("launch requires bundle_id or name")
        payload: dict[str, Any] = {
            "creates_new_application_instance": True,
        }
        if bundle_id:
            payload["bundle_id"] = bundle_id
        if name:
            payload["name"] = name
        if arguments:
            payload["additional_arguments"] = list(arguments)
        if urls:
            payload["urls"] = list(urls)

        secondary_display = (
            await self._secondary_display() if self._placement == "secondary" else None
        )
        if secondary_display is not None:
            initial_position: dict[str, Any] = {
                "x": secondary_display.frame.x + 64,
                "y": secondary_display.frame.y + 64,
            }
            if window_title is not None:
                initial_position["title"] = window_title
            payload["initial_window_position"] = initial_position

        active_before = await self._require_active_pid()
        result = await self._call("launch_app", payload)
        data = _structured(result)
        pid = int(data.get("pid", 0))
        if pid <= 0:
            raise CuaTestError(f"launch_app did not return a valid pid: {data!r}")
        try:
            if secondary_display is not None:
                initial_placement = data.get("initial_window_placement")
                initial_bounds = (
                    _parse_frame(initial_placement.get("after_bounds"))
                    if isinstance(initial_placement, dict)
                    else None
                )
                if (
                    not isinstance(initial_placement, dict)
                    or initial_placement.get("verified") is not True
                    or initial_bounds is None
                    or not _frame_center_is_inside(
                        initial_bounds,
                        secondary_display.frame,
                    )
                ):
                    raise CuaTestError(
                        "launch_app did not verify early placement on secondary "
                        f"display {secondary_display.display_id}: "
                        f"{initial_placement!r}"
                    )
            if data.get("self_activation_suppressed") is False and active_before != pid:
                raise CuaForegroundViolation(
                    f"launch_app could not suppress activation of target pid {pid}"
                )

            windows = self._windows_from(data.get("windows", []))
            window = self._select_window(windows, window_title)
            if window is None:
                window = await self._wait_for_window(pid, window_title, timeout)
            window = await self._place_window(window)

            active_after = await self._require_active_pid()
            if active_after == pid:
                raise CuaForegroundViolation(f"launch_app left target pid {pid} in the foreground")
        except BaseException as launch_error:
            try:
                await self._call("kill_app", {"pid": pid})
            except CuaToolError as cleanup_error:
                raise CuaTestError(
                    f"{launch_error}; test app cleanup also failed: {cleanup_error}"
                ) from launch_error
            raise

        app = CuaApplication(
            self,
            window,
            owns_process=True,
            launch_metadata=data,
        )
        self._apps.append(app)
        return app

    async def attach(
        self,
        pid: int,
        *,
        window_id: int | None = None,
        window_title: str | None = None,
        timeout: float = 5.0,
    ) -> CuaApplication:
        windows = await self._list_windows(pid)
        if window_id is not None:
            window = next((item for item in windows if item.window_id == window_id), None)
        else:
            window = self._select_window(windows, window_title)
        if window is None:
            window = await self._wait_for_window(pid, window_title, timeout, window_id)
        window = await self._place_window(window)
        app = CuaApplication(self, window, owns_process=False)
        self._apps.append(app)
        return app

    async def close(self) -> None:
        if self._closed:
            return
        self._closed = True
        first_error: BaseException | None = None
        for app in reversed(self._apps):
            try:
                await app.close()
            except BaseException as error:
                first_error = first_error or error
        if self._session_started and self._public_session is not None:
            try:
                result = await self._driver.call_tool(
                    "end_session",
                    json.dumps({"session": self._public_session}, separators=(",", ":")),
                )
                if result.is_error:
                    raise CuaToolError("end_session", result.text, result.error_code)
            except BaseException as error:
                first_error = first_error or error
        if self._bound_session is not None:
            try:
                self._bound_session.close()
            except BaseException as error:
                first_error = first_error or error
        if self._owner_driver is not None:
            try:
                await self._owner_driver.shutdown()
            except BaseException as error:
                first_error = first_error or error
        if first_error is not None:
            raise first_error

    async def _call(self, name: str, arguments: dict[str, Any]) -> _ToolResultLike:
        if not self._session_started and self._public_session is not None:
            result = await self._driver.call_tool(
                "start_session",
                json.dumps(
                    {
                        "session": self._public_session,
                        "capture_scope": "window",
                    },
                    separators=(",", ":"),
                    sort_keys=True,
                ),
            )
            if result.is_error:
                raise CuaToolError("start_session", result.text, result.error_code)
            self._session_started = True
        result = await self._driver.call_tool(
            name,
            json.dumps(arguments, separators=(",", ":"), sort_keys=True),
        )
        if result.is_error:
            raise CuaToolError(name, result.text, result.error_code)
        return result

    async def _active_pid(self) -> int | None:
        try:
            result = await self._call("get_frontmost_app", {})
            value = _structured(result).get("pid")
            pid = int(value)
            if pid > 0:
                return pid
        except (CuaToolError, TypeError, ValueError):
            pass
        return None

    async def _require_active_pid(self) -> int:
        pid = await self._active_pid()
        if pid is None:
            raise CuaTestError(
                "Cua Driver could not identify a frontmost app; "
                "background safety cannot be verified"
            )
        return pid

    async def _list_windows(self, pid: int | None = None) -> list[WindowSnapshot]:
        arguments = {"pid": pid} if pid is not None else {}
        result = await self._call("list_windows", arguments)
        return self._windows_from(_structured(result).get("windows", []))

    async def _place_window(self, window: WindowSnapshot) -> WindowSnapshot:
        active_pid = await self._require_active_pid()
        if active_pid == window.pid:
            raise CuaForegroundViolation(
                f"window placement refused because target pid {window.pid} " "is already foreground"
            )
        if self._placement == "unchanged":
            return await self._require_background_window(
                window,
                operation="window placement preflight",
            )
        display = await self._secondary_display()
        if display is None:
            return await self._require_background_window(
                window,
                operation="window placement preflight",
            )

        refreshed = await self._list_windows(window.pid)
        current = next(
            (item for item in refreshed if item.window_id == window.window_id),
            None,
        )
        frame = current.frame if current else window.frame
        if frame is None or frame.width <= 0 or frame.height <= 0:
            raise CuaTestError(
                f"window {window.window_id} has no bounds; cannot place it on a secondary display"
            )

        x, y = _placement_origin(frame, display.frame)
        result = await self._call(
            "move_window",
            {
                "pid": window.pid,
                "window_id": window.window_id,
                "x": x,
                "y": y,
            },
        )
        after = _parse_frame(_structured(result).get("after_bounds"))
        if after is None or not _frame_center_is_inside(after, display.frame):
            raise CuaTestError(
                f"move_window did not place window {window.window_id} on "
                f"secondary display {display.display_id}"
            )
        current = await self._require_background_window(
            window,
            operation="move_window",
        )
        return replace(current, frame=after)

    async def _require_background_window(
        self,
        target: WindowSnapshot,
        *,
        operation: str,
    ) -> WindowSnapshot:
        active_pid = await self._require_active_pid()
        if active_pid == target.pid:
            if operation.endswith("preflight"):
                message = (
                    f"{operation} refused because target pid {target.pid} " "is already foreground"
                )
            else:
                message = (
                    f"{operation} activated target pid {target.pid}; "
                    "background-only contract failed"
                )
            raise CuaForegroundViolation(message)

        windows = await self._list_windows()
        current = next(
            (
                window
                for window in windows
                if window.pid == target.pid and window.window_id == target.window_id
            ),
            None,
        )
        if current is None:
            raise CuaTestError(
                f"{operation} could not find target window {target.window_id} "
                f"for pid {target.pid}"
            )
        if current.is_on_screen is False:
            return current
        if current.z_index is None:
            raise CuaTestError(
                f"{operation} cannot verify target window order because z_index is missing"
            )

        active_windows = [
            window
            for window in windows
            if window.pid == active_pid
            and window.is_on_screen is not False
            and window.z_index is not None
            and window.frame is not None
        ]
        if not active_windows:
            raise CuaTestError(
                f"{operation} cannot verify window order because active pid "
                f"{active_pid} has no visible window"
            )
        if current.frame is None:
            raise CuaTestError(
                f"{operation} cannot verify target window order because bounds are missing"
            )
        active_z_index = max(
            window.z_index for window in active_windows if window.z_index is not None
        )
        if current.z_index >= active_z_index:
            raise CuaWindowOrderViolation(
                f"{operation} raised target window {target.window_id} above the "
                f"active application's visible windows"
            )
        return current

    async def _secondary_display(self) -> DisplaySnapshot | None:
        topology_driver = self._owner_driver or self._driver
        result = await topology_driver.call_tool("get_screen_size", "{}")
        if result.is_error:
            raise CuaToolError("get_screen_size", result.text, result.error_code)
        data = _structured(result)
        if "displays" not in data:
            raise CuaTestError(
                "Cua Driver does not expose display geometry required for safe test placement"
            )
        value = data["displays"]
        if not isinstance(value, list):
            raise CuaTestError("get_screen_size returned a non-array displays field")
        displays = [
            display
            for raw in value
            if isinstance(raw, dict)
            for display in [_display_from(raw)]
            if display is not None
        ]
        if not displays:
            raise CuaTestError("Cua Driver reported no usable displays")
        main = next((display for display in displays if display.is_main), None)
        candidates = [
            display
            for display in displays
            if not display.is_main
            and not display.is_mirrored
            and (main is None or display.frame != main.frame)
        ]
        if not candidates:
            return None
        return min(
            candidates,
            key=lambda display: (
                not display.is_builtin,
                -(display.frame.width * display.frame.height),
                display.display_id,
            ),
        )

    async def _wait_for_window(
        self,
        pid: int,
        title: str | None,
        timeout: float,
        window_id: int | None = None,
    ) -> WindowSnapshot:
        deadline = asyncio.get_running_loop().time() + timeout
        while True:
            windows = await self._list_windows(pid)
            if window_id is not None:
                selected = next(
                    (item for item in windows if item.window_id == window_id),
                    None,
                )
            else:
                selected = self._select_window(windows, title)
            if selected is not None:
                return selected
            if asyncio.get_running_loop().time() >= deadline:
                target = f"window_id={window_id}" if window_id is not None else f"title={title!r}"
                raise CuaWaitTimeout(
                    f"pid {pid} did not publish a matching window within {timeout:.2f}s ({target})"
                )
            await asyncio.sleep(0.1)

    def _windows_from(self, value: Any) -> list[WindowSnapshot]:
        if not isinstance(value, list):
            return []
        windows = []
        for raw in value:
            if not isinstance(raw, dict):
                continue
            try:
                windows.append(
                    WindowSnapshot(
                        pid=int(raw["pid"]),
                        window_id=int(raw["window_id"]),
                        title=str(raw.get("title", "")),
                        app_name=str(raw.get("app_name", raw.get("name", ""))),
                        is_on_screen=_optional_bool(raw.get("is_on_screen")),
                        frame=_parse_frame(raw.get("bounds")),
                        z_index=_optional_int(raw.get("z_index")),
                    )
                )
            except (KeyError, TypeError, ValueError):
                continue
        return windows

    def _select_window(
        self,
        windows: Sequence[WindowSnapshot],
        title: str | None,
    ) -> WindowSnapshot | None:
        if title is not None:
            return next((window for window in windows if window.title == title), None)
        return windows[0] if windows else None

    async def _capture(
        self,
        app: CuaApplication,
        name: str,
    ) -> ArtifactBundle | None:
        timestamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S.%fZ")
        directory = self._artifacts_dir / f"{timestamp}-{_slug(name)}"
        directory.mkdir(parents=True, exist_ok=False)
        screenshot = directory / "window.png"
        try:
            result = await self._call(
                "get_window_state",
                {
                    "pid": app.pid,
                    "window_id": app.window_id,
                    "include_screenshot": True,
                    "screenshot_out_file": str(screenshot),
                },
            )
        except CuaToolError as error:
            (directory / "capture-error.txt").write_text(str(error), encoding="utf-8")
            return None
        data = _structured(result)
        tree_path = directory / "tree.txt"
        structured_path = directory / "snapshot.json"
        tree_path.write_text(str(data.get("tree_markdown", result.text)), encoding="utf-8")
        structured_path.write_text(
            json.dumps(data, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )
        screenshot_path: Path | None = screenshot if screenshot.exists() else None
        if screenshot_path is None and result.images:
            first_image = result.images[0]
            if first_image.mime_type == "image/png":
                screenshot.write_bytes(base64.b64decode(first_image.data_base64))
                screenshot_path = screenshot
        return ArtifactBundle(directory, tree_path, structured_path, screenshot_path)


def _optional_string(value: Any) -> str | None:
    return value if isinstance(value, str) else None


def _optional_bool(value: Any) -> bool | None:
    return value if isinstance(value, bool) else None


def _optional_int(value: Any) -> int | None:
    return value if isinstance(value, int) and not isinstance(value, bool) else None


def _parse_frame(value: Any) -> Frame | None:
    if not isinstance(value, dict):
        return None
    try:
        return Frame(
            x=float(value.get("x", 0)),
            y=float(value.get("y", 0)),
            width=float(value.get("w", value.get("width", 0))),
            height=float(value.get("h", value.get("height", 0))),
        )
    except (TypeError, ValueError):
        return None


def _display_from(value: dict[str, Any]) -> DisplaySnapshot | None:
    frame = _parse_frame(value.get("bounds"))
    try:
        display_id = int(value["display_id"])
        scale_factor = float(value.get("scale_factor", 1))
    except (KeyError, TypeError, ValueError):
        return None
    if frame is None or frame.width <= 0 or frame.height <= 0:
        return None
    return DisplaySnapshot(
        display_id=display_id,
        frame=frame,
        scale_factor=scale_factor,
        is_main=value.get("is_main") is True,
        is_builtin=value.get("is_builtin") is True,
        is_mirrored=value.get("is_mirrored") is True,
    )


def _placement_origin(window: Frame, display: Frame, margin: float = 24.0) -> tuple[float, float]:
    def centered(start: float, available: float, length: float) -> float:
        if length + margin * 2 > available:
            return start
        desired = start + (available - length) / 2
        return min(
            max(desired, start + margin),
            start + available - length - margin,
        )

    return (
        centered(display.x, display.width, window.width),
        centered(display.y, display.height, window.height),
    )


def _frame_center_is_inside(frame: Frame, display: Frame) -> bool:
    center_x = frame.x + frame.width / 2
    center_y = frame.y + frame.height / 2
    return (
        display.x <= center_x <= display.x + display.width
        and display.y <= center_y <= display.y + display.height
    )


__all__ = [
    "AppSnapshot",
    "ArtifactBundle",
    "CuaApplication",
    "CuaElement",
    "CuaForegroundViolation",
    "CuaQueryError",
    "CuaTestError",
    "CuaTestSession",
    "CuaToolError",
    "CuaWaitTimeout",
    "CuaWindowOrderViolation",
    "DisplaySnapshot",
    "DriverLike",
    "ElementCollection",
    "ElementSnapshot",
    "Frame",
    "PlacementPolicy",
    "WindowSnapshot",
]
