"""cua SDK release PRs regenerate every file that records the SDK version
(.github/scripts/sync_sdk_release_files.py, run by
release-sync-generated-files.yml), so merging a release leaves no drift."""

from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
from pathlib import Path

import pytest
import yaml

ROOT = Path(__file__).resolve().parents[3]
SCRIPT = ROOT / ".github/scripts/sync_sdk_release_files.py"
WORKFLOW = ROOT / ".github/workflows/release-sync-generated-files.yml"

spec = importlib.util.spec_from_file_location("sync_sdk_release_files", SCRIPT)
sync = importlib.util.module_from_spec(spec)
sys.modules["sync_sdk_release_files"] = sync
spec.loader.exec_module(sync)


def git(root: Path, *args: str) -> None:
    subprocess.run(["git", *args], cwd=root, check=True, capture_output=True)


@pytest.fixture
def repo(tmp_path: Path) -> Path:
    files = {
        "libs/cua/VERSION": "0.4.0\n",
        "docs/content/docs/fleets/quickstart.mdx": "npm install @trycua/cua@0.3.1 && npm install tsx\n",
        "docs/content/docs/fleets/guides/images.mdx": "This uses the `cua` 0.3.1 CLI.\nA provider newer than 0.3.0.\n",
        # The checkout, editable: follows.
        "libs/python/a/uv.lock": '[[package]]\nname = "cua"\nversion = "0.3.1"\nsource = { editable = "../../cua/python" }\n',
        # A registry release of cua: left alone.
        "libs/python/b/uv.lock": '[[package]]\nname = "cua"\nversion = "0.2.0"\nsource = { registry = "https://pypi.org/simple" }\n',
        # Another package whose name starts with cua: left alone.
        "libs/python/c/uv.lock": '[[package]]\nname = "cua-sandbox"\nversion = "0.9.0"\nsource = { editable = "." }\n',
        "libs/cua/typescript/package-lock.json": json.dumps({"packages": {"": {"version": "0.3.1"}}}),
        "samples/ts/package-lock.json": json.dumps(
            {"packages": {"": {}, "../../libs/cua/typescript": {"version": "0.3.1"}, "node_modules/x": {}}}
        ),
        "samples/other/package-lock.json": json.dumps({"packages": {"": {}, "node_modules/@trycua/cua": {}}}),
    }
    for rel, content in files.items():
        path = tmp_path / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)
    git(tmp_path, "init", "-q")
    git(tmp_path, "add", "-A")
    return tmp_path


def test_hand_facts_follow_the_version_and_nothing_else(repo: Path) -> None:
    changed = sync.sync_hand_facts(repo, "0.4.0")
    assert changed == ["docs/content/docs/fleets/quickstart.mdx", "docs/content/docs/fleets/guides/images.mdx"]
    assert "@trycua/cua@0.4.0 " in (repo / "docs/content/docs/fleets/quickstart.mdx").read_text()
    images = (repo / "docs/content/docs/fleets/guides/images.mdx").read_text()
    assert "`cua` 0.4.0 CLI" in images
    assert "newer than 0.3.0" in images  # a provider version, not the SDK's
    assert sync.sync_hand_facts(repo, "0.4.0") == []  # idempotent


def test_a_missing_hand_fact_fails_loudly(repo: Path) -> None:
    (repo / "docs/content/docs/fleets/guides/images.mdx").write_text("reworded\n")
    with pytest.raises(RuntimeError, match="no cua version fact"):
        sync.sync_hand_facts(repo, "0.4.0")


def test_only_locks_on_the_checkout_are_selected(repo: Path) -> None:
    assert sync.uv_locks_on_the_checkout(repo) == ["libs/python/a/uv.lock"]
    # The SDK's own lock is Release Please's; registry installs are not linked.
    assert sync.npm_locks_linking_the_sdk(repo) == ["samples/ts/package-lock.json"]


def test_hand_facts_match_what_the_docs_test_checks() -> None:
    facts = (ROOT / "docs/scripts/tests/test_fleet_docs_version_facts.py").read_text()
    for page, _, _ in sync.HAND_FACTS:
        assert f'"{page}"' in facts
    # Every hand fact exists today (the regex still matches the page).
    version = sync.sdk_version(ROOT)
    for page, pattern, template in sync.HAND_FACTS:
        text = (ROOT / "docs/content/docs" / page).read_text()
        assert template.format(version=version) in text, page


def test_generators_are_configured_and_checked_by_check_docs() -> None:
    config = json.loads((ROOT / "scripts/docs-generators/config.json").read_text())["generators"]
    check_docs = (ROOT / ".github/workflows/ci-check-docs.yml").read_text()
    for generator in sync.GENERATORS:
        assert config[generator]["enabled"], generator
        assert f'"{generator}"' in check_docs, generator


def workflow() -> dict:
    return yaml.safe_load(WORKFLOW.read_text())


def test_runs_on_release_branches_only() -> None:
    wf = workflow()
    on = wf[True] if True in wf else wf["on"]
    assert list(on) == ["push"]
    assert on["push"]["branches"] == ["release-please--branches--main--components--*"]
    assert wf["permissions"] == {"contents": "read"}
    assert wf["concurrency"]["cancel-in-progress"] is True
    job = wf["jobs"]["cua-sdk"]
    assert "refs/heads/release-please--branches--main--components--cua-sdk" in job["if"]


def test_never_loops_on_its_own_commit() -> None:
    wf = workflow()
    assert f"!startsWith(github.event.head_commit.message, '{wf['env']['COMMIT_SUBJECT']}')" in wf["jobs"]["cua-sdk"]["if"]


def test_regeneration_holds_no_credentials() -> None:
    steps = workflow()["jobs"]["cua-sdk"]["steps"]
    names = [s.get("name", s.get("uses", "")) for s in steps]
    checkout = next(s for s in steps if s.get("uses", "").startswith("actions/checkout"))
    assert checkout["with"]["persist-credentials"] is False
    token = names.index("Generate release GitHub App token")
    regen = names.index("Regenerate the files that record the cua SDK version")
    assert regen < token, "the App token is minted only after the regeneration"
    assert steps[token]["with"]["app-id"] == "${{ secrets.RELEASE_APP_ID }}"
    assert steps[token]["with"]["private-key"] == "${{ secrets.RELEASE_APP_PRIVATE_KEY }}"
    assert "python3 .github/scripts/sync_sdk_release_files.py" in steps[regen]["run"]
    assert workflow()["jobs"]["cua-sdk"]["env"]["NPM_CONFIG_IGNORE_SCRIPTS"] == "true"
    assert "--ignore-scripts" in next(s["run"] for s in steps if s.get("name", "").startswith("Install docs dependencies"))


def test_commits_only_when_something_changed() -> None:
    steps = workflow()["jobs"]["cua-sdk"]["steps"]
    stage = next(s for s in steps if s.get("id") == "stage")
    assert "git diff --cached --quiet" in stage["run"]
    for s in steps:
        if s.get("name") in ("Generate release GitHub App token", "Commit and push to the release branch"):
            assert s["if"] == "steps.stage.outputs.changed == 'true'"
    push = next(s for s in steps if s.get("name") == "Commit and push to the release branch")["run"]
    assert "--force" not in push
