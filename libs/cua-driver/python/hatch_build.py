"""Hatch build hook for platform-tagged cua-driver wheels."""

import os

from hatchling.builders.hooks.plugin.interface import BuildHookInterface


class CustomBuildHook(BuildHookInterface):
    """Apply the wheel tag selected by build_wheel.py."""

    def initialize(self, version: str, build_data: dict) -> None:
        wheel_tag = os.environ.get("CUA_DRIVER_WHEEL_TAG")
        if not wheel_tag:
            return

        build_data["tag"] = wheel_tag
        build_data["pure_python"] = False
