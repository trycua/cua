#!/usr/bin/env python3
"""Install a private wheel and gate its native tool inventory."""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
import sys
import tempfile
import zipfile
from email.parser import BytesParser
from pathlib import Path

from pydantic import BaseModel, ConfigDict, Field, ValidationError

PRIVATE_DISTRIBUTION = "cua-driver-hcomp"
REQUIRED_TOOLS = {
    "list_apps",
    "get_window_state",
    "click",
    "drag",
    "scroll",
    "press_key",
    "type_text",
    "set_value",
    "select_text",
}
REQUIRED_PROPERTIES_BY_PLATFORM = {
    "darwin": {},
    "linux": {
        "drag": {"input_route"},
        "set_value": {"delivery_mode"},
    },
}


class WheelGateError(RuntimeError):
    """The wheel does not satisfy the private publication contract."""


class InputSchema(BaseModel):
    model_config = ConfigDict(extra="allow")

    properties: dict[str, object] = Field(default_factory=dict)


class ToolDefinition(BaseModel):
    model_config = ConfigDict(extra="allow")

    name: str
    input_schema: InputSchema = Field(alias="inputSchema")


class ToolInventory(BaseModel):
    model_config = ConfigDict(extra="allow")

    tools: list[ToolDefinition]


class WheelProvenance(BaseModel):
    model_config = ConfigDict(extra="forbid")

    distribution: str
    version: str
    source_sha: str
    packaging_sha: str
    platform: str
    architecture: str
    executable_sha256: str
    sdk_sha256: str


def sha256_bytes(content: bytes) -> str:
    return hashlib.sha256(content).hexdigest()


def expected_version(source_sha: str) -> str:
    validate_git_sha("source_sha", source_sha)
    return f"0.19.3+hcomp.{source_sha[:12]}"


def validate_git_sha(label: str, value: str) -> None:
    if len(value) != 40 or any(character not in "0123456789abcdef" for character in value):
        raise WheelGateError(f"embedded {label} is not a lowercase 40-character Git SHA")


def validate_sha256(label: str, value: str) -> None:
    if len(value) != 64 or any(character not in "0123456789abcdef" for character in value):
        raise WheelGateError(f"embedded {label} is not a lowercase SHA-256 digest")


def one_member(names: list[str], suffix: str) -> str:
    matches = [name for name in names if name.endswith(suffix)]
    if len(matches) != 1:
        raise WheelGateError(f"expected one wheel member ending in {suffix}, found {len(matches)}")
    return matches[0]


def verify_wheel_provenance(wheel: Path) -> WheelProvenance:
    try:
        with zipfile.ZipFile(wheel) as archive:
            names = archive.namelist()
            provenance = WheelProvenance.model_validate_json(
                archive.read(one_member(names, "cua_driver/hcomp_build.json"))
            )
            metadata = BytesParser().parsebytes(
                archive.read(one_member(names, ".dist-info/METADATA"))
            )
            entry_points = archive.read(one_member(names, ".dist-info/entry_points.txt")).decode(
                "utf-8"
            )
            executable_name = "cua-driver.exe" if provenance.platform == "win32" else "cua-driver"
            executable = archive.read(one_member(names, f"cua_driver/bin/{executable_name}"))
            sdk_suffix = {
                "darwin": "cua_driver/libcua_driver_sdk.dylib",
                "linux": "cua_driver/libcua_driver_sdk.so",
                "win32": "cua_driver/cua_driver_sdk.dll",
            }.get(provenance.platform)
            if sdk_suffix is None:
                raise WheelGateError(f"unsupported provenance platform: {provenance.platform}")
            sdk = archive.read(one_member(names, sdk_suffix))
    except (KeyError, OSError, ValidationError, zipfile.BadZipFile) as error:
        raise WheelGateError(f"invalid wheel provenance: {error}") from error

    if provenance.distribution != PRIVATE_DISTRIBUTION:
        raise WheelGateError("wheel provenance has the public distribution identity")
    validate_git_sha("packaging_sha", provenance.packaging_sha)
    validate_sha256("executable_sha256", provenance.executable_sha256)
    validate_sha256("sdk_sha256", provenance.sdk_sha256)
    if provenance.version != expected_version(provenance.source_sha):
        raise WheelGateError("wheel version does not match its source SHA")
    if metadata["Name"] != PRIVATE_DISTRIBUTION or metadata["Version"] != provenance.version:
        raise WheelGateError("wheel metadata does not match embedded provenance")
    if "cua-driver = cua_driver.__main__:main" not in entry_points:
        raise WheelGateError("wheel does not preserve the cua-driver console script")
    if sha256_bytes(executable) != provenance.executable_sha256:
        raise WheelGateError("bundled executable hash does not match provenance")
    if sha256_bytes(sdk) != provenance.sdk_sha256:
        raise WheelGateError("bundled SDK hash does not match provenance")
    return provenance


def validate_inventory(payload: object, platform: str) -> ToolInventory:
    try:
        inventory = ToolInventory.model_validate(payload)
    except ValidationError as error:
        raise WheelGateError(f"invalid tools/list schema: {error}") from error
    required_properties = REQUIRED_PROPERTIES_BY_PLATFORM.get(platform)
    if required_properties is None:
        raise WheelGateError(f"unsupported inventory platform: {platform}")
    tools = {tool.name: tool for tool in inventory.tools}
    missing_tools = sorted(REQUIRED_TOOLS - tools.keys())
    if missing_tools:
        raise WheelGateError(f"missing required tools: {', '.join(missing_tools)}")
    for tool_name, properties in required_properties.items():
        missing = sorted(properties - tools[tool_name].input_schema.properties.keys())
        if missing:
            raise WheelGateError(
                f"{tool_name} is missing required input properties: {', '.join(missing)}"
            )
    return inventory


RUNTIME_PROBE = r"""
import asyncio
import importlib.metadata
import json
import sys
from pathlib import Path

import cua_driver
from cua_driver import CuaDriver


async def probe():
    distribution = importlib.metadata.distribution("cua-driver-hcomp")
    scripts = {
        entry.name: entry.value
        for entry in distribution.entry_points
        if entry.group == "console_scripts"
    }
    if scripts.get("cua-driver") != "cua_driver.__main__:main":
        raise RuntimeError("installed console script identity mismatch")
    if distribution.version != cua_driver.__version__:
        raise RuntimeError("installed package version mismatch")
    driver = CuaDriver.create(None)
    try:
        inventory = json.loads(await driver.list_tools_json())
    finally:
        await driver.shutdown()
    Path(sys.argv[1]).write_text(json.dumps(inventory), encoding="utf-8")


asyncio.run(probe())
"""


def installed_inventory(wheel: Path) -> object:
    with tempfile.TemporaryDirectory(prefix="cua-driver-hcomp-gate-") as temporary:
        root = Path(temporary)
        environment = root / "venv"
        subprocess.run([sys.executable, "-m", "venv", environment], check=True)
        python = environment / ("Scripts/python.exe" if sys.platform == "win32" else "bin/python")
        subprocess.run(
            [
                str(python),
                "-m",
                "pip",
                "install",
                "--disable-pip-version-check",
                "--no-deps",
                str(wheel.resolve()),
            ],
            check=True,
        )
        output = root / "inventory.json"
        subprocess.run([str(python), "-c", RUNTIME_PROBE, str(output)], check=True)
        return json.loads(output.read_text(encoding="utf-8"))


def gate_wheel(wheel: Path, inventory_output: Path) -> None:
    provenance = verify_wheel_provenance(wheel)
    inventory = validate_inventory(installed_inventory(wheel), provenance.platform)
    inventory_output.parent.mkdir(parents=True, exist_ok=True)
    inventory_output.write_text(
        json.dumps(inventory.model_dump(by_alias=True), indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("wheel", type=Path)
    parser.add_argument("--inventory-output", type=Path, required=True)
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    gate_wheel(args.wheel, args.inventory_output)


if __name__ == "__main__":
    main()
