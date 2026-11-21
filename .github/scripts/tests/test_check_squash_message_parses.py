import importlib.util
import shutil
import tempfile
from pathlib import Path

import pytest

SCRIPT = Path(__file__).resolve().parents[1] / "check_squash_message_parses.py"
spec = importlib.util.spec_from_file_location("check_squash_message_parses", SCRIPT)
mod = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mod)


def test_squash_message_lists_each_commit_as_a_bullet():
    message = mod.squash_message("fix(x): title (#1)", ["fix(x): a\n\nbody", "fix(x): b"])
    assert message == "fix(x): title (#1)\n\n* fix(x): a\n\nbody\n\n* fix(x): b\n"


@pytest.mark.skipif(not (shutil.which("node") and shutil.which("npm")), reason="needs node")
def test_the_unparseable_pr_7637_shape_is_rejected_and_plain_messages_pass():
    with tempfile.TemporaryDirectory() as tmp:
        prefix = Path(tmp)
        try:
            mod.install_parser(prefix)
        except Exception:
            pytest.skip("npm registry unavailable")
        bad = mod.squash_message(
            "fix(a): t (#1)",
            ['fix(a): c\n\n`Perform(cell, "Name:Pin\\nSelector:(null)")`'],
        )
        good = mod.squash_message("fix(a): t (#1)", ["fix(a): c\n\nplain body"])
        assert mod.parse_error(bad, prefix)
        assert mod.parse_error(good, prefix) is None
