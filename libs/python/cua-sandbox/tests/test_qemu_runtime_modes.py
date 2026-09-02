"""QEMURuntime must explain a mode/argument mismatch in its own terms.

The factory defaults to mode="docker", which routes to QEMUDockerRuntime — a
constructor that does not take extra_args. extra_args is how -cpu host is
passed, the documented fix for the commonest local failure in the Minecraft
how-to (the game exiting during resource loading under the default qemu64
model). So QEMURuntime(extra_args=["-cpu", "host"]) used to fail with
"QEMUDockerRuntime.__init__() got an unexpected keyword argument 'extra_args'",
naming a class the caller never mentioned and saying nothing about mode.
"""

import pytest
from cua_sandbox.runtime.qemu import (
    QEMUBaremetalRuntime,
    QEMUDockerRuntime,
    QEMURuntime,
    QEMUWSL2Runtime,
)


def test_extra_args_on_the_default_mode_names_mode_and_the_fix():
    with pytest.raises(TypeError) as excinfo:
        QEMURuntime(extra_args=["-cpu", "host"])

    message = str(excinfo.value)
    assert "extra_args" in message
    # Names the knob that actually selects the backend...
    assert "mode='docker'" in message
    # ...and the modes that can take the argument.
    assert "mode='bare-metal'" in message
    assert "mode='wsl2'" in message
    # ...not an implementation class the caller never mentioned.
    assert "QEMUDockerRuntime" not in message


def test_the_modes_that_build_the_command_line_still_take_extra_args():
    runtime = QEMURuntime(mode="bare-metal", extra_args=["-cpu", "host"])
    assert isinstance(runtime, QEMUBaremetalRuntime)
    assert runtime.extra_args == ["-cpu", "host"]

    runtime = QEMURuntime(mode="wsl2", extra_args=["-cpu", "host"])
    assert isinstance(runtime, QEMUWSL2Runtime)
    assert runtime.extra_args == ["-cpu", "host"]


def test_an_argument_no_mode_accepts_is_reported_without_a_redirect():
    with pytest.raises(TypeError) as excinfo:
        QEMURuntime(mode="bare-metal", definitely_not_a_qemu_option=1)

    message = str(excinfo.value)
    assert "definitely_not_a_qemu_option" in message
    assert "accepted by" not in message


def test_a_misspelled_mode_is_rejected_instead_of_falling_back_to_docker():
    with pytest.raises(ValueError, match="Unknown QEMURuntime mode 'baremetal'"):
        QEMURuntime(mode="baremetal")


def test_the_default_backend_is_unchanged():
    """Every existing caller keeps the runtime it had."""
    assert isinstance(QEMURuntime(), QEMUDockerRuntime)
    assert isinstance(QEMURuntime(mode="docker"), QEMUDockerRuntime)
    assert isinstance(QEMURuntime(mode="bare-metal"), QEMUBaremetalRuntime)
    assert isinstance(QEMURuntime(mode="wsl2"), QEMUWSL2Runtime)


def test_valid_arguments_still_reach_the_selected_runtime():
    runtime = QEMURuntime(mode="docker", api_port=18001, vnc_port=18006, ephemeral=True)
    assert isinstance(runtime, QEMUDockerRuntime)
    assert runtime.api_port == 18001
