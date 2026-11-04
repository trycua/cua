from cua_sandbox.registry.qemu_builder import apply_windows_accel

BASE = ["qemu", "-machine", "q35", "-cpu", "qemu64,+ssse3,+sse4.1,+sse4.2,+popcnt", "-m", "8192"]


def test_software_emulation_is_the_default_and_uses_the_max_cpu_model():
    out = apply_windows_accel(BASE, {})
    assert out[-2:] == ["-accel", "tcg"]
    assert out[out.index("-cpu") + 1] == "max"
    assert BASE[BASE.index("-cpu") + 1].startswith("qemu64"), "the input command is not modified"


def test_whpx_can_be_chosen_and_then_runs_the_host_cpu_model():
    out = apply_windows_accel(BASE, {"CUA_QEMU_ACCEL": "whpx,kernel-irqchip=off"})
    assert out[-2:] == ["-accel", "whpx,kernel-irqchip=off"]
    assert out[out.index("-cpu") + 1] == "host"


def test_a_command_without_a_cpu_flag_only_gains_the_accelerator():
    out = apply_windows_accel(["qemu", "-m", "4096"], {})
    assert "-cpu" not in out and out[-2:] == ["-accel", "tcg"]
