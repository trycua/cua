from cua_sandbox.registry.qemu_builder import apply_windows_accel

BASE = ["qemu", "-machine", "q35", "-cpu", "qemu64,+ssse3,+sse4.1,+sse4.2,+popcnt", "-m", "8192"]


def test_windows_hosts_get_whpx_and_the_host_cpu_by_default():
    out = apply_windows_accel(BASE, {})
    assert out[-2:] == ["-accel", "whpx,kernel-irqchip=off"]
    assert out[out.index("-cpu") + 1] == "host"
    assert BASE[BASE.index("-cpu") + 1].startswith("qemu64"), "the input command is not modified"


def test_software_emulation_can_be_forced_and_keeps_the_cpu_model():
    out = apply_windows_accel(BASE, {"CUA_QEMU_ACCEL": "tcg"})
    assert out[-2:] == ["-accel", "tcg"]
    assert out[out.index("-cpu") + 1].startswith("qemu64")


def test_a_command_without_a_cpu_flag_is_left_alone_apart_from_the_accelerator():
    out = apply_windows_accel(["qemu", "-m", "4096"], {})
    assert "-cpu" not in out and out[-2:] == ["-accel", "whpx,kernel-irqchip=off"]
