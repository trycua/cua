"""The neutral ``computer/v1`` operations the three arm drivers share, with a fake in place of the Mac."""

from __future__ import annotations

import pytest
from agentenv_protocol.computer import Unsupported
from agentenv_protocol.computer.platform import Platform
from conftest import DRIVERS, load

NAMES = ("agent-desktop", "macos-harness", "codex-cu")


def test_the_helper_is_the_same_file_in_each_drivers_folder():
    copies = {(DRIVERS / name / "macos_computer.py").read_bytes() for name in NAMES}
    assert len(copies) == 1


@pytest.fixture(scope="module")
def mc():
    return load(DRIVERS / "agent-desktop", "macos_computer.py")


class FakeBackend:
    def __init__(self):
        self.calls = []

    def screen(self):
        return 1920, 1080, 2.0

    def grab(self):
        return 3840, 2160, b"\x89PNG"

    def __getattr__(self, name):
        def record(*args):
            self.calls.append((name, *args))

        return record


@pytest.fixture
def driver(mc):
    from agentenv_protocol.computer import computer_driver

    @computer_driver(name="fake", supports=[{"os": {"type": "macos"}, "arch": ["arm64"]}])
    class Fake(mc.MacComputerDriver):
        backend_factory = FakeBackend

    return Fake()


def test_it_serves_the_seven_computer_tools_on_apple_silicon(mc):
    from agentenv_protocol.computer import computer_driver

    @computer_driver(name="fake", supports=[{"os": {"type": "macos"}, "arch": ["arm64"]}])
    class Fake(mc.MacComputerDriver):
        pass

    card = Fake.declaration(Platform(os="macos", arch="arm64"))
    assert sorted(t.name for t in card.capabilities.tools) == [
        "computer_click",
        "computer_drag",
        "computer_key",
        "computer_screenshot",
        "computer_scroll",
        "computer_type",
        "computer_wait",
    ]


@pytest.mark.parametrize(
    "key, code, flags",
    [
        ("a", 0, 0),
        ("A", 0, 0x20000),
        ("?", 44, 0x20000),
        ("Enter", 36, 0),
        ("ArrowUp", 126, 0),
        ("\\", 42, 0),
    ],
)
def test_a_key_resolves_to_its_us_keyboard_code(mc, key, code, flags):
    assert mc.resolve_key(key) == (code, flags)


def test_a_chord_holds_its_modifiers_over_the_last_key(mc):
    assert mc.resolve_chord(["Meta", "c"]) == (8, mc.FLAG_COMMAND)
    assert mc.resolve_chord(["Control", "Alt", "Delete"]) == (117, mc.FLAG_CONTROL | mc.FLAG_OPTION)


def test_a_chord_that_isnt_one_is_refused(mc):
    with pytest.raises(Unsupported):
        mc.resolve_chord(["a", "b"])  # a isn't a modifier
    with pytest.raises(Unsupported):
        mc.resolve_chord(["Shift", "Meta"])  # ends in a modifier
    with pytest.raises(Unsupported):
        mc.resolve_key("Eject")


async def test_the_display_and_a_capture_come_from_the_backend(driver):
    display = await driver.screen()
    image = await driver.grab()
    assert (display.width, display.height, display.scale) == (1920, 1080, 2.0)
    assert (image.width, image.height, image.mode) == (3840, 2160, "png")


async def test_input_reaches_the_backend_with_macos_buttons_and_flags(driver, mc):
    await driver.click_at(10, 20, "right", 2, ["Meta"])
    await driver.scroll_at(5, 6, "down", 3, [])
    await driver.type_in("hi")
    await driver.press_keys(["Meta", "Shift", "A"])
    assert driver.mac().calls == [
        ("click", 10, 20, 1, 2, mc.FLAG_COMMAND),
        ("scroll", 5, 6, 0, -3, 0),
        ("type_text", "hi"),
        ("key", 0, mc.FLAG_COMMAND | mc.FLAG_SHIFT),
    ]


async def test_a_failed_post_is_a_failed_delivery(mc):
    from agentenv_protocol.computer import DeliveryFailed, computer_driver

    class Broken(FakeBackend):
        def click(self, *args):
            raise RuntimeError("no event tap")

    @computer_driver(name="broken", supports=[{"os": {"type": "macos"}, "arch": ["arm64"]}])
    class Fake(mc.MacComputerDriver):
        backend_factory = Broken

    with pytest.raises(DeliveryFailed):
        await Fake().click_at(1, 1, "left", 1, [])
