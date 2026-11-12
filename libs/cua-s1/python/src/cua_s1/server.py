"""Optional MCP surface for safe Cua-S1 form planning and execution."""

from __future__ import annotations

import functools
import importlib
import ntpath
import os
from pathlib import Path
from typing import Any, Callable, Iterable

from .driver import (
    BaseDriver,
    CuaDriver,
    DriverError,
    WindowTarget,
    target_from_window,
)
from .pdf import PdfError, extract_entities
from .planner import Planner, PlannerError, run_form
from .schema import Entity

DEFAULT_SESSION = f"cua-s1-{os.getpid()}"


class ServerError(RuntimeError):
    def __init__(self, code: str, message: str, **details: Any) -> None:
        super().__init__(message)
        self.code = code
        self.message = message
        self.details = details

    def to_dict(self) -> dict[str, Any]:
        return {"code": self.code, "message": self.message, "details": self.details}


def structured_errors(function: Callable[..., dict[str, Any]]) -> Callable[..., dict[str, Any]]:
    """Return stable error objects instead of leaking transport exceptions."""

    @functools.wraps(function)
    def wrapper(*args: Any, **kwargs: Any) -> dict[str, Any]:
        try:
            return function(*args, **kwargs)
        except (DriverError, PdfError, PlannerError, ServerError) as exc:
            return {"ok": False, "error": _sanitize_public_error(exc.to_dict())}
        except Exception as exc:  # keep unexpected internals out of public responses
            return {
                "ok": False,
                "error": {
                    "code": "internal_error",
                    "message": "The Cua-S1 runtime encountered an unexpected error",
                    "details": {"error_type": type(exc).__name__},
                },
            }

    return wrapper


class RuntimeService:
    def __init__(
        self,
        planner: Planner,
        driver: BaseDriver,
        *,
        allowed_pdf_roots: Iterable[str | Path],
        pdf_base_dir: str | Path | None = None,
    ) -> None:
        roots = tuple(Path(root).expanduser().resolve() for root in allowed_pdf_roots)
        if not roots:
            raise ServerError("no_allowed_pdf_roots", "At least one allowed PDF root is required")
        self.planner = planner
        self.driver = driver
        self.allowed_pdf_roots = roots
        self.pdf_base_dir = Path(pdf_base_dir or roots[0]).expanduser().resolve()

    @structured_errors
    def extract_pdf_entities(self, pdf_path: str) -> dict[str, Any]:
        entities = self._entities(pdf_path, None)
        return {"ok": True, "entities": [_entity_dict(entity) for entity in entities]}

    @structured_errors
    def plan_form(
        self,
        pdf_path: str,
        *,
        pid: int | None = None,
        window_id: int | None = None,
        window_title: str | None = None,
        extra_entities: dict[str, str] | None = None,
        min_confidence: float = 0.5,
        submit: bool = False,
        delivery_mode: str = "background",
    ) -> dict[str, Any]:
        target, title, window = self._target(pid, window_id, window_title)
        report = run_form(
            self.planner,
            self.driver,
            self._entities(pdf_path, extra_entities),
            target=target,
            form_title=title,
            min_confidence=min_confidence,
            execute=False,
            submit=submit,
            delivery_mode=delivery_mode,  # type: ignore[arg-type]
        )
        return {"ok": True, "window": window, "report": report}

    @structured_errors
    def fill_form(
        self,
        pdf_path: str,
        *,
        pid: int | None = None,
        window_id: int | None = None,
        window_title: str | None = None,
        extra_entities: dict[str, str] | None = None,
        min_confidence: float = 0.5,
        execute: bool = False,
        submit: bool = False,
        delivery_mode: str = "background",
    ) -> dict[str, Any]:
        """Plan by default; execution and submission are separate opt-ins."""

        target, title, window = self._target(pid, window_id, window_title)
        report = run_form(
            self.planner,
            self.driver,
            self._entities(pdf_path, extra_entities),
            target=target,
            form_title=title,
            min_confidence=min_confidence,
            execute=execute,
            submit=submit,
            delivery_mode=delivery_mode,  # type: ignore[arg-type]
        )
        return {"ok": True, "window": window, "report": report}

    def _entities(
        self,
        pdf_path: str,
        extra_entities: dict[str, str] | None,
    ) -> list[Entity]:
        entities = extract_entities(
            pdf_path,
            allowed_roots=self.allowed_pdf_roots,
            base_dir=self.pdf_base_dir,
        )
        if extra_entities is not None and not isinstance(extra_entities, dict):
            raise ServerError(
                "invalid_extra_entities",
                "extra_entities must be an object mapping labels to values",
            )
        for label, value in (extra_entities or {}).items():
            if not isinstance(label, str) or not isinstance(value, str):
                raise ServerError(
                    "invalid_extra_entity",
                    "Extra entity labels and values must be strings",
                )
            entities.append(Entity(label, value))
        return entities

    def _target(
        self,
        pid: int | None,
        window_id: int | None,
        window_title: str | None,
    ) -> tuple[WindowTarget, str, dict[str, Any]]:
        if (pid is None) != (window_id is None):
            raise ServerError(
                "incomplete_window_target",
                "pid and window_id must be supplied together",
                pid=pid,
                window_id=window_id,
            )
        if pid is not None and window_id is not None:
            window = self.driver.find_window(pid=pid, window_id=window_id)
            if (
                window_title
                and window_title.casefold() not in str(window.get("title") or "").casefold()
            ):
                raise ServerError(
                    "window_title_mismatch",
                    "The exact window does not match the requested title constraint",
                    pid=pid,
                    window_id=window_id,
                    requested_title=window_title,
                    observed_title=window.get("title"),
                )
        elif window_title:
            window = self.driver.find_window(title_contains=window_title)
        else:
            raise ServerError(
                "window_target_required",
                "Provide an exact pid/window_id pair or a title that resolves to one window",
            )
        target = target_from_window(window)
        return target, str(window.get("title") or window_title or ""), _public_window(window)


def create_service_from_environment() -> RuntimeService:
    """Build the runtime from explicit host configuration.

    ``CUA_S1_PLANNER_FACTORY`` must be ``module:attribute``. The referenced
    trusted host callable may return either a ``Planner`` or a planning backend.
    No model implementation or checkpoint location is embedded here.
    """

    planner_spec = os.environ.get("CUA_S1_PLANNER_FACTORY")
    if not planner_spec:
        raise ServerError(
            "planner_not_configured",
            "Set CUA_S1_PLANNER_FACTORY to a trusted module:attribute factory",
        )
    planner = _load_planner(planner_spec)
    roots = _allowed_roots_from_environment()
    driver = CuaDriver(
        session=os.environ.get("CUA_S1_SESSION", DEFAULT_SESSION),
        transport=os.environ.get("CUA_S1_DRIVER_TRANSPORT", "auto"),  # type: ignore[arg-type]
        binary=os.environ.get("CUA_S1_DRIVER_BINARY"),
    )
    driver.start_session()
    return RuntimeService(planner, driver, allowed_pdf_roots=roots)


def create_mcp_server(service: RuntimeService | None = None) -> Any:
    try:
        from mcp.server.fastmcp import FastMCP
    except ImportError as exc:
        raise ServerError(
            "mcp_dependency_missing",
            "The MCP server requires the optional 'mcp' dependency",
        ) from exc

    runtime = service or create_service_from_environment()
    server = FastMCP(
        "cua-s1",
        instructions=(
            "Plan form actions from a bounded PDF. plan_form and fill_form are dry-run by "
            "default. Set execute=true to authorize mutations and submit=true separately "
            "to authorize one recognized submit-control click. Always identify one exact window."
        ),
    )
    server.tool()(runtime.extract_pdf_entities)
    server.tool()(runtime.plan_form)
    server.tool()(runtime.fill_form)
    return server


def main() -> None:
    server = create_mcp_server()
    server.run(transport="stdio")


def _load_planner(spec: str) -> Planner:
    module_name, separator, attribute_name = spec.partition(":")
    if not separator or not module_name or not attribute_name:
        raise ServerError(
            "invalid_planner_factory",
            "CUA_S1_PLANNER_FACTORY must use module:attribute syntax",
        )
    try:
        attribute = getattr(importlib.import_module(module_name), attribute_name)
        candidate = attribute() if callable(attribute) else attribute
    except Exception as exc:
        raise ServerError(
            "planner_factory_failed",
            "The configured planner factory could not be loaded",
            module=module_name,
            attribute=attribute_name,
            error_type=type(exc).__name__,
        ) from exc
    return candidate if isinstance(candidate, Planner) else Planner(candidate)


def _allowed_roots_from_environment() -> tuple[Path, ...]:
    configured = os.environ.get("CUA_S1_ALLOWED_PDF_ROOTS")
    if not configured:
        return (Path.cwd().resolve(),)
    roots = tuple(
        Path(value).expanduser().resolve()
        for value in configured.split(os.pathsep)
        if value.strip()
    )
    if not roots:
        raise ServerError(
            "no_allowed_pdf_roots",
            "CUA_S1_ALLOWED_PDF_ROOTS did not contain a usable path",
        )
    return roots


def _entity_dict(entity: Entity) -> dict[str, str]:
    return {"label": entity.label, "value": entity.value}


def _public_window(window: dict[str, Any]) -> dict[str, Any]:
    return {
        "pid": window.get("pid"),
        "window_id": window.get("window_id"),
        "app_name": window.get("app_name"),
        "title": window.get("title"),
        "is_on_screen": window.get("is_on_screen"),
        "minimized": window.get("minimized"),
    }


_SENSITIVE_ERROR_KEYS = {
    "allowed_roots",
    "binary",
    "element",
    "label",
    "observed_title",
    "path",
    "requested_title",
    "response",
    "stderr",
    "title",
    "tree_markdown",
    "value",
    "window",
}


def _sanitize_public_error(error: dict[str, Any]) -> dict[str, Any]:
    """Remove host paths and raw accessibility content from public errors."""

    return _sanitize_error_value(error)


def _sanitize_error_value(value: Any, *, key: str | None = None) -> Any:
    if key in _SENSITIVE_ERROR_KEYS:
        return "<redacted>"
    if isinstance(value, dict):
        return {name: _sanitize_error_value(item, key=name) for name, item in value.items()}
    if isinstance(value, list):
        return [_sanitize_error_value(item) for item in value]
    if isinstance(value, tuple):
        return [_sanitize_error_value(item) for item in value]
    if isinstance(value, str) and (os.path.isabs(value) or ntpath.isabs(value)):
        return "<redacted-path>"
    return value


if __name__ == "__main__":
    main()
