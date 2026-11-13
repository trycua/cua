"""Hatch build hook for platform-tagged cua wheels."""

import os

from hatchling.builders.hooks.plugin.interface import BuildHookInterface

# Set by build_wheel.py when it also builds the `cua-cli` wheel.
REQUIRE_CLI_ENV = "CUA_SDK_WHEEL_REQUIRES_CLI"
# Set by cli/build_thin_wheel.py.
THIN_CLI_ENV = "CUA_CLI_THIN"


class CustomBuildHook(BuildHookInterface):
    """Apply the wheel tag selected by build_wheel.py, and make the `cua`
    wheel it builds require the `cua-cli` wheel of the same version."""

    def initialize(self, version: str, build_data: dict) -> None:
        # The thin `cua-cli` wheel (cli/build_thin_wheel.py): no binary, one
        # wheel for every platform.
        if self.metadata.name == "cua-cli" and os.environ.get(THIN_CLI_ENV) == "1":
            build_data["tag"] = "py3-none-any"
            build_data["pure_python"] = True
            return
        wheel_tag = os.environ.get("CUA_SDK_WHEEL_TAG")
        if not wheel_tag:
            return
        build_data["tag"] = wheel_tag
        build_data["pure_python"] = False
        if self.metadata.name == "cua" and os.environ.get(REQUIRE_CLI_ENV) == "1":
            build_data.setdefault("dependencies", []).append(
                f"cua-cli=={self.metadata.version}"
            )
