from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.batch_progress import BatchProgress
    from ..models.label_batch_summary import LabelBatchSummary


T = TypeVar("T", bound="LabelStatusResponse")


@_attrs_define
class LabelStatusResponse:
    """
    Attributes:
        label (str | Unset):
        batch_count (int | Unset):
        all_completed (bool | Unset):
        progress (BatchProgress | Unset):
        batches (list[LabelBatchSummary] | Unset):
    """

    label: str | Unset = UNSET
    batch_count: int | Unset = UNSET
    all_completed: bool | Unset = UNSET
    progress: BatchProgress | Unset = UNSET
    batches: list[LabelBatchSummary] | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        label = self.label

        batch_count = self.batch_count

        all_completed = self.all_completed

        progress: dict[str, Any] | Unset = UNSET
        if not isinstance(self.progress, Unset):
            progress = self.progress.to_dict()

        batches: list[dict[str, Any]] | Unset = UNSET
        if not isinstance(self.batches, Unset):
            batches = []
            for batches_item_data in self.batches:
                batches_item = batches_item_data.to_dict()
                batches.append(batches_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if label is not UNSET:
            field_dict["label"] = label
        if batch_count is not UNSET:
            field_dict["batch_count"] = batch_count
        if all_completed is not UNSET:
            field_dict["all_completed"] = all_completed
        if progress is not UNSET:
            field_dict["progress"] = progress
        if batches is not UNSET:
            field_dict["batches"] = batches

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.batch_progress import BatchProgress
        from ..models.label_batch_summary import LabelBatchSummary

        d = dict(src_dict)
        label = d.pop("label", UNSET)

        batch_count = d.pop("batch_count", UNSET)

        all_completed = d.pop("all_completed", UNSET)

        _progress = d.pop("progress", UNSET)
        progress: BatchProgress | Unset
        if isinstance(_progress, Unset):
            progress = UNSET
        else:
            progress = BatchProgress.from_dict(_progress)

        _batches = d.pop("batches", UNSET)
        batches: list[LabelBatchSummary] | Unset = UNSET
        if _batches is not UNSET:
            batches = []
            for batches_item_data in _batches:
                batches_item = LabelBatchSummary.from_dict(batches_item_data)

                batches.append(batches_item)

        label_status_response = cls(
            label=label,
            batch_count=batch_count,
            all_completed=all_completed,
            progress=progress,
            batches=batches,
        )

        label_status_response.additional_properties = d
        return label_status_response

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
