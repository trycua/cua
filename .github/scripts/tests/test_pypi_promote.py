"""PyPI gets Cua Driver and cua SDK wheels only through pypi-promote.yml."""

import datetime as dt
import hashlib
import json
import sys
import zipfile
from pathlib import Path
from types import SimpleNamespace

import yaml

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / ".github/scripts"))

import pypi_promote as promote  # noqa: E402

WORKFLOWS = ROOT / ".github/workflows"


def test_only_the_promotion_workflow_uploads_to_pypi():
    uploaders = []
    for wf in sorted(WORKFLOWS.glob("*.yml")):
        text = wf.read_text()
        if "secrets.PYPI_TOKEN" in text and "twine upload" in text:
            uploaders.append(wf.name)
    # cd-py-sandbox / py-reusable-publish ship sub-MB pure-Python packages on
    # each stable release; the native wheels go through the promotion.
    assert "pypi-promote.yml" in uploaders
    assert "cd-py-cua-driver.yml" not in uploaders
    assert "cd-cua-sdk.yml" not in uploaders


def test_builds_still_attach_wheels_to_the_github_release():
    driver = (WORKFLOWS / "cd-py-cua-driver.yml").read_text()
    assert "attach-wheels:" in driver and "python-sdk-checksums.txt" in driver
    assert 'gh release upload "${{ needs.get-version.outputs.tag }}" dist/*' in driver
    sdk = (WORKFLOWS / "cd-cua-sdk.yml").read_text()
    assert "cp dist/wheel-*/*.whl" in sdk


def test_promotion_triggers_and_upload_guard():
    wf = yaml.safe_load((WORKFLOWS / "pypi-promote.yml").read_text())
    on = wf.get("on", wf.get(True))
    assert on["schedule"] == [{"cron": "0 15 * * 1"}]
    assert on["release"] == {"types": ["released"]}
    assert set(on["workflow_dispatch"]["inputs"]) == {"component", "version", "require_green", "dry_run"}
    steps = {s.get("name"): s for s in wf["jobs"]["promote"]["steps"]}
    upload = steps["Upload to PyPI"]
    assert "steps.plan.outputs.action == 'promote'" in upload["if"]
    assert "steps.scope.outputs.upload == 'true'" in upload["if"]
    assert 'if [ "$CANONICAL" != true ]' in upload["run"]
    assert "exit 1" in steps["Fail when PyPI has fallen behind"]["run"]
    scope = steps["Decide whether this component runs"]["run"]
    assert '"$SENDER_TYPE" == User' in scope
    assert steps["Decide whether this component runs"]["env"]["SCHEDULE"] == "${{ vars.PYPI_PROMOTE_SCHEDULE }}"


def test_wheel_sets():
    driver = promote.COMPONENTS["cua-driver"]
    names = {f"cua_driver-1.2.3-{t}.whl" for t in promote.DRIVER_TAGS}
    assert promote.wheel_names(driver, "1.2.3", names) is not None
    assert promote.wheel_names(driver, "1.2.3", names - {next(iter(names))}) is None
    sdk = promote.COMPONENTS["cua-sdk"]
    cua = {f"cua-1.2.3-{t}.whl" for t in promote.SDK_TAGS}
    fat = {f"cua_cli-1.2.3-{t}.whl" for t in promote.SDK_TAGS}
    order = promote.wheel_names(sdk, "1.2.3", cua | fat)
    assert order[0].startswith("cua_cli-") and order[-1].startswith("cua-")
    thin = promote.wheel_names(sdk, "1.2.3", cua | {"cua_cli-1.2.3-py3-none-any.whl"})
    assert thin[0] == "cua_cli-1.2.3-py3-none-any.whl" and len(thin) == 7
    assert promote.wheel_names(sdk, "1.2.3", cua) is None


def test_parse_checksums_accepts_both_layouts():
    a, b = "a" * 64, "b" * 64
    sums = promote.parse_checksums(f"{a}  dist/x.whl\n{b}  y.whl\n# note\n")
    assert sums == {"x.whl": a, "y.whl": b}


def test_thin_cli_manifest_must_match_release_checksums(tmp_path):
    whl = tmp_path / "cua_cli-1.2.3-py3-none-any.whl"
    manifest = {"assets": {"linux-x64": {"name": "cua-cli-1.2.3-linux-x64.tar.gz", "sha256": "c" * 64}}}
    with zipfile.ZipFile(whl, "w") as z:
        z.writestr("cua_cli/_release.json", json.dumps(manifest))
    promote.verify_thin_cli(whl, {"cua-cli-1.2.3-linux-x64.tar.gz": "c" * 64})
    try:
        promote.verify_thin_cli(whl, {"cua-cli-1.2.3-linux-x64.tar.gz": "d" * 64})
    except SystemExit as e:
        assert "does not match" in str(e)
    else:
        raise AssertionError("mismatch accepted")


def fake(monkeypatch, releases, pypi_versions, uploaded="2026-11-01T00:00:00Z", bad=()):
    rels = [
        {"tagName": f"cua-driver-rs-v{v}", "version": v, "isDraft": False, "isPrerelease": True, "publishedAt": p}
        for v, p in releases
    ]
    monkeypatch.setattr(promote, "releases", lambda comp: sorted(rels, key=lambda r: promote.vkey(r["version"]), reverse=True))
    monkeypatch.setattr(promote, "promotable", lambda comp, r, require_green: "red" if r["version"] in bad else None)
    newest = max(pypi_versions, key=promote.vkey) if pypi_versions else None
    monkeypatch.setattr(promote, "pypi_state", lambda comp: (newest, promote.ts(uploaded) if newest else None, set(pypi_versions)))


def args(**kw):
    base = dict(component="cua-driver", version="", min_age_hours=24, max_stale_days=14, require_green=True)
    return SimpleNamespace(**{**base, **kw})


def iso(days_ago):
    return (dt.datetime.now(dt.timezone.utc) - dt.timedelta(days=days_ago)).isoformat()


def test_weekly_promotes_the_newest_build_past_the_soak(monkeypatch):
    fake(monkeypatch, [("0.48.2", iso(0.2)), ("0.48.1", iso(2)), ("0.48.0", iso(3))], ["0.47.19"], uploaded=iso(5))
    out = promote.plan(args())
    assert (out["action"], out["version"], out["stale"]) == ("promote", "0.48.1", "false")


def test_red_builds_are_skipped(monkeypatch):
    fake(monkeypatch, [("0.48.1", iso(2)), ("0.48.0", iso(3))], ["0.47.19"], uploaded=iso(5), bad={"0.48.1"})
    assert promote.plan(args())["version"] == "0.48.0"


def test_already_on_pypi_is_a_skip(monkeypatch):
    fake(monkeypatch, [("0.48.0", iso(3))], ["0.48.0"])
    assert promote.plan(args())["action"] == "skip"


def test_stale_when_pypi_lags_more_than_14_days(monkeypatch):
    fake(monkeypatch, [("0.49.0", iso(2))], ["0.47.19"], uploaded=iso(30))
    out = promote.plan(args())
    assert out["action"] == "promote" and out["stale"] == "true"
    # Nothing promotable for weeks is stale too.
    fake(monkeypatch, [("0.49.0", iso(2))], ["0.47.19"], uploaded=iso(30), bad={"0.49.0"})
    out = promote.plan(args())
    assert out["action"] == "none" and out["stale"] == "true"


def test_manual_version_ignores_the_soak(monkeypatch):
    fake(monkeypatch, [("0.48.2", iso(0.1))], ["0.47.19"])
    out = promote.plan(args(version="0.48.2", min_age_hours=0))
    assert (out["action"], out["version"]) == ("promote", "0.48.2")


def test_sha256_helper(tmp_path):
    f = tmp_path / "x"
    f.write_bytes(b"cua")
    assert promote.sha256(f) == hashlib.sha256(b"cua").hexdigest()
