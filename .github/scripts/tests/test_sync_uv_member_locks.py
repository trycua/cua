from pathlib import Path
import subprocess

from sync_uv_member_locks import main


def package(name: str, version: str, source: str) -> str:
    return f'[[package]]\nname = "{name}"\nversion = "{version}"\nsource = {{ {source} }}\n'


def write(root: Path, rel: str, content: str) -> None:
    path = root / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content)


def test_in_repo_packages_follow_their_pyproject(tmp_path: Path, capsys) -> None:
    write(tmp_path, "libs/a/pyproject.toml", '[project]\nname = "pkg-a"\nversion = "2.0.0"\n')
    write(tmp_path, "uv.lock", package("pkg-a", "1.0.0", 'editable = "libs/a"'))
    write(tmp_path, "libs/b/uv.lock", package("pkg-a", "1.0.0", 'directory = "../a"'))
    # A registry release, and a path whose pyproject names another package: untouched.
    write(tmp_path, "libs/c/uv.lock", package("pkg-a", "1.0.0", 'registry = "https://pypi.org/simple"'))
    write(tmp_path, "libs/d/uv.lock", package("other", "1.0.0", 'editable = "../a"'))
    subprocess.run(["git", "init", "-q"], cwd=tmp_path, check=True)
    subprocess.run(["git", "add", "-A"], cwd=tmp_path, check=True)

    assert main(["--repo-root", str(tmp_path)]) == 0

    assert capsys.readouterr().out.split() == ["libs/b/uv.lock", "uv.lock"]
    assert '"2.0.0"' in (tmp_path / "uv.lock").read_text()
    assert '"2.0.0"' in (tmp_path / "libs/b/uv.lock").read_text()
    assert '"1.0.0"' in (tmp_path / "libs/c/uv.lock").read_text()
    assert '"1.0.0"' in (tmp_path / "libs/d/uv.lock").read_text()
