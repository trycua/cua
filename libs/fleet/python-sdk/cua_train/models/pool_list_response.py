from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.pool_list_response_vms_item import PoolListResponseVmsItem


T = TypeVar("T", bound="PoolListResponse")


@_attrs_define
class PoolListResponse:
    """
    Attributes:
        vms (list[PoolListResponseVmsItem]):
        count (int):
    """

    vms: list[PoolListResponseVmsItem]
    count: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        vms = []
        for vms_item_data in self.vms:
            vms_item = vms_item_data.to_dict()
            vms.append(vms_item)

        count = self.count

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "vms": vms,
                "count": count,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.pool_list_response_vms_item import PoolListResponseVmsItem

        d = dict(src_dict)
        vms = []
        _vms = d.pop("vms")
        for vms_item_data in _vms:
            vms_item = PoolListResponseVmsItem.from_dict(vms_item_data)

            vms.append(vms_item)

        count = d.pop("count")

        pool_list_response = cls(
            vms=vms,
            count=count,
        )

        pool_list_response.additional_properties = d
        return pool_list_response

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
