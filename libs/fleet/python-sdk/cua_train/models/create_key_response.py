from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="CreateKeyResponse")


@_attrs_define
class CreateKeyResponse:
    """
    Attributes:
        client_id (str | Unset):
        client_secret (str | Unset):
        name (str | Unset):
        namespace (str | Unset):
        token_url (str | Unset):
    """

    client_id: str | Unset = UNSET
    client_secret: str | Unset = UNSET
    name: str | Unset = UNSET
    namespace: str | Unset = UNSET
    token_url: str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        client_id = self.client_id

        client_secret = self.client_secret

        name = self.name

        namespace = self.namespace

        token_url = self.token_url

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if client_id is not UNSET:
            field_dict["client_id"] = client_id
        if client_secret is not UNSET:
            field_dict["client_secret"] = client_secret
        if name is not UNSET:
            field_dict["name"] = name
        if namespace is not UNSET:
            field_dict["namespace"] = namespace
        if token_url is not UNSET:
            field_dict["token_url"] = token_url

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        client_id = d.pop("client_id", UNSET)

        client_secret = d.pop("client_secret", UNSET)

        name = d.pop("name", UNSET)

        namespace = d.pop("namespace", UNSET)

        token_url = d.pop("token_url", UNSET)

        create_key_response = cls(
            client_id=client_id,
            client_secret=client_secret,
            name=name,
            namespace=namespace,
            token_url=token_url,
        )

        create_key_response.additional_properties = d
        return create_key_response

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
