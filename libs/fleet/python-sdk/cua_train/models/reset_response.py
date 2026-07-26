from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="ResetResponse")


@_attrs_define
class ResetResponse:
    """
    Attributes:
        screenshot (str):
        problem (str):
        vm_id (str):
    """

    screenshot: str
    problem: str
    vm_id: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        screenshot = self.screenshot

        problem = self.problem

        vm_id = self.vm_id

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "screenshot": screenshot,
                "problem": problem,
                "vm_id": vm_id,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        screenshot = d.pop("screenshot")

        problem = d.pop("problem")

        vm_id = d.pop("vm_id")

        reset_response = cls(
            screenshot=screenshot,
            problem=problem,
            vm_id=vm_id,
        )

        reset_response.additional_properties = d
        return reset_response

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
