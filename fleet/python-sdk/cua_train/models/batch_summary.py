from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.timing_stats import TimingStats


T = TypeVar("T", bound="BatchSummary")


@_attrs_define
class BatchSummary:
    """
    Attributes:
        n_ok (int | Unset):
        n_fail (int | Unset):
        total_wall_ms (int | Unset):
        reset_stats (TimingStats | Unset):
        step_stats (TimingStats | Unset):
        shutdown_stats (TimingStats | Unset):
    """

    n_ok: int | Unset = UNSET
    n_fail: int | Unset = UNSET
    total_wall_ms: int | Unset = UNSET
    reset_stats: TimingStats | Unset = UNSET
    step_stats: TimingStats | Unset = UNSET
    shutdown_stats: TimingStats | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        n_ok = self.n_ok

        n_fail = self.n_fail

        total_wall_ms = self.total_wall_ms

        reset_stats: dict[str, Any] | Unset = UNSET
        if not isinstance(self.reset_stats, Unset):
            reset_stats = self.reset_stats.to_dict()

        step_stats: dict[str, Any] | Unset = UNSET
        if not isinstance(self.step_stats, Unset):
            step_stats = self.step_stats.to_dict()

        shutdown_stats: dict[str, Any] | Unset = UNSET
        if not isinstance(self.shutdown_stats, Unset):
            shutdown_stats = self.shutdown_stats.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if n_ok is not UNSET:
            field_dict["n_ok"] = n_ok
        if n_fail is not UNSET:
            field_dict["n_fail"] = n_fail
        if total_wall_ms is not UNSET:
            field_dict["total_wall_ms"] = total_wall_ms
        if reset_stats is not UNSET:
            field_dict["reset_stats"] = reset_stats
        if step_stats is not UNSET:
            field_dict["step_stats"] = step_stats
        if shutdown_stats is not UNSET:
            field_dict["shutdown_stats"] = shutdown_stats

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.timing_stats import TimingStats

        d = dict(src_dict)
        n_ok = d.pop("n_ok", UNSET)

        n_fail = d.pop("n_fail", UNSET)

        total_wall_ms = d.pop("total_wall_ms", UNSET)

        _reset_stats = d.pop("reset_stats", UNSET)
        reset_stats: TimingStats | Unset
        if isinstance(_reset_stats, Unset):
            reset_stats = UNSET
        else:
            reset_stats = TimingStats.from_dict(_reset_stats)

        _step_stats = d.pop("step_stats", UNSET)
        step_stats: TimingStats | Unset
        if isinstance(_step_stats, Unset):
            step_stats = UNSET
        else:
            step_stats = TimingStats.from_dict(_step_stats)

        _shutdown_stats = d.pop("shutdown_stats", UNSET)
        shutdown_stats: TimingStats | Unset
        if isinstance(_shutdown_stats, Unset):
            shutdown_stats = UNSET
        else:
            shutdown_stats = TimingStats.from_dict(_shutdown_stats)

        batch_summary = cls(
            n_ok=n_ok,
            n_fail=n_fail,
            total_wall_ms=total_wall_ms,
            reset_stats=reset_stats,
            step_stats=step_stats,
            shutdown_stats=shutdown_stats,
        )

        batch_summary.additional_properties = d
        return batch_summary

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
