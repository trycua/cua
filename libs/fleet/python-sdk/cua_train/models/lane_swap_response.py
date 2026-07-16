from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="LaneSwapResponse")


@_attrs_define
class LaneSwapResponse:
    """
    Attributes:
        lane_id (str):
        vm_id (str):
        wait_seconds (float):
    """

    lane_id: str
    vm_id: str
    wait_seconds: float
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        lane_id = self.lane_id

        vm_id = self.vm_id

        wait_seconds = self.wait_seconds

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "lane_id": lane_id,
                "vm_id": vm_id,
                "wait_seconds": wait_seconds,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        lane_id = d.pop("lane_id")

        vm_id = d.pop("vm_id")

        wait_seconds = d.pop("wait_seconds")

        lane_swap_response = cls(
            lane_id=lane_id,
            vm_id=vm_id,
            wait_seconds=wait_seconds,
        )

        lane_swap_response.additional_properties = d
        return lane_swap_response

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
