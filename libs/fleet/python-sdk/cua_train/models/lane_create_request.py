from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="LaneCreateRequest")


@_attrs_define
class LaneCreateRequest:
    """Body of POST /lanes.

    ``ttl_seconds`` is the absolute time the claim is reserved for;
    after that the reaper deletes it. With ``autoRenew=true`` the lane
    auto-renew timer in pool-operator pushes this forward, so the
    customer's effective TTL is "as long as the lane exists".

        Attributes:
            ttl_seconds (int | Unset):  Default: 300.
    """

    ttl_seconds: int | Unset = 300
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        ttl_seconds = self.ttl_seconds

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if ttl_seconds is not UNSET:
            field_dict["ttl_seconds"] = ttl_seconds

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        ttl_seconds = d.pop("ttl_seconds", UNSET)

        lane_create_request = cls(
            ttl_seconds=ttl_seconds,
        )

        lane_create_request.additional_properties = d
        return lane_create_request

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
