from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="RunResult")


@_attrs_define
class RunResult:
    """
    Attributes:
        run_id (int | Unset):
        vm_id (str | Unset):
        ok (bool | Unset):
        error (str | Unset):
        reset_ms (int | Unset):
        step_ms (list[int] | Unset):
        shutdown_ms (int | Unset):
        actions (list[str] | Unset):
        screenshots (list[str] | Unset):
    """

    run_id: int | Unset = UNSET
    vm_id: str | Unset = UNSET
    ok: bool | Unset = UNSET
    error: str | Unset = UNSET
    reset_ms: int | Unset = UNSET
    step_ms: list[int] | Unset = UNSET
    shutdown_ms: int | Unset = UNSET
    actions: list[str] | Unset = UNSET
    screenshots: list[str] | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        run_id = self.run_id

        vm_id = self.vm_id

        ok = self.ok

        error = self.error

        reset_ms = self.reset_ms

        step_ms: list[int] | Unset = UNSET
        if not isinstance(self.step_ms, Unset):
            step_ms = self.step_ms

        shutdown_ms = self.shutdown_ms

        actions: list[str] | Unset = UNSET
        if not isinstance(self.actions, Unset):
            actions = self.actions

        screenshots: list[str] | Unset = UNSET
        if not isinstance(self.screenshots, Unset):
            screenshots = self.screenshots

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if run_id is not UNSET:
            field_dict["run_id"] = run_id
        if vm_id is not UNSET:
            field_dict["vm_id"] = vm_id
        if ok is not UNSET:
            field_dict["ok"] = ok
        if error is not UNSET:
            field_dict["error"] = error
        if reset_ms is not UNSET:
            field_dict["reset_ms"] = reset_ms
        if step_ms is not UNSET:
            field_dict["step_ms"] = step_ms
        if shutdown_ms is not UNSET:
            field_dict["shutdown_ms"] = shutdown_ms
        if actions is not UNSET:
            field_dict["actions"] = actions
        if screenshots is not UNSET:
            field_dict["screenshots"] = screenshots

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        run_id = d.pop("run_id", UNSET)

        vm_id = d.pop("vm_id", UNSET)

        ok = d.pop("ok", UNSET)

        error = d.pop("error", UNSET)

        reset_ms = d.pop("reset_ms", UNSET)

        step_ms = cast(list[int], d.pop("step_ms", UNSET))

        shutdown_ms = d.pop("shutdown_ms", UNSET)

        actions = cast(list[str], d.pop("actions", UNSET))

        screenshots = cast(list[str], d.pop("screenshots", UNSET))

        run_result = cls(
            run_id=run_id,
            vm_id=vm_id,
            ok=ok,
            error=error,
            reset_ms=reset_ms,
            step_ms=step_ms,
            shutdown_ms=shutdown_ms,
            actions=actions,
            screenshots=screenshots,
        )

        run_result.additional_properties = d
        return run_result

    @property
    def additional_keys(self) -> list[str]:
        return list(self.additional_properties.keys())

    def __getitem__(self, key: str) -> Any:
        return self.additional_properties[key]

    def __setitem__(self, key: str, value: Any) -> None:
        self.additional_properties[key] = value

    def __delitem__(self, key: str) -> None:
        del self.additional_properties[key]

    def __contains__(self, key: str) -> bool:
        return key in self.additional_properties
