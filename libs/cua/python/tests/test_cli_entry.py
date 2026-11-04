"""The `cua` console command: runs the CLI from the `cua-cli` wheel, hands
it the wheel's static cua-spacesd, and without the CLI (`--no-deps`) says
how to install it."""

from __future__ import annotations

import os

from cua import _cli


def test_missing_cli_prints_how_to_install(monkeypatch, capsys) -> None:
    monkeypatch.setattr(_cli, "binary", lambda: None)
    assert _cli.main() == 127
    err = capsys.readouterr().err
    assert 'pip install "cua-cli' in err
    assert err.count("\n") == 1


def test_the_cli_comes_from_the_cua_cli_package() -> None:
    exe = _cli.binary()
    if exe is None:  # an SDK-only install
        return
    assert exe.is_file() and exe.parent.name == "bin" and exe.parent.parent.name == "cua_cli"


def test_the_cli_is_handed_the_wheel_spacesd(monkeypatch, tmp_path) -> None:
    sd = tmp_path / "cua-spacesd-linux-x86_64"
    sd.write_bytes(b"")
    seen = {}
    monkeypatch.setattr(_cli, "binary", lambda: tmp_path / "cua")
    monkeypatch.setattr(_cli, "spacesd", lambda: sd)
    monkeypatch.delenv(_cli.SPACESD_ENV, raising=False)

    def fake_exec(path, argv):
        seen["env"] = os.environ.get(_cli.SPACESD_ENV)
        raise SystemExit(0)

    monkeypatch.setattr(_cli.os, "execv", fake_exec)
    monkeypatch.setattr(_cli.subprocess, "call", lambda argv: fake_exec(argv[0], argv))
    try:
        _cli.main()
    except SystemExit:
        pass
    assert seen["env"] == str(sd)
    # A value the user set wins.
    monkeypatch.setenv(_cli.SPACESD_ENV, "/mine")
    try:
        _cli.main()
    except SystemExit:
        pass
    assert seen["env"] == "/mine"


def test_the_source_pyproject_pins_no_cua_cli() -> None:
    # Source and editable installs of libs/cua/python must resolve without
    # a cua-cli of the checkout's version on an index; build_wheel.py adds
    # the pin to the built wheel only.
    import tomllib
    from pathlib import Path

    pyproject = Path(__file__).resolve().parent.parent / "pyproject.toml"
    project = tomllib.loads(pyproject.read_text())["project"]
    deps = project.get("dependencies", []) + [
        d for ds in project.get("optional-dependencies", {}).values() for d in ds
    ]
    assert not [d for d in deps if d.replace(" ", "").startswith("cua-cli")]


def test_a_built_wheel_with_its_cli_requires_it_at_its_version() -> None:
    from importlib import metadata

    try:
        metadata.version("cua-cli")
    except metadata.PackageNotFoundError:
        return  # a source, editable or --no-deps install
    requires = [r.replace(" ", "") for r in metadata.requires("cua") or []]
    assert f"cua-cli=={metadata.version('cua')}" in requires
    assert metadata.version("cua-cli") == metadata.version("cua")
