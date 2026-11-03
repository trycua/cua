"""The bench's macOS app envs, offline: what each declares at put, the launcher's commands and window placement with a
fake Cua Driver, and the sentinel's arming against a log on disk."""

from __future__ import annotations

import json
import os
import plistlib
import re
import signal
import subprocess
import sys
from pathlib import Path

import pytest
from conftest import APPS, ARTIFACTS, load

TASKS = {"cdb-s02": "CdbS02", "cdb-s03": "CdbS03", "cdb-s04": "CdbS04"}
COMMIT = "22a67eacd15dc71811e330ed88951dd615bb395d"
SHARED = ("bench_macos.py", "install.sh")


def declared(folder: str, cls: str):
    return getattr(load(APPS / folder, "app.py"), cls).declaration()


def test_the_files_every_task_app_shares_are_the_same_in_each():
    for name in SHARED:
        assert len({(APPS / t / name).read_bytes() for t in TASKS}) == 1, name
    assert (APPS / "benchlab" / "build_app.sh").read_bytes() == (
        APPS / "bench-sentinel" / "build_app.sh"
    ).read_bytes()


@pytest.mark.parametrize("task", TASKS)
def test_a_task_app_lists_its_tasks_files_at_the_pinned_commit_with_the_macos_launch_descriptor(
    task,
):
    listed = json.loads((APPS / task / "files.json").read_text())
    paths = {f["path"] for f in listed["files"]}
    assert listed["commit"] == COMMIT
    assert "platform/launch.macos.json" in paths and "platform/launch.linux.json" not in paths
    for f in listed["files"]:
        assert f["url"] == (
            f"https://raw.githubusercontent.com/trycua/cua/{COMMIT}/libs/cua-driver-bench/taskpack/tasks/shared/{task}/{f['path']}"
        )
        assert re.fullmatch(r"[0-9a-f]{64}", f["sha256"])
        assert not f["path"].startswith(("tests/", "evaluator/", "tools/"))
        assert not (f["path"].startswith("apps/") and "/tests/" in f["path"])


@pytest.mark.parametrize("task", TASKS)
def test_a_task_app_declares_its_files_runtimes_workspace_and_launch(task):
    decl = declared(task, TASKS[task])
    macos = decl.os["macos"]
    files = json.loads((APPS / task / "files.json").read_text())["files"]
    tos = [d["to"] for d in macos.install["downloads"]]
    assert tos[: len(files)] == [f"task/{f['path']}" for f in files]
    assert sorted(tos[len(files) :]) == sorted(
        [
            "runtime/electron-arm64.zip",
            *(
                ["runtime/node-arm64.tar.xz", "runtime/chrome-arm64.zip"]
                if task == "cdb-s04"
                else []
            ),
        ]
    )
    assert decl.supports == [{"os": {"type": "macos"}, "arch": ["arm64"]}]
    assert macos.paths == [task] and macos.launch.at == "boot"
    assert macos.launch.command == ["venv/bin/python", "bench_macos.py"]
    assert [s.name for s in decl.install_steps] == ["workspace", "browser"]
    assert [c.name for c in decl.ready_checks] == ["apps_installed", "windows"]
    assert all(s.os == ["macos"] for s in decl.install_steps)


def test_every_download_is_https_and_checked_by_sha256():
    for task, cls in TASKS.items():
        for d in declared(task, cls).os["macos"].install["downloads"]:
            assert d["url"].startswith("https://") and re.fullmatch(r"[0-9a-f]{64}", d["sha256"])


@pytest.fixture(scope="module")
def bench():
    return load(APPS / "cdb-s03", "bench_macos.py")


def spec_of(task: str):
    # The launch descriptors are fetched at put, so the test uses the shapes the public ones have.
    return {
        "workspace_root": "${HOME}/" + task,
        "apps": [
            {
                "id": "mail",
                "command": ["npx", "electron", ".", "--workspace=${workspace}"],
                "unset_env": ["ELECTRON_RUN_AS_NODE"],
            },
            {"id": "archive", "command": ["node", "server.js"]},
            {
                "id": "browser",
                "command": [
                    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
                    "--no-first-run",
                    "--user-data-dir=${workspace}/.runtime/browser",
                    "http://127.0.0.1:4184/",
                ],
            },
            {
                "id": "calc",
                "command": [
                    "/Applications/LibreOffice.app/Contents/MacOS/soffice",
                    "--calc",
                    "-env:UserInstallation=${workspace_uri}/.runtime/libreoffice",
                    "${workspace}/a.ods",
                ],
            },
        ],
    }


def test_the_launcher_runs_the_apps_runtimes_in_place_of_npx_node_and_chrome(bench):
    values = {
        "${bundle}": "/opt/b/task",
        "${workspace}": "/Users/lume/cdb-s04",
        "${workspace_uri}": "file:///x",
        "${python}": "/py",
    }
    mail, archive, browser, calc = (bench._command(a, values) for a in spec_of("cdb-s04")["apps"])
    assert mail == [
        str(bench.RUNTIME / "electron/Electron.app/Contents/MacOS/Electron"),
        ".",
        "--workspace=/Users/lume/cdb-s04",
        "--user-data-dir=/Users/lume/cdb-s04/.runtime/electron-mail",
    ]
    assert archive == [str(bench.RUNTIME / "node/bin/node"), "server.js"]
    assert browser[0].endswith(
        "chrome/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing"
    )
    assert browser[1:] == [
        "--no-first-run",
        "--user-data-dir=/Users/lume/cdb-s04/.runtime/browser",
        "http://127.0.0.1:4184/",
    ]
    assert calc[0] == "/Applications/LibreOffice.app/Contents/MacOS/soffice"
    assert bench._sub("${unknown}/x", values) == "${unknown}/x"


def test_electron_is_pinned_to_the_darwin_arm64_release(bench):
    urls = [d.url for d in bench.task_install(workspace="cdb-s02", first_window="x")["downloads"]]
    assert any(u.endswith(f"electron-v{bench.ELECTRON}-darwin-arm64.zip") for u in urls)
    assert (
        bench.ELECTRON_SHA256 == "01bb5d8756d220002ed5f8dccfb5a01391c4b919667af7156257cf5252e3e8ae"
    )


def test_a_browser_install_unpacks_chrome_and_writes_the_managed_policies(
    bench, tmp_path, monkeypatch
):
    runtime = tmp_path / "runtime"
    runtime.mkdir()
    plist = tmp_path / "Managed Preferences" / "com.google.chrome.for.testing.plist"
    bench.install_browser(runtime, plist)
    assert not plist.exists()

    (runtime / "chrome-arm64.zip").write_bytes(b"zip")

    def ditto(argv, check):
        assert argv[:3] == ["ditto", "-x", "-k"]
        (Path(argv[4]) / "chrome-mac-arm64" / bench.CHROME_APP).mkdir(parents=True)

    monkeypatch.setattr(bench.subprocess, "run", ditto)
    bench.install_browser(runtime, plist)
    assert (runtime / "chrome" / bench.CHROME_APP).is_dir() and not (
        runtime / "chrome-arm64.zip"
    ).exists()
    policies = plistlib.loads(plist.read_bytes())
    assert policies["BrowserSignin"] == 0 and policies["DeveloperToolsAvailability"] == 2


def test_a_missing_libreoffice_or_gnucash_is_named_by_the_apps_ready_check(bench, monkeypatch):
    spec = spec_of("cdb-s02")
    monkeypatch.setattr(bench, "LIBREOFFICE", Path("/nonexistent/soffice"))
    spec["apps"][3]["command"][0] = "/nonexistent/soffice"
    assert bench.missing_apps(spec) == ["/nonexistent/soffice"]
    assert bench.missing_apps({"apps": [{"command": ["node"]}]}) == []


def test_a_stopped_apps_locks_are_removed_before_a_launch(bench, tmp_path):
    for name in (".~lock.a.ods#", "book.gnucash.LCK", "book.gnucash.LNK", "keep.ods"):
        (tmp_path / name).write_text("")
    bench._unlock(tmp_path)
    assert sorted(p.name for p in tmp_path.iterdir()) == ["keep.ods"]


def test_a_tip_of_the_day_is_switched_off_for_libreoffice_and_gnucash(bench, tmp_path):
    profile = tmp_path / "lo"
    bench._quiet([str(bench.LIBREOFFICE), f"-env:UserInstallation={profile.as_uri()}"], {})
    assert "ShowTipOfTheDay" in (profile / "user" / "registrymodifications.xcu").read_text()
    env = {"GNC_CONFIG_HOME": str(tmp_path / "gnc")}
    bench._quiet([str(bench.GNUCASH)], env)
    keyfile = (tmp_path / "gnc" / "glib-2.0" / "settings" / "keyfile").read_text()
    assert "show-at-startup=false" in keyfile and env["GSETTINGS_BACKEND"] == "keyfile"


def windows(*titles):
    return [
        {"title": t, "window_id": i, "pid": 100 + i, "bounds": None}
        for i, t in enumerate(titles, 1)
    ]


def test_a_window_two_matches_hold_belongs_to_the_more_specific(bench):
    listed = windows(
        "Ledger Lantern Mail",
        "kestrel-wharf-reconciliation.ods — LibreOffice",
        "kestrel-wharf.gnucash - GnuCash",
    )
    specs = [
        {"title": "Ledger Lantern Mail"},
        {"title_contains": "kestrel-wharf-reconciliation"},
        {"title_contains": "kestrel-wharf"},
    ]
    found = bench.claimed(specs, listed)
    assert [[w["window_id"] for w in f] for f in found] == [[1], [2], [3]]
    assert bench.claimed([{"title": "Nope"}], listed) == [[]]


def test_the_windows_check_wants_each_of_the_tasks_windows(bench):
    specs = [{"title": "Copperfield Mail"}, {"title_contains": "lighthouse-replan"}]
    assert all(bench.claimed(specs, windows("Copperfield Mail", "lighthouse-replan.ods")))
    assert not all(bench.claimed(specs, windows("Copperfield Mail")))


def test_a_window_is_moved_until_it_stays_at_its_bounds_three_looks_running(bench):
    bounds = {"x": 20, "y": 30, "width": 900, "height": 680}
    state = {"bounds": {"x": 0, "y": 0, "width": 500, "height": 500}}
    moves, clock = [], {"t": 0.0}

    def listing():
        return [
            {"title": "Copperfield Mail", "window_id": 1, "pid": 9, "bounds": dict(state["bounds"])}
        ]

    def move(listed, wanted):
        moves.append((listed["window_id"], wanted))
        state["bounds"] = dict(wanted)

    def sleep(s):
        clock["t"] += s

    assert bench._place(
        [{"title": "Copperfield Mail", "bounds": bounds}],
        listing=listing,
        move=move,
        now=lambda: clock["t"],
        sleep=sleep,
    )
    assert moves == [(1, bounds)]


def test_a_window_that_never_shows_fails_the_placement_after_its_deadline(bench):
    clock = {"t": 0.0}
    assert not bench._place(
        [{"title": "Missing", "bounds": {"x": 0, "y": 0, "width": 1, "height": 1}}],
        listing=lambda: [],
        move=lambda *a: None,
        now=lambda: clock["t"],
        sleep=lambda s: clock.update(t=clock["t"] + 30),
    )


def test_cua_drivers_window_list_is_read_for_titles_ids_and_pids(bench, monkeypatch):
    out = 'log line\n{"windows": [{"title": "A", "window_id": 7, "pid": 3, "bounds": {"x": 1}}, {"app": "no title"}]}'
    monkeypatch.setattr(bench, "cua_driver", lambda *args, **kw: out)
    assert bench.list_windows() == [{"title": "A", "window_id": 7, "pid": 3, "bounds": {"x": 1}}]
    monkeypatch.setattr(bench, "cua_driver", lambda *args, **kw: "not json")
    assert bench.list_windows() == []


# -- BenchLab and the sentinel ----------------------------------------------------------------------------------------


def test_benchlab_builds_from_the_three_swift_sources_at_the_pinned_commit():
    decl = declared("benchlab", "BenchLab")
    macos = decl.os["macos"]
    files = json.loads((APPS / "benchlab" / "files.json").read_text())
    assert [f["path"] for f in files["files"]] == [
        "swift/BenchLab.swift",
        "swift/BenchLabInterrupts.swift",
        "swift/BenchLabModes.swift",
    ]
    assert files["commit"] == COMMIT and all(
        f["url"].startswith(f"https://raw.githubusercontent.com/trycua/cua/{COMMIT}/")
        for f in files["files"]
    )
    assert macos.install["run"][0]["script"] == "install.sh" and macos.paths == [".benchlab"]
    assert (APPS / "benchlab" / "install.sh").read_text().count("BenchLab") >= 2


def test_benchlabs_settings_are_the_tasks_modes_and_a_seed():
    modes = declared("benchlab", "BenchLab").settings["properties"]["mode"]["enum"]
    assert modes == [
        "listdrag",
        "hover",
        "tooltip",
        "irmodal",
        "irbanner",
        "irunsaved",
        "irconsent",
    ]


def test_benchlab_starts_as_the_bench_does_with_its_mode_seed_and_data_paths(tmp_path):
    lab = load(APPS / "benchlab", "lab.py")
    settings = type("S", (), {"mode": "tooltip", "seed": 7})()
    argv = lab.argv(settings, tmp_path)
    assert argv[1:] == [
        "--mode",
        "tooltip",
        "--seed",
        "7",
        "--state",
        str(tmp_path / "state.json"),
        "--events",
        str(tmp_path / "events.jsonl"),
    ]
    assert argv[0].endswith("build/BenchLab.app/Contents/MacOS/BenchLab")


def test_the_sentinels_condition_is_a_setting_and_the_background_one_is_the_users_window():
    decl = declared("bench-sentinel", "BenchSentinel")
    assert decl.settings["properties"]["condition"]["enum"] == ["foreground", "background"]
    sentinel = load(APPS / "bench-sentinel", "sentinel.py")
    log = Path("/tmp/s.jsonl")
    assert sentinel.argv("foreground", log)[1:] == ["--log", "/tmp/s.jsonl"]
    assert sentinel.argv("background", log)[1:] == [
        "--log",
        "/tmp/s.jsonl",
        "--user-activity",
        "--frame",
        "420,340,900,520",
    ]
    assert decl.os["macos"].launch.ready.command == ["venv/bin/python", "sentinel.py", "--ready"]


def test_the_sentinel_is_ready_once_its_log_shows_a_start_and_a_sample(tmp_path):
    sentinel = load(APPS / "bench-sentinel", "sentinel.py")
    log = tmp_path / "sentinel.jsonl"
    assert not sentinel.ready(log)
    log.write_text('{"ev": "start", "pid": 1}\n{"ev": "s"')  # a torn last line
    assert not sentinel.ready(log)
    log.write_text('{"ev": "start", "pid": 1}\n{"ev": "s", "t": 1}\n')
    assert sentinel.ready(log)


@pytest.fixture
def arm():
    return load(ARTIFACTS / "sentinel-macos", "run.py")


def write(log, *lines):
    with open(log, "a") as f:
        f.writelines(json.dumps(line) + "\n" for line in lines)


def test_the_arming_artifact_reads_the_running_sentinels_pid_and_armed_state(
    arm, tmp_path, monkeypatch
):
    log = tmp_path / "s.jsonl"
    write(
        log,
        {"ev": "start", "pid": 11},
        {"ev": "start", "pid": 22},
        {"ev": "armed", "active": True, "key": True},
    )
    monkeypatch.setattr(arm, "_condition", lambda pid: "background")
    assert arm.Log(log).state() == (22, True, "background")


def test_a_log_read_leaves_a_torn_last_line_for_the_next_read(arm, tmp_path):
    log = tmp_path / "s.jsonl"
    log.write_text('{"ev": "a"}\n{"ev": "b"')
    reader = arm.Log(log)
    assert [line["ev"] for line in reader.read()] == ["a"]
    with open(log, "a") as f:
        f.write("}\n")
    assert [line["ev"] for line in reader.read()] == ["b"]


def test_arming_clears_the_clipboard_activates_then_arms_and_checks_the_armed_line(
    arm, tmp_path, monkeypatch
):
    log = tmp_path / "s.jsonl"
    write(log, {"ev": "start", "pid": 4242})
    sent, ran = [], []
    monkeypatch.setattr(arm, "LOG", log)
    monkeypatch.setattr(arm, "_condition", lambda pid: "foreground")
    monkeypatch.setattr(arm.subprocess, "run", lambda argv, **kw: ran.append(argv))

    def kill(pid, sig):
        sent.append(sig)
        if sig == signal.SIGUSR1:
            write(log, {"ev": "s", "active": True, "key": True})
        else:
            write(log, {"ev": "armed", "active": True, "key": True})

    monkeypatch.setattr(arm.os, "kill", kill)
    result = arm.run({"args": {"action": "arm", "condition": "foreground"}})
    assert ran == [["pbcopy"]] and sent == [signal.SIGUSR1, signal.SIGUSR2]
    assert result["line"]["ev"] == "armed" and not result["already"]


def test_arming_in_the_wrong_condition_or_twice_is_refused_or_a_no_op(arm, tmp_path, monkeypatch):
    log = tmp_path / "s.jsonl"
    write(log, {"ev": "start", "pid": 1}, {"ev": "armed", "active": True, "key": True})
    monkeypatch.setattr(arm, "LOG", log)
    monkeypatch.setattr(arm, "_condition", lambda pid: "foreground")
    with pytest.raises(RuntimeError, match="foreground condition, not background"):
        arm.run({"args": {"action": "arm", "condition": "background"}})
    assert arm.run({"args": {"action": "arm"}})["already"] is True
    with pytest.raises(ValueError):
        arm.run({"args": {"action": "toggle"}})


def test_disarming_a_sentinel_that_is_gone_says_so_for_the_grade(arm, tmp_path, monkeypatch):
    log = tmp_path / "s.jsonl"
    write(log, {"ev": "start", "pid": 1}, {"ev": "armed", "active": True, "key": True})
    monkeypatch.setattr(arm, "LOG", log)
    monkeypatch.setattr(arm, "_condition", lambda pid: "foreground")

    def kill(pid, sig):
        raise ProcessLookupError

    monkeypatch.setattr(arm.os, "kill", kill)
    assert arm.run({"args": {"action": "disarm"}})["sentinel"] == "gone"


def test_the_build_script_compiles_with_the_benchs_flags_and_signs_ad_hoc():
    script = (APPS / "benchlab" / "build_app.sh").read_text()
    assert "-parse-as-library -swift-version 5 -target arm64-apple-macosx26.0" in script
    assert "codesign --force --sign -" in script and "CFBundleIdentifier" in script
    assert subprocess.run(["bash", "-n", str(APPS / "benchlab" / "build_app.sh")]).returncode == 0
    for name in ("benchlab", "bench-sentinel"):
        assert subprocess.run(["sh", "-n", str(APPS / name / "install.sh")]).returncode == 0
    assert subprocess.run(["sh", "-n", str(APPS / "cdb-s02" / "install.sh")]).returncode == 0


# -- the image's app installer ----------------------------------------------------------------------------------------


def test_the_app_installer_checks_each_download_runs_the_scripts_then_the_install_steps(tmp_path):
    import hashlib
    import importlib.util
    import shutil

    spec = importlib.util.spec_from_file_location(
        "install_app", APPS.parents[3] / "image" / "install_app.py"
    )
    installer = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(installer)
    folder = tmp_path / "benchlab"
    shutil.copytree(APPS / "benchlab", folder)
    files = json.loads((folder / "files.json").read_text())
    for f in files["files"]:
        f["sha256"] = hashlib.sha256(b"x").hexdigest()
    (folder / "files.json").write_text(json.dumps(files))
    ran = []

    def run(argv, cwd, check):
        ran.append(argv)

    done = installer.install(folder, "BenchLab", sudo=True, get=lambda url: b"x", run=run)
    assert done == [
        "download swift/BenchLab.swift",
        "download swift/BenchLabInterrupts.swift",
        "download swift/BenchLabModes.swift",
        "run install.sh",
        "step data_folder",
    ]
    assert (folder / "swift" / "BenchLab.swift").read_bytes() == b"x"
    assert ran[0] == [
        "sh",
        "install.sh",
    ]  # a session script runs as the invoking user, not through sudo
    assert ran[1][-2:] == ["app.py:BenchLab", "data_folder"] and "sudo" not in ran[1]


def test_a_download_that_doesnt_match_its_pin_stops_the_install(tmp_path):
    import importlib.util
    import shutil

    spec = importlib.util.spec_from_file_location(
        "install_app", APPS.parents[3] / "image" / "install_app.py"
    )
    installer = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(installer)
    folder = tmp_path / "bench-sentinel"
    shutil.copytree(APPS / "bench-sentinel", folder)
    with pytest.raises(SystemExit, match="not the pinned"):
        installer.install(
            folder, "BenchSentinel", get=lambda url: b"tampered", run=lambda *a, **k: None
        )
