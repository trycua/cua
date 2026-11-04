"""Docs-only pull requests skip `E2E: cua SDK` (e2e_cua_sdk_scope.py).

The suite reads the docs only through fenced code blocks. These fixtures pin
what runs it, what skips it, that every doubt runs it, that the workflow's
`paths:` filter and the script agree, and that `ci-gate` counts a skipped run
as passed.
"""

import glob
import importlib.util
import os
import subprocess
import sys
from pathlib import Path

import pytest
import yaml

ROOT = Path(__file__).resolve().parents[3]
MODULE_PATH = ROOT / ".github/scripts/e2e_cua_sdk_scope.py"
SPEC = importlib.util.spec_from_file_location("e2e_cua_sdk_scope", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
scope = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = scope
SPEC.loader.exec_module(scope)

WORKFLOW = yaml.safe_load((ROOT / ".github/workflows/e2e-cua-sdk.yml").read_text())
GATE = yaml.safe_load((ROOT / ".github/workflows/ci-gate.yml").read_text())

TAGGED = "```python test=\"docs\" id=\"demo\"\nprint(1)\n```"
TAGGED_EDITED = "```python test=\"docs\" id=\"demo\"\nprint(2)\n```"
PLAIN = "```bash\nnpm install\n```"
PLAIN_EDITED = "```bash\nnpm install --save\n```"
GUIDE = "docs/content/docs/cua-sdk/guides/images.mdx"
OTHER = "docs/content/docs/spaces/guides/plans.mdx"


def page(fence: str, prose: str = "Intro.") -> str:
    return f"# Title\n\n{prose}\n\n{fence}\n\nAfter.\n"


def decide(changes, base=None, head=None, **kwargs):
    files = {"base": base or {}, "head": head or {}}
    run, why = scope.decide(changes, lambda side, path: files[side].get(path), **kwargs)
    return run, why


def modified(path, before, after, **kwargs):
    return decide([scope.Change("M", path)], {path: before}, {path: after}, **kwargs)


# --- docs pages: only a change to a fence can change a result -------------------


def test_prose_edit_on_a_tagged_page_skips():
    run, why = modified(GUIDE, page(TAGGED), page(TAGGED, prose="A new sentence."))
    assert not run, why


def test_prose_edit_on_an_untagged_page_skips():
    assert not modified(OTHER, page(PLAIN), page(PLAIN, prose="New."))[0]


def test_edited_untagged_block_on_an_untagged_page_skips():
    assert not modified(OTHER, page(PLAIN), page(PLAIN_EDITED))[0]


def test_edited_tagged_block_runs():
    assert modified(GUIDE, page(TAGGED), page(TAGGED_EDITED))[0]


def test_changed_tag_runs():
    retagged = page(TAGGED.replace('id="demo"', 'id="other"'))
    assert modified(GUIDE, page(TAGGED), retagged)[0]


def test_gaining_a_tag_runs():
    assert modified(OTHER, page(PLAIN), page('```bash test="docs" id="x"\nnpm install\n```'))[0]


def test_losing_every_tag_runs():
    assert modified(GUIDE, page(TAGGED), page(PLAIN))[0]


def test_untagged_block_edit_beside_tagged_blocks_runs():
    # Conservative: it can shift the index of a tagged block that has no id.
    both = TAGGED + "\n\n" + PLAIN
    edited = TAGGED + "\n\n" + PLAIN_EDITED
    assert modified(GUIDE, page(both), page(edited))[0]


def test_generated_banner_on_a_tagged_page_runs():
    banner = "{/* AUTO-GENERATED */}\n"
    assert modified(GUIDE, page(TAGGED), banner + page(TAGGED))[0]


@pytest.mark.parametrize("suffix", [".md", ".mdx"])
def test_both_page_suffixes_are_compared(suffix):
    path = "docs/content/docs/cua-sdk/guides/x" + suffix
    assert modified(path, page(TAGGED), page(TAGGED_EDITED))[0]
    assert not modified(path, page(TAGGED), page(TAGGED, prose="New."))[0]


def test_added_tagged_page_runs():
    assert decide([scope.Change("A", GUIDE)], head={GUIDE: page(TAGGED)})[0]


def test_added_untagged_page_skips():
    assert not decide([scope.Change("A", OTHER)], head={OTHER: page(PLAIN)})[0]


def test_deleted_tagged_page_runs():
    assert decide([scope.Change("D", GUIDE)], base={GUIDE: page(TAGGED)})[0]


def test_deleted_untagged_page_skips():
    assert not decide([scope.Change("D", OTHER)], base={OTHER: page(PLAIN)})[0]


def test_renamed_tagged_page_runs():
    new = "docs/content/docs/cua-sdk/guides/images-2.mdx"
    assert decide([scope.Change("R", new, GUIDE)], {GUIDE: page(TAGGED)}, {new: page(TAGGED)})[0]


def test_renamed_untagged_page_skips():
    new = "docs/content/docs/spaces/guides/plans-2.mdx"
    assert not decide([scope.Change("R", new, OTHER)], {OTHER: page(PLAIN)}, {new: page(PLAIN)})[0]


def test_one_tagged_change_among_prose_edits_runs():
    changes = [scope.Change("M", OTHER), scope.Change("M", GUIDE)]
    base = {OTHER: page(PLAIN), GUIDE: page(TAGGED)}
    head = {OTHER: page(PLAIN, prose="New."), GUIDE: page(TAGGED_EDITED)}
    assert decide(changes, base, head)[0]


@pytest.mark.parametrize(
    "path",
    [
        "docs/content/docs/spaces/meta.json",
        "docs/content/docs/redirects.json",
        "docs/content/docs/spaces/img/shot.png",
    ],
)
def test_docs_files_that_are_not_pages_skip(path):
    assert not decide([scope.Change("M", path)])[0]


# --- every other input of the suite runs it -----------------------------------


@pytest.mark.parametrize(
    "path",
    [
        "libs/cua/crates/cua-sdk/src/lib.rs",
        "libs/cua/Cargo.lock",
        "libs/cua/rust-toolchain.toml",
        "libs/cua/typescript/package.json",
        "libs/cua-spacesd/crates/cua-spacesd-server/src/lib.rs",
        "libs/cua-spacesd/Cargo.lock",
        "libs/images/linux/Dockerfile",
        "libs/python/cua-sandbox/cua_sandbox/__init__.py",
        "libs/cua-driver/rust/crates/cua-driver-core/src/lib.rs",
        "libs/cua-driver/rust/crates/cua-driver-core/src/mcp_skills.rs",
        "libs/cua-driver/rust/crates/cua-driver-sdk/Cargo.toml",
        "libs/cua-driver/rust/crates/platform-linux/src/wayland/shell_helper.rs",
        "libs/cua-driver/rust/crates/platform-macos/Cargo.toml",
        "libs/cua-driver/rust/crates/platform-windows/Cargo.toml",
        "libs/cua-driver/rust/Cargo.toml",
        "libs/cua-driver/rust/Skills/cua-driver/SKILL.md",
        "libs/cua-driver/rust/include/cua_driver_abi.h",
        "libs/cua-driver/wayland-helper/winrects@cua/extension.js",
        "tests/e2e/cua-sdk/run.py",
        "tests/e2e/cua-sdk/rust/Cargo.lock",
        "tests/e2e/browse/server.py",
        "docs/code-block-policy.json",
        "scripts/images/install-gvisor.sh",
        ".github/actions/linux-rust-prep/action.yml",
        ".github/actions/setup-protoc/action.yml",
        ".github/actions/rust-cache/action.yml",
        ".github/scripts/e2e_cua_sdk_scope.py",
        ".github/workflows/e2e-cua-sdk.yml",
    ],
)
def test_suite_inputs_run(path):
    assert decide([scope.Change("M", path)])[0]
    # Beside a docs prose edit too.
    changes = [scope.Change("M", OTHER), scope.Change("M", path)]
    assert decide(changes, {OTHER: page(PLAIN)}, {OTHER: page(PLAIN, prose="New.")})[0]


def test_input_renamed_out_of_the_suite_runs():
    assert decide([scope.Change("R", "libs/misc/a.rs", "libs/cua/crates/a.rs")])[0]


@pytest.mark.parametrize("path", ["README.md", "apps/cua-spaces/src/main.rs", "docs/src/app/page.tsx"])
def test_unrelated_files_beside_docs_prose_skip(path):
    changes = [scope.Change("M", OTHER), scope.Change("M", path)]
    run, why = decide(changes, {OTHER: page(PLAIN)}, {OTHER: page(PLAIN, prose="New.")})
    assert not run, why


def test_docs_prose_beside_a_driver_dependency_runs():
    # Before the docs filter was narrowed, the docs page alone started the whole
    # suite for this pull request. The driver crate is an input in its own right,
    # so the mix must not skip it.
    changes = [
        scope.Change("M", OTHER),
        scope.Change("M", "libs/cua-driver/rust/crates/cua-driver-core/src/lib.rs"),
    ]
    run, why = decide(changes, {OTHER: page(PLAIN)}, {OTHER: page(PLAIN, prose="New.")})
    assert run and "cua-driver-core" in why


@pytest.mark.parametrize(
    "path",
    [
        # Driver code the static cua-spacesd build does not compile.
        "libs/cua-driver/rust/crates/cua-driver/src/main.rs",
        "libs/cua-driver/rust/crates/platform-macos/src/lib.rs",
        "libs/cua-driver/rust/crates/platform-windows/src/lib.rs",
        "libs/cua-driver/rust/crates/cua-perception/src/lib.rs",
        "libs/cua-driver/rust/Cargo.lock",
        "libs/cua-driver/typescript/package.json",
    ],
)
def test_driver_code_outside_the_closure_beside_docs_prose_skips(path):
    changes = [scope.Change("M", OTHER), scope.Change("M", path)]
    run, why = decide(changes, {OTHER: page(PLAIN)}, {OTHER: page(PLAIN, prose="New.")})
    assert not run, why


def load_toml(path):
    try:
        import tomllib

        return tomllib.loads(path.read_text(encoding="utf-8"))
    except ModuleNotFoundError:
        return pytest.importorskip("toml").loads(path.read_text(encoding="utf-8"))


def spacesd_path_dependencies():
    """Every crate directory the libs/cua-spacesd workspace reaches through
    `path = ...` dependencies, on every platform."""
    seen = set()

    def walk(directory):
        directory = os.path.normpath(directory)
        if directory in seen:
            return
        seen.add(directory)
        doc = load_toml(Path(directory) / "Cargo.toml")
        tables = [doc.get(k, {}) for k in ("dependencies", "dev-dependencies", "build-dependencies")]
        tables.append(doc.get("workspace", {}).get("dependencies", {}))
        for target in doc.get("target", {}).values():
            tables += [target.get(k, {}) for k in ("dependencies", "dev-dependencies", "build-dependencies")]
        for table in tables:
            for spec in table.values():
                if isinstance(spec, dict) and "path" in spec:
                    walk(os.path.join(directory, spec["path"]))
        for member in doc.get("workspace", {}).get("members", []):
            for found in glob.glob(os.path.join(directory, member)):
                if (Path(found) / "Cargo.toml").exists():
                    walk(found)

    walk(ROOT / "libs" / "cua-spacesd")
    return {os.path.relpath(d, ROOT) for d in seen}


# The cargo lockfile covers these, but a Linux build does not compile them.
NOT_COMPILED_ON_LINUX = {"platform-macos", "platform-windows"}


def test_driver_closure_is_listed():
    crates = {
        d.rsplit("/", 1)[1] for d in spacesd_path_dependencies() if d.startswith("libs/cua-driver/rust/crates/")
    }
    assert {"cua-driver-core", "platform-linux"} <= crates, "the walk found no driver crates"
    for crate in sorted(crates):
        base = f"libs/cua-driver/rust/crates/{crate}"
        assert scope.is_input(f"{base}/Cargo.toml"), crate
        if crate not in NOT_COMPILED_ON_LINUX:
            assert scope.is_input(f"{base}/src/lib.rs"), f"{crate} is built into cua-spacesd but is not an input"


@pytest.mark.parametrize(
    "directory",
    [
        "libs/cua-driver/rust/Skills/cua-driver",
        "libs/cua-driver/rust/include",
        "libs/cua-driver/wayland-helper/winrects@cua",
    ],
)
def test_compile_time_includes_exist(directory):
    # The directories behind the include_str! inputs: a rename must update PATHS.
    assert (ROOT / directory).is_dir()


# --- doubt runs the suite ------------------------------------------------------


@pytest.mark.parametrize("event", ["workflow_dispatch", "schedule", "push"])
def test_other_events_run(event):
    assert modified(GUIDE, page(TAGGED), page(TAGGED, prose="New."), event=event)[0]


def test_release_branch_runs():
    ref = "release-please--branches--main"
    assert modified(GUIDE, page(TAGGED), page(TAGGED, prose="New."), head_ref=ref)[0]


def test_empty_change_list_runs():
    assert decide([])[0]


def test_unreadable_page_runs():
    assert decide([scope.Change("M", GUIDE)], {GUIDE: page(TAGGED)}, {})[0]
    assert decide([scope.Change("M", GUIDE)], {}, {GUIDE: page(TAGGED)})[0]


def test_unknown_status_runs():
    assert decide([scope.Change("U", GUIDE)], {GUIDE: page(TAGGED)}, {GUIDE: page(TAGGED)})[0]


def test_broken_extractor_runs():
    class Broken:
        @staticmethod
        def fences(*_):
            raise RuntimeError("boom")

    run, why = decide([scope.Change("M", GUIDE)], {GUIDE: page(TAGGED)}, {GUIDE: page(TAGGED)}, extract=Broken)
    assert run and "failing closed" in why


# --- the script and the workflow agree ----------------------------------------


def test_script_paths_match_the_workflow_filter():
    filter_paths = WORKFLOW[True]["pull_request"]["paths"]
    assert sorted(filter_paths) == sorted([*scope.PATHS, scope.DOCS_GLOB])


def test_every_path_matches_itself_and_nothing_wider():
    for pattern in scope.PATHS:
        sample = pattern.replace("**", "a/b.txt")
        assert scope.is_input(sample), pattern
    assert not scope.is_input("libs/cua-driver/rust/crates/cua-driver/src/main.rs")
    assert not scope.is_input("libs/cua-driver/rust/crates/platform-macos/src/lib.rs")
    assert not scope.is_input("libs/cua-driver/rust/crates/cua-driver-core-extras/a.rs")
    assert not scope.is_input("libs/cuax/a.rs")
    assert not scope.is_input("tests/e2e/cua-sdk-other/a.py")


def test_workflow_wiring():
    jobs = WORKFLOW["jobs"]
    changes, live = jobs["changes"], jobs["live"]
    # The suite is five build jobs, the test shards that need all of them, and
    # the merge job that needs the shards: skipping the builds skips the rest.
    builds = ["build-sdk", "build-cli", "build-fixtures", "build-rust-tests", "build-images"]
    for name in builds:
        assert jobs[name]["needs"] == "changes", name
        # Only an explicit `false` skips the suite.
        assert "needs.changes.outputs.run != 'false'" in jobs[name]["if"], name
    assert sorted(jobs["shard"]["needs"]) == sorted(builds)
    assert jobs["matrix"]["needs"] == "shard"
    assert "needs" not in jobs["shard"].get("if", "")
    assert changes["outputs"] == {"run": "${{ steps.scope.outputs.run }}"}
    checkout = changes["steps"][0]
    assert checkout["with"]["fetch-depth"] == 2
    assert changes["steps"][1]["env"]["EVENT"] == "${{ github.event_name }}"
    # The nightly lane is not behind the scope job.
    assert "needs" not in live
    assert "github.event_name == 'schedule'" in live["if"] and "needs.changes" not in live["if"]
    assert set(WORKFLOW[True]) == {"pull_request", "schedule", "workflow_dispatch"}


# --- git: the real change list --------------------------------------------------


def git(repo, *args):
    env = {
        **os.environ,
        "GIT_AUTHOR_NAME": "t",
        "GIT_AUTHOR_EMAIL": "t@example.com",
        "GIT_COMMITTER_NAME": "t",
        "GIT_COMMITTER_EMAIL": "t@example.com",
    }
    subprocess.run(["git", "-C", str(repo), *args], check=True, capture_output=True, env=env)


def write(repo, rel, text):
    path = repo / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def scoped(repo, capsys, monkeypatch, event="pull_request"):
    monkeypatch.setenv("EVENT", event)
    monkeypatch.delenv("HEAD_REF", raising=False)
    monkeypatch.delenv("GITHUB_OUTPUT", raising=False)
    assert scope.main(["--repo", str(repo)]) == 0
    return dict(line.split("=", 1) for line in capsys.readouterr().out.splitlines())


@pytest.fixture
def repo(tmp_path):
    git(tmp_path, "init", "-q")
    write(tmp_path, GUIDE, page(TAGGED))
    write(tmp_path, OTHER, page(PLAIN))
    write(tmp_path, "README.md", "readme\n")
    git(tmp_path, "add", "-A")
    git(tmp_path, "commit", "-q", "-m", "base")
    return tmp_path


def commit(repo):
    git(repo, "add", "-A")
    git(repo, "commit", "-q", "-m", "head")


def test_git_prose_edit_skips(repo, capsys, monkeypatch):
    write(repo, GUIDE, page(TAGGED, prose="Rewritten."))
    write(repo, "README.md", "changed\n")
    commit(repo)
    out = scoped(repo, capsys, monkeypatch)
    assert out["run"] == "false", out


def test_git_block_edit_runs(repo, capsys, monkeypatch):
    write(repo, GUIDE, page(TAGGED_EDITED))
    commit(repo)
    assert scoped(repo, capsys, monkeypatch)["run"] == "true"


def test_git_sdk_change_runs(repo, capsys, monkeypatch):
    write(repo, "libs/cua/crates/cua-sdk/src/lib.rs", "// x\n")
    commit(repo)
    assert scoped(repo, capsys, monkeypatch)["run"] == "true"


def test_git_rename_of_a_tagged_page_runs(repo, capsys, monkeypatch):
    git(repo, "mv", GUIDE, GUIDE.replace("images", "pictures"))
    commit(repo)
    assert scoped(repo, capsys, monkeypatch)["run"] == "true"


def test_git_untagged_page_deletion_skips(repo, capsys, monkeypatch):
    git(repo, "rm", "-q", OTHER)
    commit(repo)
    assert scoped(repo, capsys, monkeypatch)["run"] == "false"


def test_git_dispatch_runs(repo, capsys, monkeypatch):
    write(repo, GUIDE, page(TAGGED, prose="Rewritten."))
    commit(repo)
    assert scoped(repo, capsys, monkeypatch, event="workflow_dispatch")["run"] == "true"


def test_git_failure_runs(tmp_path, capsys, monkeypatch):
    # Not a repository: no change list, so the suite runs.
    out = scoped(tmp_path, capsys, monkeypatch)
    assert out["run"] == "true" and "failing closed" in out["reason"]


def test_git_writes_github_output(repo, monkeypatch, tmp_path):
    write(repo, GUIDE, page(TAGGED, prose="Rewritten."))
    commit(repo)
    target = tmp_path / "out.txt"
    monkeypatch.setenv("EVENT", "pull_request")
    monkeypatch.setenv("GITHUB_OUTPUT", str(target))
    assert scope.main(["--repo", str(repo)]) == 0
    assert "run=false\n" in target.read_text()


# --- ci-gate counts a skipped run as passed ------------------------------------

GATE_SCRIPT = next(s["run"] for s in GATE["jobs"]["ci-gate"]["steps"] if s.get("name", "").startswith("Wait for every other workflow"))


def run_gate(rows, jobs):
    mock = """
set -uo pipefail
api() {
  case "$*" in
    *'/jobs?'*) printf '%s' "$JOBS" ;;
    *'head_sha='*) printf '%s' "$ROWS" ;;
    *) printf '999' ;;
  esac
}
sleeps=0
sleep() { sleeps=$((sleeps + 1)); if (( sleeps > 4 )); then exit 124; fi; }
"""
    return subprocess.run(
        ["bash", "-c", mock + GATE_SCRIPT[GATE_SCRIPT.index("sha=$PR_SHA") :]],
        env={
            **os.environ,
            "PR_SHA": "abc",
            "QUEUE_REF": "",
            "GITHUB_REPOSITORY": "test/repo",
            "GITHUB_RUN_ID": "99",
            "ROWS": rows,
            "JOBS": jobs,
        },
        capture_output=True,
        text=True,
        timeout=10,
    )


@pytest.mark.parametrize(
    ("conclusion", "jobs", "verdict"),
    [
        # `local` skipped, `changes` ran: the run succeeded.
        ("success", "E2E: cua SDK / changes\tsuccess", "pass"),
        # Every job skipped (the jq filter drops them): not missing, not a failure.
        ("success", "", "skipped"),
        ("skipped", "", "skipped"),
        # The suite ran and failed: still a failure.
        ("failure", "hermetic + container\tfailure", "fail"),
    ],
)
def test_gate_treats_a_skipped_suite_as_passed(conclusion, jobs, verdict):
    rows = f"7\t70\tE2E: cua SDK\t{conclusion}\tcompleted"
    done = run_gate(rows, jobs)
    assert done.returncode == (1 if verdict == "fail" else 0), done.stderr
    assert f"E2E: cua SDK: {verdict}" in done.stdout


def test_gate_does_not_wait_for_a_workflow_that_never_started():
    # A paths filter that does not match creates no run at all.
    done = run_gate("1\t11\tbuild\tsuccess\tcompleted", "test\tsuccess")
    assert done.returncode == 0
    assert "E2E: cua SDK" not in done.stdout
