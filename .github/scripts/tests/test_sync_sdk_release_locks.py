import json
from pathlib import Path
import subprocess

import pytest

from sync_sdk_release_locks import main

ROOT = Path(__file__).resolve().parents[3]


def uv_package(name: str, version: str, source: str) -> str:
    return f'[[package]]\nname = "{name}"\nversion = "{version}"\nsource = {{ {source} }}\n'


@pytest.fixture
def repo(tmp_path: Path) -> Path:
    sdk_entry = '    "../../libs/cua/typescript": {\n      "name": "@trycua/cua",\n      "version": "0.4.1",\n      "license": "MIT"\n    },\n'
    files = {
        "libs/cua/VERSION": "0.5.0\n",
        # The checkout, editable or as a directory: follows the release.
        "uv.lock": "version = 1\nrevision = 3\n\n" + uv_package("cua", "0.4.1", 'editable = "libs/cua/python"'),
        "libs/a/uv.lock": uv_package("cua", "0.4.1", 'directory = "../cua/python"'),
        # A registry release of cua, and another cua-* package: left alone.
        "libs/b/uv.lock": uv_package("cua", "0.2.0", 'registry = "https://pypi.org/simple"'),
        "libs/c/uv.lock": uv_package("cua-sandbox", "0.4.1", 'editable = "."'),
        # The SDK's own lock is Release Please's.
        "libs/cua/typescript/package-lock.json": '{\n  "packages": {\n    "": {\n      "version": "0.4.1"\n    }\n  }\n}\n',
        "samples/ts/package-lock.json": '{\n  "packages": {\n    "": {\n      "version": "1.0.0"\n    },\n'
        + sdk_entry
        + '    "node_modules/x": {\n      "version": "0.4.1"\n    }\n  }\n}\n',
        # A registry install of the SDK is not a link to the checkout.
        "samples/other/package-lock.json": json.dumps(
            {"packages": {"": {}, "node_modules/@trycua/cua": {"version": "0.4.1"}}}, indent=2
        ),
    }
    for rel, content in files.items():
        path = tmp_path / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)
    subprocess.run(["git", "init", "-q"], cwd=tmp_path, check=True)
    subprocess.run(["git", "add", "-A"], cwd=tmp_path, check=True)
    return tmp_path


def test_only_the_recorded_sdk_version_changes(repo: Path, capsys: pytest.CaptureFixture[str]) -> None:
    before = {p: p.read_text() for p in repo.rglob("*.*") if ".git" not in p.parts}

    assert main(["--repo-root", str(repo)]) == 0

    assert capsys.readouterr().out.split() == ["libs/a/uv.lock", "uv.lock", "samples/ts/package-lock.json"]
    changed = {p for p, text in before.items() if p.read_text() != text}
    assert changed == {repo / "uv.lock", repo / "libs/a/uv.lock", repo / "samples/ts/package-lock.json"}
    for path in changed:
        assert path.read_text() == before[path].replace('"0.4.1"', '"0.5.0"', 1)
    # The lock revision is not rewritten, and a dependency at the old version stays.
    assert "revision = 3" in (repo / "uv.lock").read_text()
    assert '"node_modules/x": {\n      "version": "0.4.1"' in (repo / "samples/ts/package-lock.json").read_text()

    assert main(["--repo-root", str(repo)]) == 0
    assert capsys.readouterr().out.strip() == ""


def test_an_unrecognised_sdk_link_fails_loudly(repo: Path) -> None:
    lock = repo / "samples/ts/package-lock.json"
    data = json.loads(lock.read_text())
    lock.write_text(json.dumps(data, separators=(",", ":")))
    with pytest.raises(RuntimeError, match="expected one version"):
        main(["--repo-root", str(repo)])


def test_release_please_runs_the_lock_sync_for_sdk_releases() -> None:
    workflow = (ROOT / ".github/workflows/release-please.yml").read_text()
    sdk_branch = workflow.split('elif [[ "$SDK" == "true" ]]; then', 1)[1].split("else\n", 1)[0]
    assert "python3 .github/scripts/sync_sdk_release_locks.py" in sdk_branch
    assert 'git add -- "${SDK_LOCK_PATHS[@]}"' in sdk_branch
