from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.cancel_label_batch_item import CancelLabelBatchItem


T = TypeVar("T", bound="CancelLabelResponse")


@_attrs_define
class CancelLabelResponse:
    """
    Attributes:
        label (str | Unset):
        batch_count (int | Unset):
        cancelled_count (int | Unset):
        already_done (int | Unset):
        batches (list[CancelLabelBatchItem] | Unset):
    """

    label: str | Unset = UNSET
    batch_count: int | Unset = UNSET
    cancelled_count: int | Unset = UNSET
    already_done: int | Unset = UNSET
    batches: list[CancelLabelBatchItem] | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        label = self.label

        batch_count = self.batch_count

        cancelled_count = self.cancelled_count

        already_done = self.already_done

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
        if cancelled_count is not UNSET:
            field_dict["cancelled_count"] = cancelled_count
        if already_done is not UNSET:
            field_dict["already_done"] = already_done
        if batches is not UNSET:
            field_dict["batches"] = batches

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.cancel_label_batch_item import CancelLabelBatchItem

        d = dict(src_dict)
        label = d.pop("label", UNSET)

        batch_count = d.pop("batch_count", UNSET)

        cancelled_count = d.pop("cancelled_count", UNSET)

        already_done = d.pop("already_done", UNSET)

        _batches = d.pop("batches", UNSET)
        batches: list[CancelLabelBatchItem] | Unset = UNSET
        if _batches is not UNSET:
            batches = []
            for batches_item_data in _batches:
                batches_item = CancelLabelBatchItem.from_dict(batches_item_data)

                batches.append(batches_item)

        cancel_label_response = cls(
            label=label,
            batch_count=batch_count,
            cancelled_count=cancelled_count,
            already_done=already_done,
            batches=batches,
        )

        cancel_label_response.additional_properties = d
        return cancel_label_response

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
