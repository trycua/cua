from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.batch_summary import BatchSummary
    from ..models.run_result import RunResult


T = TypeVar("T", bound="BatchResultsResponse")


@_attrs_define
class BatchResultsResponse:
    """
    Attributes:
        batch_id (str | Unset):
        name (str | Unset):
        status (str | Unset):
        runs (list[RunResult] | Unset):
        s3_bucket (str | Unset):
        s3_key (str | Unset):
        s3_presigned_url (str | Unset):
        summary (BatchSummary | Unset):
    """

    batch_id: str | Unset = UNSET
    name: str | Unset = UNSET
    status: str | Unset = UNSET
    runs: list[RunResult] | Unset = UNSET
    s3_bucket: str | Unset = UNSET
    s3_key: str | Unset = UNSET
    s3_presigned_url: str | Unset = UNSET
    summary: BatchSummary | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        batch_id = self.batch_id

        name = self.name

        status = self.status

        runs: list[dict[str, Any]] | Unset = UNSET
        if not isinstance(self.runs, Unset):
            runs = []
            for runs_item_data in self.runs:
                runs_item = runs_item_data.to_dict()
                runs.append(runs_item)

        s3_bucket = self.s3_bucket

        s3_key = self.s3_key

        s3_presigned_url = self.s3_presigned_url

        summary: dict[str, Any] | Unset = UNSET
        if not isinstance(self.summary, Unset):
            summary = self.summary.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if batch_id is not UNSET:
            field_dict["batch_id"] = batch_id
        if name is not UNSET:
            field_dict["name"] = name
        if status is not UNSET:
            field_dict["status"] = status
        if runs is not UNSET:
            field_dict["runs"] = runs
        if s3_bucket is not UNSET:
            field_dict["s3_bucket"] = s3_bucket
        if s3_key is not UNSET:
            field_dict["s3_key"] = s3_key
        if s3_presigned_url is not UNSET:
            field_dict["s3_presigned_url"] = s3_presigned_url
        if summary is not UNSET:
            field_dict["summary"] = summary

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.batch_summary import BatchSummary
        from ..models.run_result import RunResult

        d = dict(src_dict)
        batch_id = d.pop("batch_id", UNSET)

        name = d.pop("name", UNSET)

        status = d.pop("status", UNSET)

        _runs = d.pop("runs", UNSET)
        runs: list[RunResult] | Unset = UNSET
        if _runs is not UNSET:
            runs = []
            for runs_item_data in _runs:
                runs_item = RunResult.from_dict(runs_item_data)

                runs.append(runs_item)

        s3_bucket = d.pop("s3_bucket", UNSET)

        s3_key = d.pop("s3_key", UNSET)

        s3_presigned_url = d.pop("s3_presigned_url", UNSET)

        _summary = d.pop("summary", UNSET)
        summary: BatchSummary | Unset
        if isinstance(_summary, Unset):
            summary = UNSET
        else:
            summary = BatchSummary.from_dict(_summary)

        batch_results_response = cls(
            batch_id=batch_id,
            name=name,
            status=status,
            runs=runs,
            s3_bucket=s3_bucket,
            s3_key=s3_key,
            s3_presigned_url=s3_presigned_url,
            summary=summary,
        )

        batch_results_response.additional_properties = d
        return batch_results_response

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
