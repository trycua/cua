from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="LabelInProgressResponse")


@_attrs_define
class LabelInProgressResponse:
    """
    Attributes:
        label (str | Unset):
        batch_count (int | Unset):
        all_completed (bool | Unset):
        message (str | Unset):
    """

    label: str | Unset = UNSET
    batch_count: int | Unset = UNSET
    all_completed: bool | Unset = UNSET
    message: str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        label = self.label

        batch_count = self.batch_count

        all_completed = self.all_completed

        message = self.message

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if label is not UNSET:
            field_dict["label"] = label
        if batch_count is not UNSET:
            field_dict["batch_count"] = batch_count
        if all_completed is not UNSET:
            field_dict["all_completed"] = all_completed
        if message is not UNSET:
            field_dict["message"] = message

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        label = d.pop("label", UNSET)

        batch_count = d.pop("batch_count", UNSET)

        all_completed = d.pop("all_completed", UNSET)

        message = d.pop("message", UNSET)

        label_in_progress_response = cls(
            label=label,
            batch_count=batch_count,
            all_completed=all_completed,
            message=message,
        )

        label_in_progress_response.additional_properties = d
        return label_in_progress_response

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
