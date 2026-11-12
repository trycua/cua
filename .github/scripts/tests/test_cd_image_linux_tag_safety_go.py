"""tag-safety-test installs crane and oras with `go install`. oras requires a
newer Go than the runner ships, so without setup-go the go command downloads a
toolchain through proxy.golang.org, which 404s right after a patch release and
fails the job for reasons unrelated to the PR. The job must set Go up first.
"""

from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
WORKFLOW = (ROOT / ".github/workflows/cd-image-linux.yml").read_text()


def test_tag_safety_test_sets_up_go_before_go_install():
    job = WORKFLOW.split("\n  tag-safety-test:\n", 1)[1].split("\n  report-schedule-failure:\n", 1)[0]
    assert "actions/setup-go@" in job
    assert job.index("actions/setup-go@") < job.index("go install")
