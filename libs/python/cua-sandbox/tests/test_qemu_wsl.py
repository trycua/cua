"""The WSL2-hosted QEMU runtime has to translate Windows paths for the Linux QEMU."""

import json

from cua_sandbox.runtime.qemu import QEMUWSL2Runtime

OVERLAY = "/mnt/c/Users/demo/.cua/cua-sandbox/images/sessions/demo.qcow2"
WINDOWS_BACKING = r"C:\Users\demo\.cua\cua-sandbox\images\container-disks\abc\disk.qcow2"
WSL_BACKING = "/mnt/c/Users/demo/.cua/cua-sandbox/images/container-disks/abc/disk.qcow2"


class _Runtime(QEMUWSL2Runtime):
    """Records the commands that would have run inside WSL."""

    def __init__(self, info, *, fail=False):
        super().__init__()
        self.commands = []
        self._info = info
        self._fail = fail

    def _wsl(self, cmd, timeout=30):
        self.commands.append(cmd)
        if cmd.startswith("qemu-img info"):
            if self._fail:
                raise RuntimeError("qemu-img: command not found")
            return json.dumps(self._info)
        return ""


def _rebases(runtime):
    return [c for c in runtime.commands if c.startswith("qemu-img rebase")]


def test_a_windows_backing_path_is_rewritten_for_qemu_inside_wsl():
    """Otherwise QEMU reads the drive letter as a URI scheme: Unknown protocol 'C'."""
    runtime = _Runtime({"backing-filename": WINDOWS_BACKING})

    runtime._rebase_backing_file(OVERLAY)

    assert _rebases(runtime) == [
        f"qemu-img rebase -u -f qcow2 -F qcow2 -b '{WSL_BACKING}' '{OVERLAY}'"
    ]


def test_the_rebase_is_metadata_only():
    """-u must be present: without it qemu-img rewrites gigabytes of guest data."""
    runtime = _Runtime({"backing-filename": WINDOWS_BACKING})

    runtime._rebase_backing_file(OVERLAY)

    assert " -u " in _rebases(runtime)[0]


def test_a_backing_path_already_in_wsl_form_is_left_alone():
    runtime = _Runtime({"backing-filename": WSL_BACKING})

    runtime._rebase_backing_file(OVERLAY)

    assert _rebases(runtime) == []


def test_a_disk_with_no_backing_file_is_left_alone():
    runtime = _Runtime({"format": "qcow2"})

    runtime._rebase_backing_file(OVERLAY)

    assert _rebases(runtime) == []


def test_an_uninspectable_disk_does_not_break_the_launch():
    runtime = _Runtime({}, fail=True)

    runtime._rebase_backing_file(OVERLAY)

    assert _rebases(runtime) == []
