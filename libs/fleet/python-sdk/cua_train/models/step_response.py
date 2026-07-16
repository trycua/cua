from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="StepResponse")


@_attrs_define
class StepResponse:
    """
    Attributes:
        screenshot (None | str):
        is_finish (bool):
        reward (float):
    """

    screenshot: None | str
    is_finish: bool
    reward: float
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        screenshot: None | str
        screenshot = self.screenshot

        is_finish = self.is_finish

        reward = self.reward

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "screenshot": screenshot,
                "is_finish": is_finish,
                "reward": reward,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)

        def _parse_screenshot(data: object) -> None | str:
            if data is None:
                return data
            return cast(None | str, data)

        screenshot = _parse_screenshot(d.pop("screenshot"))

        is_finish = d.pop("is_finish")

        reward = d.pop("reward")

        step_response = cls(
            screenshot=screenshot,
            is_finish=is_finish,
            reward=reward,
        )

        step_response.additional_properties = d
        return step_response

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
