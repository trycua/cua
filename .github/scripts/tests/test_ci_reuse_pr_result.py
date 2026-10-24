"""Tests for ci_reuse_pr_result.py and its wiring in the three workflows that use it."""

import json
import re
import sys
from pathlib import Path

import pytest
import yaml

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import ci_reuse_pr_result as reuse  # noqa: E402

ROOT = Path(__file__).resolve().parents[3]
WORKFLOWS = ROOT / ".github/workflows"

REPO = "trycua/cua"
SHA = "a" * 40
HEAD = "b" * 40
TREE = "c" * 40
OTHER_TREE = "d" * 40
STAMP_JOB = "contract clients / changes"
PUSH = {
    "GITHUB_EVENT_NAME": "push",
    "GITHUB_REF": "refs/heads/main",
    "GITHUB_SHA": SHA,
    "GITHUB_REPOSITORY": REPO,
    "GITHUB_RUN_ID": "900",
}


def world(*, tree=TREE, parents=1, prs=None, runs=None, jobs=None, notes=None):
    """A fake GitHub: path -> response for the calls find_reusable makes."""
    prs = [{"number": 7, "merge_commit_sha": SHA, "merged_at": "t", "base": {"ref": "main"}, "head": {"sha": HEAD}}] if prs is None else prs
    runs = [{"id": 11, "created_at": "2026-10-24T01:00:00Z"}] if runs is None else runs
    jobs = {
        11: [
            {"id": 1, "name": STAMP_JOB, "conclusion": "success"},
            {"id": 2, "name": "Portable contract parity (ubuntu-latest)", "conclusion": "success"},
            {"id": 3, "name": "Portable contract parity (macos-latest)", "conclusion": "skipped"},
            {"id": 4, "name": "Generated contract and SDK bindings", "conclusion": "success"},
        ]
    } if jobs is None else jobs
    notes = {1: [{"message": f"ci-reuse tree={TREE}"}]} if notes is None else notes
    calls = []

    def api(path):
        calls.append(path)
        if path == f"repos/{REPO}/commits/{SHA}":
            return {"parents": [{"sha": "p"}] * parents}
        if path == f"repos/{REPO}/commits/{SHA}/pulls":
            return prs
        if path == f"repos/{REPO}/actions/runs/900":
            return {"workflow_id": 55}
        if path.startswith(f"repos/{REPO}/actions/workflows/55/runs?"):
            assert f"head_sha={HEAD}" in path and "event=pull_request" in path and "status=success" in path
            return {"workflow_runs": runs}
        m = re.fullmatch(rf"repos/{REPO}/check-runs/(\d+)/annotations", path)
        if m:
            return notes.get(int(m.group(1)), [])
        raise AssertionError(f"unexpected api call {path}")

    def jobs_of(path):
        m = re.fullmatch(rf"repos/{REPO}/actions/runs/(\d+)/jobs\?filter=latest&per_page=100", path)
        assert m, path
        return jobs[int(m.group(1))]

    return api, jobs_of, calls


def decide(env=None, subsets=None, tree=TREE, **kw):
    api, jobs_of, calls = world(**kw)
    out = reuse.decide(
        {**PUSH, **(env or {})}, subsets or {}, STAMP_JOB, api=api, jobs_of=jobs_of, tree_fn=lambda sha: tree
    )
    return (*out, calls)


def test_reuses_the_jobs_that_passed_on_the_same_tree():
    reused, flags, why, note, _ = decide(subsets={"all": ["Portable contract parity (ubuntu-latest)", "Generated contract and SDK bindings"]})
    assert reused == [STAMP_JOB, "Portable contract parity (ubuntu-latest)", "Generated contract and SDK bindings"]
    assert flags == {"all": True}
    assert "PR #7" in why and note is None


def test_skipped_jobs_are_never_reused():
    reused, flags, *_ = decide(subsets={"mac": ["Portable contract parity (macos-latest)"]})
    assert "Portable contract parity (macos-latest)" not in reused
    assert flags == {"mac": False}


def test_subset_is_all_or_nothing():
    reused, flags, *_ = decide(subsets={"s": ["Generated contract and SDK bindings", "never ran on the PR"]})
    assert "Generated contract and SDK bindings" in reused
    assert flags == {"s": False}


def test_empty_subset_is_never_true():
    assert decide(subsets={"s": []})[1] == {"s": False}


def test_a_different_tree_reuses_nothing():
    # Main moved between the pull request run and the merge: the squash commit
    # is a tree nobody tested, so every job runs.
    reused, flags, why, *_ = decide(tree=OTHER_TREE, subsets={"s": ["Generated contract and SDK bindings"]})
    assert reused == [] and flags == {"s": False}
    assert "different tree" in why


def test_unstamped_run_reuses_nothing():
    reused, _, why, *_ = decide(notes={})
    assert reused == [] and "no tree stamp" in why
    assert decide(notes={1: [{"message": "something else"}]})[0] == []
    assert decide(jobs={11: [{"id": 2, "name": "Generated contract and SDK bindings", "conclusion": "success"}]})[0] == []


def test_stamp_on_another_job_is_ignored():
    assert decide(notes={4: [{"message": f"ci-reuse tree={TREE}"}]})[0] == []


def test_no_green_run_reuses_nothing():
    reused, _, why, *_ = decide(runs=[])
    assert reused == [] and "no successful run" in why


def test_only_the_squash_commit_of_one_merged_pr_counts():
    base = {"number": 7, "merged_at": "t", "base": {"ref": "main"}, "head": {"sha": HEAD}}
    assert decide(prs=[])[0] == []
    assert decide(prs=[{**base, "merge_commit_sha": "e" * 40}])[0] == []  # not the commit pushed
    assert decide(prs=[{**base, "merge_commit_sha": SHA, "merged_at": None}])[0] == []
    assert decide(prs=[{**base, "merge_commit_sha": SHA, "base": {"ref": "release"}}])[0] == []
    two = [{**base, "merge_commit_sha": SHA}, {**base, "number": 8, "merge_commit_sha": SHA}]
    assert decide(prs=two)[0] == []


@pytest.mark.parametrize("parents", [0, 2])
def test_merge_commits_and_root_commits_reuse_nothing(parents):
    assert decide(parents=parents)[0] == []


def test_a_lookup_error_runs_everything():
    def boom(path):
        raise RuntimeError("rate limited")

    reused, flags, why, *_ = reuse.decide(PUSH, {"s": ["x"]}, STAMP_JOB, api=boom, jobs_of=None, tree_fn=lambda s: TREE)
    assert reused == [] and flags == {"s": False} and "running everything" in why


def test_runs_that_are_not_a_push_to_main_are_never_reduced():
    for env in (
        {"GITHUB_EVENT_NAME": "schedule"},
        {"GITHUB_EVENT_NAME": "workflow_dispatch"},
        {"GITHUB_REF": "refs/heads/feature"},
    ):
        _, _, _, _, calls = decide(env=env)
        assert decide(env=env)[0] == [] and calls == []


def test_pull_request_run_writes_a_stamp():
    reused, flags, _, note, calls = decide(env={"GITHUB_EVENT_NAME": "pull_request", "GITHUB_REF": "refs/pull/7/merge"}, subsets={"s": ["x"]})
    assert reused == [] and flags == {"s": False} and calls == []
    assert note == f"::notice title=ci-reuse::ci-reuse tree={TREE}"
    assert reuse.STAMP_RE.match(note.split("::")[-1])


def test_modes():
    subsets = {"s": ["Generated contract and SDK bindings"]}
    reused, flags, why, *_ = decide(env={"REUSE_MODE": "shadow"}, subsets=subsets)
    assert reused == [] and flags == {"s": False} and why.startswith("shadow: would reuse 3 jobs")
    off = decide(env={"REUSE_MODE": "off"}, subsets=subsets)
    assert off[0] == [] and off[4] == []
    stamp_off = decide(env={"REUSE_MODE": "off", "GITHUB_EVENT_NAME": "pull_request"})
    assert stamp_off[3] is None
    assert decide(env={"REUSE_MODE": "ON"}, subsets=subsets)[1] == {"s": True}
    assert decide(env={"REUSE_MODE": ""}, subsets=subsets)[1] == {"s": True}


def test_newest_matching_runs_all_contribute():
    runs = [{"id": 11, "created_at": "2026-10-24T01:00:00Z"}, {"id": 12, "created_at": "2026-10-24T02:00:00Z"}]
    jobs = {
        11: [{"id": 1, "name": STAMP_JOB, "conclusion": "success"}, {"id": 2, "name": "a", "conclusion": "success"}],
        12: [{"id": 5, "name": STAMP_JOB, "conclusion": "success"}, {"id": 6, "name": "b", "conclusion": "success"}],
    }
    notes = {1: [{"message": f"ci-reuse tree={OTHER_TREE}"}], 5: [{"message": f"ci-reuse tree={TREE}"}]}
    reused = decide(runs=runs, jobs=jobs, notes=notes)[0]
    assert reused == [STAMP_JOB, "b"]  # run 11 tested another tree


def test_remaining_legs():
    reused = ["fmt, clippy, test (ubuntu-latest)", "relay e2e (docker)"]
    legs = ["ubuntu-latest", "macos-26", "windows-latest"]
    assert reuse.remaining_legs(legs, "fmt, clippy, test ({os})", reused) == ["macos-26", "windows-latest"]
    assert reuse.remaining_legs(legs, "other ({os})", reused) == legs
    assert reuse.remaining_legs(["ubuntu-latest"], "fmt, clippy, test ({os})", reused) == []


def test_cli_writes_outputs_and_legs(tmp_path, monkeypatch, capsys):
    out = tmp_path / "out"
    monkeypatch.setenv("GITHUB_OUTPUT", str(out))
    monkeypatch.setenv("GITHUB_STEP_SUMMARY", str(tmp_path / "sum"))
    monkeypatch.setenv("REUSED", json.dumps(["t (ubuntu-latest)"]))
    monkeypatch.setenv("LEGS", json.dumps(["ubuntu-latest", "macos-26"]))
    assert reuse.main(["legs", "--template", "t ({os})", "--output", "legs"]) == 0
    assert out.read_text() == 'legs=["macos-26"]\n'
    out.write_text("")
    for var in PUSH:
        monkeypatch.delenv(var, raising=False)
    monkeypatch.setenv("GITHUB_EVENT_NAME", "schedule")
    assert reuse.main(["run", "--stamp-job", "x", "--subset", "s=a|b"]) == 0
    assert out.read_text().splitlines() == ["reused=[]", "s=false"]


# ---- wiring in the workflows --------------------------------------------------


def load(name):
    return yaml.safe_load((WORKFLOWS / name).read_text())


def job_names(workflow, expand_os=()):
    """Every job name the workflow can produce (matrix.os expanded)."""
    names = set()
    for job in workflow["jobs"].values():
        name = job.get("name")
        if not name:
            continue
        if "matrix.os" in name:
            for os_ in expand_os:
                names.add(name.replace("${{ matrix.os }}", os_))
        else:
            names.add(name)
    return names


def reuse_run_command(workflow):
    steps = workflow["jobs"]["reuse"]["steps"]
    return next(s["run"] for s in steps if s.get("id") == "reuse")


def test_cua_sdk_subset_names_are_real_jobs_with_the_pull_request_legs():
    wf = load("ci-cua-sdk.yml")
    command = reuse_run_command(wf)
    names = re.search(r'--subset "pr_subset=([^"]+)"', command).group(1).split("|")
    assert len(names) == len(set(names))
    assert '--stamp-job "reuse PR results"' in command
    assert wf["jobs"]["reuse"]["name"] == "reuse PR results"
    assert set(names) <= job_names(wf, expand_os=("ubuntu-latest", "windows-latest"))


def literals(expr):
    return [json.loads(x) for x in re.findall(r"'(\[[^']*\])'", expr)]


MATRIX_JOBS = {
    # job id: legs a pull request runs and which of them are in the reused subset
    "rust-lint": ["ubuntu-latest", "windows-latest"],
    "rust": ["ubuntu-latest"],
    "spaces-e2e": ["ubuntu-latest"],
    "bindings": ["ubuntu-latest"],
    "python": ["ubuntu-latest"],
    "node": ["ubuntu-latest"],
}


@pytest.mark.parametrize("job_id", MATRIX_JOBS)
def test_cua_sdk_matrix_on_main_is_the_full_matrix_minus_the_pull_request_legs(job_id):
    wf = load("ci-cua-sdk.yml")
    job = wf["jobs"][job_id]
    assert "reuse" in (job["needs"] if isinstance(job["needs"], list) else [job["needs"]])
    main_only, full, pr = literals(job["strategy"]["matrix"]["os"])
    assert pr == MATRIX_JOBS[job_id]
    assert main_only == [leg for leg in full if leg not in pr]
    # Every dropped leg is one the subset covers, and none is dropped when it is not.
    subset = re.search(r'--subset "pr_subset=([^"]+)"', reuse_run_command(wf)).group(1).split("|")
    prefix = job["name"].split("(")[0].replace("${{ matrix.os }}", "").strip()
    for leg in pr:
        assert f"{prefix} ({leg})" in subset


def test_cua_sdk_jobs_in_the_subset_skip_only_when_it_is_reused():
    wf = load("ci-cua-sdk.yml")
    skip = "needs.reuse.outputs.pr_subset != 'true'"
    for job_id in ("crates-dry-run", "kotlin", "build-sdk", "build-fixtures", "build-cli", "browser"):
        assert skip in wf["jobs"][job_id]["if"], job_id
        assert wf["jobs"][job_id]["if"].startswith("${{ !cancelled() && ")
    # The macOS and Windows legs of python/node still run when the Linux builds are skipped.
    for job_id in ("python", "node"):
        cond = wf["jobs"][job_id]["if"]
        assert "pr_subset" not in cond and "!cancelled()" in cond
        for need in ("build-sdk", "build-fixtures", "build-cli"):
            assert f"needs.{need}.result != 'failure'" in cond and f"needs.{need}.result != 'cancelled'" in cond
    # Jobs outside the subset are untouched by the reuse.
    assert "reuse" not in str(wf["jobs"]["swift"])


def test_cua_sdk_reuse_job_cannot_block_the_run():
    wf = load("ci-cua-sdk.yml")
    step = next(s for s in wf["jobs"]["reuse"]["steps"] if s.get("id") == "reuse")
    assert step["continue-on-error"] is True
    assert step["env"]["REUSE_MODE"] == "${{ vars.CI_REUSE_PR_RESULTS }}"
    assert wf["jobs"]["reuse"]["permissions"] == {
        "contents": "read", "actions": "read", "checks": "read", "pull-requests": "read",
    }


LANES = {
    # workflow: (stamp job, matrix job id -> (name template, legs step id), job id -> name reused as is)
    "ci-cua-spacesd.yml": (
        "cua-spacesd / changes",
        {"test": ("fmt, clippy, test ({os})", "legs_test")},
        {"relay-e2e": "relay e2e (docker)"},
    ),
    "ci-cua-driver-contract-clients.yml": (
        "contract clients / changes",
        {
            "mcp-client-discovery": ("MCP discovery in pinned clients ({os})", "legs_discovery"),
            "runtime-parity": ("Portable contract parity ({os})", "legs_parity"),
        },
        {"verify": "Generated contract and SDK bindings"},
    ),
}


@pytest.mark.parametrize("filename", LANES)
def test_scoped_lanes_wiring(filename):
    stamp_job, matrix_jobs, named_jobs = LANES[filename]
    wf = load(filename)
    changes = wf["jobs"]["changes"]
    assert changes["name"] == stamp_job
    steps = {s.get("id"): s for s in changes["steps"]}
    assert f'--stamp-job "{stamp_job}"' in steps["reuse"]["run"]
    assert steps["reuse"]["continue-on-error"] is True
    for job_id, (template, step_id) in matrix_jobs.items():
        job = wf["jobs"][job_id]
        assert job["name"].replace("${{ matrix.os }}", "{os}") == template
        step = steps[step_id]
        assert f'--template "{template}"' in step["run"]
        assert step["continue-on-error"] is True
        assert step["env"] == {"REUSED": "${{ steps.reuse.outputs.reused }}", "LEGS": "${{ steps.scope.outputs.matrix }}"}
        assert job["strategy"]["matrix"]["os"] == f"${{{{ fromJSON(needs.changes.outputs.{step_id}) }}}}"
        assert f"needs.changes.outputs.{step_id} != '[]'" in job["if"]
        # A failed legs step falls back to the unreduced matrix.
        assert changes["outputs"][step_id] == f"${{{{ steps.{step_id}.outputs.legs || steps.scope.outputs.matrix }}}}"
    for job_id, name in named_jobs.items():
        job = wf["jobs"][job_id]
        assert job["name"] == name
        assert f"!contains(fromJSON(needs.changes.outputs.reused), '{name}')" in job["if"]
    assert changes["outputs"]["reused"] == "${{ steps.reuse.outputs.reused || '[]' }}"
    assert {"actions": "read", "checks": "read"}.items() <= changes["permissions"].items()


@pytest.mark.parametrize("filename", ["ci-cua-sdk.yml", *LANES])
def test_script_change_triggers_the_workflow(filename):
    wf = load(filename)
    for event in ("pull_request", "push"):
        assert ".github/scripts/ci_reuse_pr_result.py" in wf[True][event]["paths"]
