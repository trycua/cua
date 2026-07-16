from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.batch_progress import BatchProgress


T = TypeVar("T", bound="LabelBatchSummary")


@_attrs_define
class LabelBatchSummary:
    """
    Attributes:
        batch_id (str | Unset):
        status (str | Unset):
        progress (BatchProgress | Unset):
        created_at (datetime.datetime | Unset):
        updated_at (datetime.datetime | Unset):
        s3_key (str | Unset):
        s3_presigned_url (str | Unset):
    """

    batch_id: str | Unset = UNSET
    status: str | Unset = UNSET
    progress: BatchProgress | Unset = UNSET
    created_at: datetime.datetime | Unset = UNSET
    updated_at: datetime.datetime | Unset = UNSET
    s3_key: str | Unset = UNSET
    s3_presigned_url: str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        batch_id = self.batch_id

        status = self.status

        progress: dict[str, Any] | Unset = UNSET
        if not isinstance(self.progress, Unset):
            progress = self.progress.to_dict()

        created_at: str | Unset = UNSET
        if not isinstance(self.created_at, Unset):
            created_at = self.created_at.isoformat()

        updated_at: str | Unset = UNSET
        if not isinstance(self.updated_at, Unset):
            updated_at = self.updated_at.isoformat()

        s3_key = self.s3_key

        s3_presigned_url = self.s3_presigned_url

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if batch_id is not UNSET:
            field_dict["batch_id"] = batch_id
        if status is not UNSET:
            field_dict["status"] = status
        if progress is not UNSET:
            field_dict["progress"] = progress
        if created_at is not UNSET:
            field_dict["created_at"] = created_at
        if updated_at is not UNSET:
            field_dict["updated_at"] = updated_at
        if s3_key is not UNSET:
            field_dict["s3_key"] = s3_key
        if s3_presigned_url is not UNSET:
            field_dict["s3_presigned_url"] = s3_presigned_url

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.batch_progress import BatchProgress

        d = dict(src_dict)
        batch_id = d.pop("batch_id", UNSET)

        status = d.pop("status", UNSET)

        _progress = d.pop("progress", UNSET)
        progress: BatchProgress | Unset
        if isinstance(_progress, Unset):
            progress = UNSET
        else:
            progress = BatchProgress.from_dict(_progress)

        _created_at = d.pop("created_at", UNSET)
        created_at: datetime.datetime | Unset
        if isinstance(_created_at, Unset):
            created_at = UNSET
        else:
            created_at = datetime.datetime.fromisoformat(_created_at)

        _updated_at = d.pop("updated_at", UNSET)
        updated_at: datetime.datetime | Unset
        if isinstance(_updated_at, Unset):
            updated_at = UNSET
        else:
            updated_at = datetime.datetime.fromisoformat(_updated_at)

        s3_key = d.pop("s3_key", UNSET)

        s3_presigned_url = d.pop("s3_presigned_url", UNSET)

        label_batch_summary = cls(
            batch_id=batch_id,
            status=status,
            progress=progress,
            created_at=created_at,
            updated_at=updated_at,
            s3_key=s3_key,
            s3_presigned_url=s3_presigned_url,
        )

        label_batch_summary.additional_properties = d
        return label_batch_summary

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
