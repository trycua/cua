import importlib.util
from pathlib import Path

SPEC = importlib.util.spec_from_file_location(
    "check_pr_screenshots", Path(__file__).resolve().parents[1] / "check_pr_screenshots.py"
)
mod = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(mod)

TEMPLATE = """<!-- ![x](a.png) Desktop mobile instructions -->
| | Before | After |
| Desktop | | |
| Mobile | | |
"""


def test_template_alone_fails():
    assert mod.evaluate(TEMPLATE)[0] is False


def test_two_labelled_images_pass():
    body = "Desktop: ![a](https://github.com/user-attachments/assets/1)\nMobile: ![b](https://github.com/user-attachments/assets/2)"
    assert mod.evaluate(body)[0] is True


def test_img_tags_count():
    body = 'Desktop <img src="x"> Mobile <img src="y">'
    assert mod.evaluate(body)[0] is True


def test_one_size_fails():
    assert mod.evaluate("Desktop ![a](x.png) ![b](y.png)")[0] is False


def test_waiver_needs_reason():
    assert mod.evaluate("Screenshots: not needed - copy-only change in a log line")[0] is True
    assert mod.evaluate("Screenshots: not needed")[0] is False
    assert mod.evaluate("<!-- Screenshots: not needed - example reason here -->")[0] is False
