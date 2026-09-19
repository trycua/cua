import json
import os
from pathlib import Path
import subprocess

import yaml


ROOT = Path(__file__).resolve().parents[3]
WORKFLOW = ROOT / ".github/workflows/e2e-rust-windows.yml"


def matrix_summary_script() -> str:
    workflow = yaml.safe_load(WORKFLOW.read_text(encoding="utf-8"))
    steps = workflow["jobs"]["summary"]["steps"]
    return next(step["run"] for step in steps if step.get("name") == "Publish matrix summary")


def test_matrix_summary_recovers_results_when_lane_summary_is_missing(tmp_path: Path) -> None:
    artifact = tmp_path / "artifacts/rust-windows-native"
    artifact.mkdir(parents=True)
    records = [
        {
            "cell_id": "windows-wpf-click",
            "test_status": "fail",
            "observed_behavior": "error",
            "message": "fixture stopped | unexpectedly",
            "evidence": {"video": "recordings/windows-wpf-click/recording.mp4"},
        },
        {
            "cell_id": "windows-winui3-snapshot",
            "test_status": "pass",
            "observed_behavior": "delivered",
            "message": "",
            "evidence": {},
        },
    ]
    (artifact / "results.jsonl").write_text(
        "".join(json.dumps(record) + "\n" for record in records), encoding="utf-8"
    )

    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    gh = bin_dir / "gh"
    gh.write_text(
        "#!/usr/bin/env bash\n"
        "printf '%s\\n' '{\"artifacts\":[{\"name\":\"rust-windows-native\",\"id\":1234}]}'\n",
        encoding="utf-8",
    )
    gh.chmod(0o755)

    step_summary = tmp_path / "step-summary.md"
    env = os.environ.copy()
    env.update(
        {
            "PATH": f"{bin_dir}:{env['PATH']}",
            "GITHUB_REPOSITORY": "cua/example",
            "GITHUB_RUN_ID": "77",
            "GITHUB_SERVER_URL": "https://github.example",
            "GITHUB_STEP_SUMMARY": str(step_summary),
            "SOURCE_SHA": "0123456789abcdef0123456789abcdef01234567",
        }
    )
    subprocess.run(
        ["bash", "-euo", "pipefail", "-c", matrix_summary_script()],
        cwd=tmp_path,
        env=env,
        check=True,
    )

    summary = (tmp_path / "matrix-summary.md").read_text(encoding="utf-8")
    assert "Source commit: `0123456789abcdef0123456789abcdef01234567`" in summary
    assert "## rust-windows-native" in summary
    assert "The lane did not produce `summary.md`." in summary
    assert "Records: 2; pass: 1; fail: 1; other: 0." in summary
    artifact_url = "https://github.example/cua/example/actions/runs/77/artifacts/1234"
    assert f"[Download rust-windows-native]({artifact_url})" in summary
    assert "| windows-wpf-click | fail | error | fixture stopped \\| unexpectedly |" in summary
    assert f"[recordings/windows-wpf-click/recording.mp4]({artifact_url})" in summary
    assert "No lane summary artifact was produced." not in summary
    assert step_summary.read_text(encoding="utf-8") == summary
