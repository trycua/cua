"""Project config discovery: ``.cua/config.yaml`` (or agents.yaml) in the
task's directory or an ancestor below ``$HOME``. ``~/.cua`` is the cua CLI's
state directory and is never read as a project config. Temp HOME only."""

from __future__ import annotations

from pathlib import Path

import pytest
from cua_bench.config import ConfigLoader


@pytest.fixture
def home(tmp_path, monkeypatch) -> Path:
    home = tmp_path / "home"
    home.mkdir()
    monkeypatch.setenv("HOME", str(home))
    monkeypatch.delenv("USERPROFILE", raising=False)
    return home


def _write(path: Path, text: str = "defaults:\n  max_steps: 7\n") -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)
    return path


def test_cua_cli_state_dir_in_home_is_not_a_project_config(home):
    # What the cua CLI keeps in ~/.cua, and even a config.yaml there.
    (home / ".cua" / "sandboxes").mkdir(parents=True)
    _write(home / ".cua" / "config.yaml")
    task = home / "work" / "tasks" / "first-task"
    task.mkdir(parents=True)
    loader = ConfigLoader(task)
    assert loader.find_config_dir() is None
    assert loader.load_config() is None
    assert loader.get_effective_config({}) == {}


def test_nearest_project_config_below_home_is_found(home):
    project = home / "work" / "bench"
    _write(project / ".cua" / "config.yaml")
    task = project / "tasks" / "first-task"
    task.mkdir(parents=True)
    loader = ConfigLoader(task)
    assert loader.find_config_dir() == (project / ".cua").resolve()
    assert loader.load_config().defaults.max_steps == 7


def test_agents_yaml_alone_marks_a_project(home):
    project = home / "work" / "bench"
    _write(project / ".cua" / "agents.yaml", "agents: []\n")
    assert ConfigLoader(project).find_config_dir() == (project / ".cua").resolve()


def test_a_bare_cua_dir_is_skipped_for_a_real_one_above(home):
    project = home / "work"
    _write(project / ".cua" / "config.yaml")
    (project / "nested" / ".cua").mkdir(parents=True)  # no config files
    loader = ConfigLoader(project / "nested")
    assert loader.find_config_dir() == (project / ".cua").resolve()


def test_search_outside_home_walks_to_the_root(home, tmp_path):
    outside = tmp_path / "elsewhere"
    _write(outside / ".cua" / "config.yaml")
    task = outside / "a" / "b"
    task.mkdir(parents=True)
    assert ConfigLoader(task).find_config_dir() == (outside / ".cua").resolve()


def test_nothing_above_home_is_read(tmp_path, monkeypatch):
    parent = tmp_path / "users"
    _write(parent / ".cua" / "config.yaml")
    home = parent / "me"
    (home / "work").mkdir(parents=True)
    monkeypatch.setenv("HOME", str(home))
    assert ConfigLoader(home / "work").find_config_dir() is None
