from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.reset_request_task_config import ResetRequestTaskConfig


T = TypeVar("T", bound="ResetRequest")


@_attrs_define
class ResetRequest:
    """
    Attributes:
        task_config (ResetRequestTaskConfig):
        timeout (int):
    """

    task_config: ResetRequestTaskConfig
    timeout: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        task_config = self.task_config.to_dict()

        timeout = self.timeout

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "task_config": task_config,
                "timeout": timeout,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.reset_request_task_config import ResetRequestTaskConfig

        d = dict(src_dict)
        task_config = ResetRequestTaskConfig.from_dict(d.pop("task_config"))

        timeout = d.pop("timeout")

        reset_request = cls(
            task_config=task_config,
            timeout=timeout,
        )

        reset_request.additional_properties = d
        return reset_request

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
