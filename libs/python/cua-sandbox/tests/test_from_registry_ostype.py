"""A registry image must be able to say it is Windows.

os_type selects the firmware: Windows guest disks are built UEFI-only, and both
the local QEMU runtime and the Fleet transport read os_type to decide. With
from_registry hardcoding "linux", a Windows containerDisk pulled from any
registry was handed BIOS and could not boot — the same failure fixed for Fleet
in #3125, through a different door. The only escape was reaching past the
constructor with dataclasses.replace().
"""

import inspect

import pytest
from cua_sandbox import Image

REF = "ghcr.io/trycua/minecraft-workspace:1.20.1"


def test_defaults_to_linux_so_existing_callers_are_unaffected():
    image = Image.from_registry(REF)
    assert image.os_type == "linux"
    assert image.kind is None


def test_a_windows_container_disk_can_say_so():
    image = Image.from_registry(REF, os_type="windows", kind="vm")
    assert image.os_type == "windows"
    assert image.kind == "vm"


def test_the_registry_reference_survives():
    """Naming the OS must not disturb what gets pulled."""
    plain = Image.from_registry(REF)
    windows = Image.from_registry(REF, os_type="windows", kind="vm")
    assert plain._registry == REF
    assert windows._registry == REF


def test_it_offers_the_same_knobs_as_from_file():
    """The two constructors build the same object from different sources; a
    caller should not have to know which one lets them name the guest OS."""
    from_file = set(inspect.signature(Image.from_file).parameters)
    from_registry = set(inspect.signature(Image.from_registry).parameters)
    missing = {"os_type", "kind"} - from_registry
    assert not missing, f"from_registry lacks knobs from_file has: {sorted(missing)}"
    assert "os_type" in from_file  # guards the premise, not the fix


@pytest.mark.parametrize("os_type", ["windows", "linux", "macos"])
def test_os_type_is_passed_through_verbatim(os_type):
    assert Image.from_registry(REF, os_type=os_type).os_type == os_type


def test_windows_registry_image_gets_uefi_on_fleet():
    """The end the bug was actually felt at: firmware selection."""
    from cua_sandbox.transport.fleet_cloud import FleetCloudTransport

    image = Image.from_registry(REF, os_type="windows", kind="vm")
    template = FleetCloudTransport(image=image, name="demo")._template_request().spec.vm_template
    assert template.firmware is not None, "a Windows containerDisk must not be handed BIOS"
