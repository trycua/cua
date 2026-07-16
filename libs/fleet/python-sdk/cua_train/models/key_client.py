from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="KeyClient")


@_attrs_define
class KeyClient:
    """
    Attributes:
        id (str | Unset):
        client_id (str | Unset):
        name (str | Unset):
        namespace (str | Unset):
        owner_sub (str | Unset):
    """

    id: str | Unset = UNSET
    client_id: str | Unset = UNSET
    name: str | Unset = UNSET
    namespace: str | Unset = UNSET
    owner_sub: str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        id = self.id

        client_id = self.client_id

        name = self.name

        namespace = self.namespace

        owner_sub = self.owner_sub

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if id is not UNSET:
            field_dict["id"] = id
        if client_id is not UNSET:
            field_dict["client_id"] = client_id
        if name is not UNSET:
            field_dict["name"] = name
        if namespace is not UNSET:
            field_dict["namespace"] = namespace
        if owner_sub is not UNSET:
            field_dict["owner_sub"] = owner_sub

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        id = d.pop("id", UNSET)

        client_id = d.pop("client_id", UNSET)

        name = d.pop("name", UNSET)

        namespace = d.pop("namespace", UNSET)

        owner_sub = d.pop("owner_sub", UNSET)

        key_client = cls(
            id=id,
            client_id=client_id,
            name=name,
            namespace=namespace,
            owner_sub=owner_sub,
        )

        key_client.additional_properties = d
        return key_client

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
