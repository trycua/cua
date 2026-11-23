from pathlib import Path
import subprocess

from widen_sibling_dependency_caps import main, widen


def test_cap_below_the_release_follows_it() -> None:
    text = 'dependencies = [\n    "cua-sandbox>=0.8.0,<0.10.0",\n]\n'
    assert '"cua-sandbox>=0.8.0,<0.11.0"' in widen(text, "cua-sandbox", "0.9.1", "0.10.0")


def test_extras_and_markers_are_kept() -> None:
    text = '"cua-sandbox[web]>=0.8.0,<0.10.0; python_version >= \'3.11\'"'
    assert widen(text, "cua-sandbox", "0.9.1", "0.10.0") == (
        '"cua-sandbox[web]>=0.8.0,<0.11.0; python_version >= \'3.11\'"'
    )


def test_cap_that_already_excluded_the_previous_release_is_deliberate() -> None:
    text = '"cua-sandbox>=0.1.28,<0.2.0"'
    assert widen(text, "cua-sandbox", "0.9.1", "0.10.0") == text


def test_cap_above_the_release_and_other_packages_are_untouched() -> None:
    text = '"cua-sandbox>=0.8.0,<0.12.0"\n"cua-sandbox-apps>=0.1,<0.3"\n"other>=1,<2"'
    assert widen(text, "cua-sandbox", "0.9.1", "0.10.0") == text


def test_main_prints_changed_files(tmp_path: Path, capsys) -> None:
    (tmp_path / "libs/a").mkdir(parents=True)
    (tmp_path / "libs/a/pyproject.toml").write_text('d = ["cua-sandbox>=0.8.0,<0.10.0"]\n')
    (tmp_path / "libs/b").mkdir()
    (tmp_path / "libs/b/pyproject.toml").write_text('d = ["cua-sandbox>=0.8.0"]\n')
    subprocess.run(["git", "init", "-q"], cwd=tmp_path, check=True)
    subprocess.run(["git", "add", "-A"], cwd=tmp_path, check=True)
    args = ["--repo-root", str(tmp_path), "--package", "cua-sandbox", "--from", "0.9.1", "--to", "0.10.0"]
    assert main(args) == 0
    assert capsys.readouterr().out.split() == ["libs/a/pyproject.toml"]
    assert "<0.11.0" in (tmp_path / "libs/a/pyproject.toml").read_text()
