"""The Latest pin moves Latest back when an older Cua Spaces release took it."""

import importlib.util
import json
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "pin_latest_release.py"
spec = importlib.util.spec_from_file_location("pin_latest_release", SCRIPT)
pin = importlib.util.module_from_spec(spec)
spec.loader.exec_module(pin)


def run(monkeypatch, listing, releases):
    calls = []

    def fake_gh(*args):
        calls.append(args)
        if args[:2] == ("release", "list"):
            return json.dumps(listing)
        if args[0] == "api" and "--paginate" in args:
            return json.dumps([releases])
        return ""

    monkeypatch.setattr(pin, "gh", fake_gh)
    assert pin.main() == 0
    return [c for c in calls if "PATCH" in c]


def rel(tag, latest=False, draft=False):
    return {"tagName": tag, "isLatest": latest, "isDraft": draft, "isPrerelease": False}


def api(tag, id_, draft=False):
    return {"tag_name": tag, "id": id_, "draft": draft, "prerelease": False}


def test_older_spaces_release_holding_latest_is_reset(monkeypatch):
    listing = [rel("cua-spaces-v0.37.0", latest=True), rel("cua-spaces-v0.37.1")]
    patches = run(monkeypatch, listing, [api("cua-spaces-v0.37.0", 1), api("cua-spaces-v0.37.1", 2)])
    assert len(patches) == 1 and "releases/2" in patches[0][3]


def test_newest_spaces_release_keeps_latest(monkeypatch):
    listing = [rel("cua-spaces-v0.37.1", latest=True), rel("cua-spaces-v0.37.0")]
    assert run(monkeypatch, listing, []) == []


def test_drafts_never_win(monkeypatch):
    listing = [rel("cua-spaces-v0.37.2", draft=True), rel("cua-spaces-v0.37.1", latest=True)]
    assert run(monkeypatch, listing, []) == []
