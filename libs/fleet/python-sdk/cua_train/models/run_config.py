from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.action import Action


T = TypeVar("T", bound="RunConfig")


@_attrs_define
class RunConfig:
    """
    Attributes:
        steps (list[Action]): Ordered actions to execute on the rollout VM. A step with `action_type: "RESET"` is a
            reserved control action: instead of being forwarded to the VM, it hard-resets the VM in place (new vm_id, clean
            baseline desktop), so one run can execute several independent episodes on a warm instance.
        timeout (int | Unset): Per-rollout timeout in seconds (max 24h).
        vm_id (str | Unset): Reattach this run to an existing warm VM (instance reuse / "lane"), typically a vm_id
            returned by a previous `keep_warm` batch, instead of acquiring a fresh one. If that claim has already expired,
            the run transparently falls back to a fresh VM.
    """

    steps: list[Action]
    timeout: int | Unset = UNSET
    vm_id: str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        steps = []
        for steps_item_data in self.steps:
            steps_item = steps_item_data.to_dict()
            steps.append(steps_item)

        timeout = self.timeout

        vm_id = self.vm_id

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "steps": steps,
            }
        )
        if timeout is not UNSET:
            field_dict["timeout"] = timeout
        if vm_id is not UNSET:
            field_dict["vm_id"] = vm_id

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.action import Action

        d = dict(src_dict)
        steps = []
        _steps = d.pop("steps")
        for steps_item_data in _steps:
            steps_item = Action.from_dict(steps_item_data)

            steps.append(steps_item)

        timeout = d.pop("timeout", UNSET)

        vm_id = d.pop("vm_id", UNSET)

        run_config = cls(
            steps=steps,
            timeout=timeout,
            vm_id=vm_id,
        )

        run_config.additional_properties = d
        return run_config

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
