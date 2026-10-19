"""After an install, macOS must know exactly one Cua Driver: the installed one.

`open -a CuaDriver`, a bundle-id lookup and System Settings resolve to whichever
registered copy macOS prefers, so an old release in the Trash or a benchmark
folder gets launched and labels the permission rows ("CuaDriver-0.34.0"). The
installer parses `lsregister -dump` and `mdfind`, unregisters every other
com.trycua.driver bundle with `lsregister -u`, prints one line, and never
deletes a file. The real lsregister is never called: a fake records its argv.
"""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[4]
COMMON = ROOT / "libs/cua-driver/scripts/_install-common.sh"

SEPARATOR = "-" * 80

BLOCK = """{sep}
bundle id:                  {name} (0xf0ac)
container:                  / (0x4)
path:                       {path} (0x10268)
name:                       {name}
identifier:                 {identifier}
version:                    0.36 ({{length = 32, bytes = 0x0000}})
codeInfoID:                 {identifier}
executable:                 Contents/MacOS/cua-driver
infoDictionary:             14 values (144140 (0x2330c))
                            {{
                                CFBundleIdentifier = "{identifier}";
                            }}
"""


def dump(*entries: tuple[str, str, str]) -> str:
    return "".join(
        BLOCK.format(sep=SEPARATOR, name=name, path=path, identifier=identifier)
        for name, path, identifier in entries
    ) + SEPARATOR + "\n"


class Fixture:
    def __init__(self, root: Path) -> None:
        self.root = root
        self.bin = root / "bin"
        self.bin.mkdir()
        self.unregistered = root / "unregistered.log"
        self.active = root / "Applications/CuaDriver.app"
        self.active.mkdir(parents=True)

    def app(self, relative: str) -> Path:
        path = self.root / relative
        (path / "Contents").mkdir(parents=True)
        (path / "Contents/Info.plist").write_text("<plist/>", encoding="utf-8")
        return path

    def fake_lsregister(self, dump_text: str) -> Path:
        dump_file = self.root / "dump.txt"
        dump_file.write_text(dump_text, encoding="utf-8")
        script = self.bin / "lsregister"
        script.write_text(
            "#!/bin/sh\n"
            f'case "$1" in\n'
            f'  -dump) cat "{dump_file}" ;;\n'
            f'  -u) printf "%s\\n" "$2" >> "{self.unregistered}" ;;\n'
            f'  *) exit 2 ;;\nesac\n',
            encoding="utf-8",
        )
        script.chmod(0o755)
        return script

    def fake_mdfind(self, *paths: Path) -> None:
        script = self.bin / "mdfind"
        lines = "".join(f'echo "{p}"\n' for p in paths)
        script.write_text(f"#!/bin/sh\n{lines}", encoding="utf-8")
        script.chmod(0o755)

    def run(self, body: str, lsregister: Path | None) -> subprocess.CompletedProcess[str]:
        env = {k: v for k, v in os.environ.items() if not k.startswith("CUA_DRIVER_")}
        env["PATH"] = f"{self.bin}:/usr/bin:/bin"
        if lsregister is not None:
            env["CUA_DRIVER_LSREGISTER"] = str(lsregister)
        return subprocess.run(
            ["/bin/bash", "-c", f'source "{COMMON}"\n{body}'],
            cwd=ROOT,
            env=env,
            text=True,
            capture_output=True,
            check=False,
            timeout=60,
        )

    def unregistered_paths(self) -> list[str]:
        if not self.unregistered.exists():
            return []
        return self.unregistered.read_text().splitlines()


@pytest.fixture
def fx(tmp_path: Path) -> Fixture:
    return Fixture(tmp_path)


def test_unregisters_every_other_copy_and_prints_one_line(fx: Fixture) -> None:
    downloads = fx.app("Downloads/CuaDriver.app")
    trash = fx.app(".Trash/CuaDriver.app")
    bench = fx.app("bench folder/CuaDriver-0.34.0.app")  # a path with spaces
    ls = fx.fake_lsregister(
        dump(
            ("Cua Driver", str(fx.active), "com.trycua.driver"),
            ("Cua Driver", str(downloads), "com.trycua.driver"),
            ("CuaDriver", str(trash), "com.trycua.driver"),
            ("CuaDriver-0.34.0", str(bench), "com.trycua.driver"),
        )
    )
    fx.fake_mdfind()

    result = fx.run(f'unregister_other_cua_driver_copies "{fx.active}"', ls)

    assert result.returncode == 0, result.stderr
    assert sorted(fx.unregistered_paths()) == sorted([str(downloads), str(trash), str(bench)])
    lines = result.stdout.splitlines()
    assert lines[0] == f"Found 3 old Cua Driver copies; macOS will now only use {fx.active}"
    # Honest about the files: they are still where the user put them.
    assert lines[1].startswith("Their files were left in place")
    for path in (downloads, trash, bench):
        assert str(path) in lines[1]
    assert len(lines) == 2
    # `lsregister -u` forgets a copy; the files stay where the user put them.
    for path in (downloads, trash, bench, fx.active):
        assert path.exists()


def test_one_copy_reads_naturally(fx: Fixture) -> None:
    old = fx.app("tmp/CuaDriver.app")
    ls = fx.fake_lsregister(
        dump(
            ("Cua Driver", str(fx.active), "com.trycua.driver"),
            ("Cua Driver", str(old), "com.trycua.driver"),
        )
    )
    fx.fake_mdfind()

    result = fx.run(f'unregister_other_cua_driver_copies "{fx.active}"', ls)

    assert result.stdout.splitlines()[0] == (
        f"Found 1 old Cua Driver copy; macOS will now only use {fx.active}"
    )


def test_copies_found_only_by_spotlight_are_unregistered_too(fx: Fixture) -> None:
    indexed = fx.app("Library/Application Support/images/CuaDriver.app")
    ls = fx.fake_lsregister(dump(("Cua Driver", str(fx.active), "com.trycua.driver")))
    fx.fake_mdfind(fx.active, indexed)

    result = fx.run(f'unregister_other_cua_driver_copies "{fx.active}"', ls)

    assert fx.unregistered_paths() == [str(indexed)]
    assert "Found 1 old Cua Driver copy" in result.stdout


def test_other_bundle_ids_and_the_installed_copy_are_left_alone(fx: Fixture) -> None:
    local = fx.app("Applications/CuaDriverLocal.app")
    spaces = fx.app("Applications/Cua Spaces.app")
    alias = fx.root / "alias"
    alias.symlink_to(fx.active)  # the installed copy under another name
    ls = fx.fake_lsregister(
        dump(
            ("Cua Driver", str(fx.active), "com.trycua.driver"),
            ("Cua Driver", str(alias), "com.trycua.driver"),
            ("Cua Driver Local", str(local), "com.trycua.driver.local"),
            ("Cua Spaces", str(spaces), "com.cua.spaces"),
        )
    )
    fx.fake_mdfind()

    result = fx.run(f'unregister_other_cua_driver_copies "{fx.active}"', ls)

    assert fx.unregistered_paths() == []
    assert result.stdout == ""


def test_no_other_copies_prints_nothing(fx: Fixture) -> None:
    ls = fx.fake_lsregister(dump(("Cua Driver", str(fx.active), "com.trycua.driver")))
    fx.fake_mdfind(fx.active)

    result = fx.run(f'unregister_other_cua_driver_copies "{fx.active}"', ls)

    assert result.returncode == 0
    assert result.stdout == ""


def test_a_failed_unregister_warns_but_never_fails_the_install(fx: Fixture) -> None:
    old = fx.app("Downloads/CuaDriver.app")
    ls = fx.fake_lsregister(
        dump(
            ("Cua Driver", str(fx.active), "com.trycua.driver"),
            ("Cua Driver", str(old), "com.trycua.driver"),
        )
    )
    fx.fake_mdfind()
    ls.write_text("#!/bin/sh\n[ \"$1\" = -dump ] && cat " + str(fx.root / "dump.txt") + "\nexit 1\n")

    result = fx.run(f'unregister_other_cua_driver_copies "{fx.active}"', ls)

    assert result.returncode == 0
    assert f"could not unregister the old Cua Driver copy at {old}" in result.stderr


def test_missing_lsregister_is_a_quiet_no_op(fx: Fixture) -> None:
    result = fx.run(
        f'unregister_other_cua_driver_copies "{fx.active}"', fx.root / "does-not-exist"
    )

    assert result.returncode == 0
    assert result.stdout == "" and result.stderr == ""


def test_installer_runs_the_cleanup_after_the_swap_commits() -> None:
    installer = (ROOT / "libs/cua-driver/scripts/_install-rust.sh").read_text()

    commit = installer.index("MACOS_APP_INSTALL_COMMITTED=1")
    cleanup = installer.index('unregister_other_cua_driver_copies "$APP_DEST"')
    reset = installer.index("macos_reset_tcc_after_requirement_change", commit)
    # After the new bundle is registered and committed, before any permission
    # reset resolves the bundle id through LaunchServices.
    assert commit < cleanup < reset
    # A tarball loader failure must not abort the install either.
    assert "unregister_other_cua_driver_copies() { :; }" in installer


def test_installer_forgets_the_unpacked_app_before_deleting_its_temp_dir() -> None:
    installer = (ROOT / "libs/cua-driver/scripts/_install-rust.sh").read_text()

    cleanup = installer[installer.index("cleanup_on_exit() {"):]
    cleanup = cleanup[: cleanup.index("\n}\n")]
    forget = cleanup.index("forget_staged_macos_app")
    delete = cleanup.index('rm -rf "$TMP_DIR"')
    assert forget < delete
    helper = installer[installer.index("forget_staged_macos_app() {"):]
    helper = helper[: helper.index("\n}\n")]
    assert '"$lsregister" -u "$SRC_APP"' in helper


def test_registrations_whose_files_are_gone_get_no_left_in_place_note(fx: Fixture) -> None:
    gone = fx.root / "extracted-and-deleted/CuaDriver.app"  # never created on disk
    ls = fx.fake_lsregister(
        dump(
            ("Cua Driver", str(fx.active), "com.trycua.driver"),
            ("Cua Driver", str(gone), "com.trycua.driver"),
        )
    )
    fx.fake_mdfind()

    result = fx.run(f'unregister_other_cua_driver_copies "{fx.active}"', ls)

    assert fx.unregistered_paths() == [str(gone)]
    assert result.stdout.splitlines() == [
        f"Found 1 old Cua Driver copy; macOS will now only use {fx.active}"
    ]


def test_many_leftover_copies_are_summarised(fx: Fixture) -> None:
    olds = [fx.app(f"old{i}/CuaDriver.app") for i in range(5)]
    ls = fx.fake_lsregister(
        dump(
            ("Cua Driver", str(fx.active), "com.trycua.driver"),
            *[("Cua Driver", str(path), "com.trycua.driver") for path in olds],
        )
    )
    fx.fake_mdfind()

    result = fx.run(f'unregister_other_cua_driver_copies "{fx.active}"', ls)

    lines = result.stdout.splitlines()
    assert lines[0].startswith("Found 5 old Cua Driver copies")
    assert lines[1].endswith("and 2 more")
