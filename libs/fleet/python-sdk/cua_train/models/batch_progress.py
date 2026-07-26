from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="BatchProgress")


@_attrs_define
class BatchProgress:
    """
    Attributes:
        completed (int | Unset):
        total (int | Unset):
        ok (int | Unset):
        failed (int | Unset):
    """

    completed: int | Unset = UNSET
    total: int | Unset = UNSET
    ok: int | Unset = UNSET
    failed: int | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        completed = self.completed

        total = self.total

        ok = self.ok

        failed = self.failed

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if completed is not UNSET:
            field_dict["completed"] = completed
        if total is not UNSET:
            field_dict["total"] = total
        if ok is not UNSET:
            field_dict["ok"] = ok
        if failed is not UNSET:
            field_dict["failed"] = failed

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        completed = d.pop("completed", UNSET)

        total = d.pop("total", UNSET)

        ok = d.pop("ok", UNSET)

        failed = d.pop("failed", UNSET)

        batch_progress = cls(
            completed=completed,
            total=total,
            ok=ok,
            failed=failed,
        )

        batch_progress.additional_properties = d
        return batch_progress

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
