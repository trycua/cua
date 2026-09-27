"""Keep the weights-backed Cua-S1 checks pinned, verified, secret-free, and cache-free."""

from __future__ import annotations

import importlib.util
import json
import re
from pathlib import Path

import pytest
import yaml

ROOT = Path(__file__).resolve().parents[3]
WORKFLOW = ROOT / ".github/workflows/ci-cua-s1-weights.yml"
LOCK = ROOT / "libs/cua-s1/ci/weights.lock.json"
FETCH = ROOT / "libs/cua-s1/ci/fetch_pinned_weights.py"
README = ROOT / "libs/cua-s1/README.md"
LINUX_RUNNER = ROOT / "scripts/ci/linux/run-rust-e2e.sh"
ROW = "perception_s1_decision_loop_test"


def load() -> dict:
    return yaml.safe_load(WORKFLOW.read_text(encoding="utf-8"))


def fetch_module():
    spec = importlib.util.spec_from_file_location("fetch_pinned_weights", FETCH)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


def test_workflow_is_read_only_secret_free_and_never_caches_weights() -> None:
    text = WORKFLOW.read_text(encoding="utf-8")
    workflow = load()
    assert workflow["permissions"] == {"contents": "read"}
    assert "secrets." not in text
    assert "HF_TOKEN" not in text
    assert "pull_request_target" not in workflow[True]
    # The weights exceed half of the repository's 10 GB Actions cache.
    assert "actions/cache" not in text
    for job in workflow["jobs"].values():
        for step in job["steps"]:
            if "setup-uv" in step.get("uses", ""):
                assert step["with"]["cache-dependency-glob"] == "libs/cua-s1/python/uv.lock"


def test_triggers_are_nightly_dispatch_and_path_filtered_pull_requests() -> None:
    triggers = load()[True]
    assert triggers["schedule"]
    assert "workflow_dispatch" in triggers
    assert "push" not in triggers
    paths = triggers["pull_request"]["paths"]
    for required in (
        "libs/cua-s1/**",
        "libs/cua-driver/examples/jev-use/python/choose_decision.py",
        "libs/cua-driver/examples/jev-use/python/decision_models.py",
        "libs/cua-driver/examples/jev-use/verify_decision_cli.py",
        "libs/cua-driver/examples/jev-use/fixtures/jev-choice-*",
        f"libs/cua-driver/rust/crates/cua-driver-e2e/tests/{ROW}.rs",
        ".github/workflows/ci-cua-s1-weights.yml",
    ):
        assert required in paths, required


def test_jobs_use_standard_hosted_linux_and_cpu_without_float32() -> None:
    workflow = load()
    for name, job in workflow["jobs"].items():
        assert job["runs-on"] == "ubuntu-latest", name
    options = workflow[True]["workflow_dispatch"]["inputs"]["dtype"]["options"]
    assert "float32" not in options  # 18.7 GB of float32 weights exceed 16 GB of RAM
    assert workflow["jobs"]["smoke"]["env"]["S1_DEVICE"] == "cpu"
    assert workflow["jobs"]["driver-e2e"]["env"]["S1_DEVICE"] == "cpu"


def test_smoke_runs_the_documented_verifier_in_both_modalities() -> None:
    steps = {step.get("name"): step for step in load()["jobs"]["smoke"]["steps"]}
    documented = steps["Run the documented chooser smoke (cold process per decision)"]["run"]
    assert "verify_decision_cli.py" in documented
    assert "for modality in text multimodal" in documented
    assert "--fixture negative --expected-id abstain" in documented
    assert "jev-choice-request-v1.png" in documented
    assert "jev-choice-negative-v1.png" in documented
    assert "fetch_pinned_weights.py" in steps["Download and verify the pinned weights"]["run"]
    assert "GITHUB_STEP_SUMMARY" in steps["Publish latency and memory summary"]["run"]


def test_fixture_screenshots_exist_for_both_fixtures() -> None:
    fixtures = ROOT / "libs/cua-driver/examples/jev-use/fixtures"
    for stem in ("jev-choice-request-v1", "jev-choice-negative-v1"):
        assert (fixtures / f"{stem}.json").is_file()
        assert (fixtures / f"{stem}.png").read_bytes().startswith(b"\x89PNG\r\n\x1a\n")


def test_lock_pins_match_the_documented_revisions() -> None:
    fetch = fetch_module()
    artifacts = {item["repo_id"]: item for item in fetch.load_lock(LOCK)}
    assert set(artifacts) == {"Qwen/Qwen3.5-4B", "cua-ai/cua-s1-4b-0.2"}
    readme = README.read_text(encoding="utf-8")
    for repo_id, artifact in artifacts.items():
        assert re.search(
            rf"{re.escape(repo_id)}\s*\\?\n?\s*--revision {artifact['revision']}", readme
        ), repo_id
    base = artifacts["Qwen/Qwen3.5-4B"]["files"]
    assert {
        "model.safetensors-00001-of-00002.safetensors",
        "model.safetensors-00002-of-00002.safetensors",
        "config.json",
        "tokenizer.json",
    } <= set(base)
    adapter = artifacts["cua-ai/cua-s1-4b-0.2"]["files"]
    for modality in ("text", "multimodal"):
        assert f"{modality}/adapter_config.json" in adapter
        assert f"{modality}/adapter_model.safetensors" in adapter


def test_fetch_rejects_unsafe_locks(tmp_path: Path) -> None:
    fetch = fetch_module()
    document = json.loads(LOCK.read_text(encoding="utf-8"))
    document["artifacts"][1]["files"] = {"../escape": {"size": 1, "sha256": "0" * 64}}
    unsafe = tmp_path / "lock.json"
    unsafe.write_text(json.dumps(document), encoding="utf-8")
    with pytest.raises(ValueError, match="unsafe file path"):
        fetch.load_lock(unsafe)
    document["artifacts"][1]["files"] = {"a": {"size": 1, "sha256": "0" * 64}}
    document["artifacts"][1]["revision"] = "main"
    unsafe.write_text(json.dumps(document), encoding="utf-8")
    with pytest.raises(ValueError, match="full commit SHA"):
        fetch.load_lock(unsafe)


def test_verify_detects_missing_extra_and_tampered_files(tmp_path: Path) -> None:
    fetch = fetch_module()
    import hashlib

    good = b"weights"
    artifact = {
        "name": "demo",
        "files": {
            "a.safetensors": {"size": len(good), "sha256": hashlib.sha256(good).hexdigest()},
            "sub/b.json": {"size": 2, "sha256": hashlib.sha256(b"{}").hexdigest()},
        },
    }
    (tmp_path / "sub").mkdir()
    (tmp_path / "a.safetensors").write_bytes(good)
    (tmp_path / "sub/b.json").write_bytes(b"{}")
    (tmp_path / ".cache/huggingface").mkdir(parents=True)
    (tmp_path / ".cache/huggingface/meta").write_text("ignored")
    assert fetch.verify_artifact(tmp_path, artifact) == []

    (tmp_path / "a.safetensors").write_bytes(b"weightz")
    (tmp_path / "extra.bin").write_bytes(b"x")
    (tmp_path / "sub/b.json").unlink()
    problems = "\n".join(fetch.verify_artifact(tmp_path, artifact))
    assert "a.safetensors: sha256" in problems
    assert "sub/b.json: missing" in problems
    assert "extra.bin: not in the lock" in problems


def test_driver_row_runs_through_the_canonical_linux_runner_only_in_its_lane() -> None:
    job = load()["jobs"]["driver-e2e"]
    steps = {step.get("name"): step for step in job["steps"]}
    row = steps["Run the S1 + OmniParser capture-loop row"]
    assert row["env"]["CUA_E2E_INTERNAL_LANE"] == "s1-perception"
    assert "scripts/ci/linux/run-rust-e2e.sh" in row["run"]
    assert "xvfb-run" in row["run"]
    download = steps["Download the published cua-perception release"]["run"]
    assert "releases/download/${tag}" in download
    assert "sha256sum --check --strict" in download
    assert job["env"]["S1_MODALITY"] == "text"

    runner = LINUX_RUNNER.read_text(encoding="utf-8")
    assert f"--test {ROW}" in runner
    lane = runner.split('if [[ "${SUITE}" == s1-perception ]]; then', 1)[1].split("\nfi\n", 1)[0]
    assert ROW in lane
    # The canonical lanes never select the model-backed row.
    assert runner.count(ROW) == 1
    for required in (
        "CUA_E2E_PERCEPTION_CATALOG",
        "CUA_E2E_PERCEPTION_VERSION",
        "CUA_E2E_S1_PYTHON",
        "S1_BASE_MODEL_PATH",
        "S1_ADAPTER_PATH",
    ):
        assert required in runner


def test_driver_row_records_a_typed_limitation_without_avx512_bf16() -> None:
    steps = load()["jobs"]["driver-e2e"]["steps"]
    names = [step.get("name") or step.get("uses") for step in steps]
    check = steps[names.index("Check the runner CPU against the capture-lifetime budget")]
    assert check["id"] == "cpu"
    assert "avx512_bf16" in check["run"]
    assert "cua-e2e-limitation-v1" in check["run"]
    assert "perception-s1-decision-loop-limitation.json" in check["run"]
    gated = steps[names.index(check["name"]) + 1 : names.index("Publish row summary")]
    assert gated and all(step["if"] == "steps.cpu.outputs.supported == 'true'" for step in gated)
    for name in ("Publish row summary", "Upload row evidence"):
        assert steps[names.index(name)]["if"] == "always()"
