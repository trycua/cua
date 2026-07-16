from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="TimingStats")


@_attrs_define
class TimingStats:
    """
    Attributes:
        n (int | Unset):
        min_ (int | Unset):
        mean (float | Unset):
        max_ (int | Unset):
    """

    n: int | Unset = UNSET
    min_: int | Unset = UNSET
    mean: float | Unset = UNSET
    max_: int | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        n = self.n

        min_ = self.min_

        mean = self.mean

        max_ = self.max_

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if n is not UNSET:
            field_dict["n"] = n
        if min_ is not UNSET:
            field_dict["min"] = min_
        if mean is not UNSET:
            field_dict["mean"] = mean
        if max_ is not UNSET:
            field_dict["max"] = max_

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        n = d.pop("n", UNSET)

        min_ = d.pop("min", UNSET)

        mean = d.pop("mean", UNSET)

        max_ = d.pop("max", UNSET)

        timing_stats = cls(
            n=n,
            min_=min_,
            mean=mean,
            max_=max_,
        )

        timing_stats.additional_properties = d
        return timing_stats

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
