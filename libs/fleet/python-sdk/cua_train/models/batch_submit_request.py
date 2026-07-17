from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.run_config import RunConfig


T = TypeVar("T", bound="BatchSubmitRequest")


@_attrs_define
class BatchSubmitRequest:
    """
    Attributes:
        runs (list[RunConfig]):
        concurrency (int | Unset):  Default: 10.
        pool_name (str | Unset): Optional — must match the URL `{pool}` if provided; ignored otherwise.
        name (str | Unset): Optional batch name. Overridden by the URL `{label}` on label routes.
        keep_warm (bool | Unset): Leave each run's VM running when the run ends (instead of shutting it down), so a
            follow-up batch can reattach to it via RunConfig.vm_id (read each lane's surviving id from RunResult.vm_id).
            Best-effort: a warm VM is reaped after its claim TTL (~minutes), after which reattach falls back to a fresh VM.
    """

    runs: list[RunConfig]
    concurrency: int | Unset = 10
    pool_name: str | Unset = UNSET
    name: str | Unset = UNSET
    keep_warm: bool | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        runs = []
        for runs_item_data in self.runs:
            runs_item = runs_item_data.to_dict()
            runs.append(runs_item)

        concurrency = self.concurrency

        pool_name = self.pool_name

        name = self.name

        keep_warm = self.keep_warm

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "runs": runs,
            }
        )
        if concurrency is not UNSET:
            field_dict["concurrency"] = concurrency
        if pool_name is not UNSET:
            field_dict["pool_name"] = pool_name
        if name is not UNSET:
            field_dict["name"] = name
        if keep_warm is not UNSET:
            field_dict["keep_warm"] = keep_warm

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.run_config import RunConfig

        d = dict(src_dict)
        runs = []
        _runs = d.pop("runs")
        for runs_item_data in _runs:
            runs_item = RunConfig.from_dict(runs_item_data)

            runs.append(runs_item)

        concurrency = d.pop("concurrency", UNSET)

        pool_name = d.pop("pool_name", UNSET)

        name = d.pop("name", UNSET)

        keep_warm = d.pop("keep_warm", UNSET)

        batch_submit_request = cls(
            runs=runs,
            concurrency=concurrency,
            pool_name=pool_name,
            name=name,
            keep_warm=keep_warm,
        )

        batch_submit_request.additional_properties = d
        return batch_submit_request

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
