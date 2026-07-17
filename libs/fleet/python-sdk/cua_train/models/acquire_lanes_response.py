from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="AcquireLanesResponse")


@_attrs_define
class AcquireLanesResponse:
    """
    Attributes:
        vm_ids (list[str] | Unset): One warm VM id per acquired lane. Use as RunConfig.vm_id.
    """

    vm_ids: list[str] | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        vm_ids: list[str] | Unset = UNSET
        if not isinstance(self.vm_ids, Unset):
            vm_ids = self.vm_ids

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if vm_ids is not UNSET:
            field_dict["vm_ids"] = vm_ids

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        vm_ids = cast(list[str], d.pop("vm_ids", UNSET))

        acquire_lanes_response = cls(
            vm_ids=vm_ids,
        )

        acquire_lanes_response.additional_properties = d
        return acquire_lanes_response

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
