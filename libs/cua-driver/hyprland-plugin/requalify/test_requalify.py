"""Unit and dry-run tests for the Hyprland plugin requalification automation."""

import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest import mock
import urllib.error

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[3]
sys.path.insert(0, str(HERE))

import channels  # noqa: E402
import headless  # noqa: E402
import manifest  # noqa: E402
import qualify  # noqa: E402

EDGE_PINS = {"hyprland": "0.56.2-4", "aquamarine": "0.15.1-1", "glibc": "2.44-1", "hyprcursor": "0.1.13-7",
             "hyprgraphics": "0.5.1-4", "hyprlang": "0.6.8-5", "hyprutils": "0.14.2-1", "libgcc": "16.2.1-1",
             "libstdc++": "16.2.1-1", "libxkbcommon": "1.13.2-1", "wayland": "1.26.0-1"}


def desc(name, version, sha="0" * 64, depends=(), filename=None):
    lines = ["%FILENAME%", filename or f"{name}-{version}-x86_64.pkg.tar.zst", "", "%NAME%", name, "",
             "%VERSION%", version, "", "%SHA256SUM%", sha, "", "%BUILDDATE%", "1760000000", ""]
    if depends:
        lines += ["%DEPENDS%", *depends, ""]
    return "\n".join(lines) + "\n"


def database(packages):
    """A gzip pacman sync database from {name: (version, sha[, depends])}."""
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w:gz") as archive:
        for name, spec in packages.items():
            version, sha = spec[0], spec[1]
            depends = spec[2] if len(spec) > 2 else ()
            data = desc(name, version, sha, depends).encode()
            info = tarfile.TarInfo(f"{name}-{version}/desc")
            info.size = len(data)
            archive.addfile(info, io.BytesIO(data))
    return buffer.getvalue()


def tracked(hyprland="0.56.2-4", sha="a" * 64, **overrides):
    packages = {name: (EDGE_PINS.get(name, "16.2.1-1"), "b" * 64) for name in channels.TRACKED}
    packages["hyprland"] = (hyprland, sha)
    packages.update(overrides)
    return packages


def split(packages):
    core_names = {"glibc", "gcc", "libgcc", "libstdc++"}
    core = {k: v for k, v in packages.items() if k in core_names}
    extra = {k: v for k, v in packages.items() if k not in core_names}
    return core, extra


def omarchy_db(plugin_version="0.32.0-3", pins=EDGE_PINS, extra=None):
    packages = {"cua-hyprland-plugin": (plugin_version, "c" * 64,
                                        [f"{k}={v}" for k, v in pins.items()] + ["python>=3.11", "libfoo.so=1-64"]),
                "cua-driver-bin": ("0.33.4-1", "d" * 64), "omarchy-keyring": ("20251027-1", "e" * 64)}
    packages.update(extra or {})
    return database(packages)


class FakeMirrors:
    """Serves databases by URL; any other URL raises like a down mirror."""

    def __init__(self, routes):
        self.routes = routes
        self.calls = []

    def __call__(self, url, headers=None):
        self.calls.append(url)
        for prefix, data in self.routes.items():
            if url.startswith(prefix):
                return data
        raise urllib.error.URLError(f"no route for {url}")


def mirrors(arch=None, edge=None, rc=None, releases=None):
    arch_core, arch_extra = split(arch or tracked())
    edge_core, edge_extra = split(edge or tracked())
    rc_core, rc_extra = split(rc or tracked(hyprland="0.56.2-2", sha="f" * 64))
    routes = {
        "https://geo.mirror.pkgbuild.com/core/": database(arch_core),
        "https://geo.mirror.pkgbuild.com/extra/": database(arch_extra),
        "https://mirror.omarchy.org/core/": database(edge_core),
        "https://mirror.omarchy.org/extra/": database(edge_extra),
        "https://pkgs.omarchy.org/edge/": omarchy_db(),
        "https://rc-mirror.omarchy.org/core/": database(rc_core),
        "https://rc-mirror.omarchy.org/extra/": database(rc_extra),
        "https://pkgs.omarchy.org/rc/": omarchy_db(extra={"cua-hyprland-plugin": ("0.28.2-2", "c" * 64)}),
        channels.HYPRLAND_RELEASES: json.dumps(releases if releases is not None else [
            {"tag_name": "v0.56.2", "draft": False, "prerelease": False, "html_url": "u"},
            {"tag_name": "v0.57.0-rc1", "draft": False, "prerelease": True}]).encode(),
    }
    return FakeMirrors(routes)


class DatabaseTest(unittest.TestCase):
    def test_parse_db_reads_version_sha_depends_and_filename(self):
        parsed = channels.parse_db(database({"hyprland": ("0.56.2-4", "a" * 64, ["aquamarine=0.15.1-1"])}))
        self.assertEqual(parsed["hyprland"]["version"], "0.56.2-4")
        self.assertEqual(parsed["hyprland"]["sha256"], "a" * 64)
        self.assertEqual(parsed["hyprland"]["depends"], ["aquamarine=0.15.1-1"])
        self.assertEqual(parsed["hyprland"]["filename"], "hyprland-0.56.2-4-x86_64.pkg.tar.zst")

    @unittest.skipUnless(shutil.which("zstd"), "zstd not installed")
    def test_parse_db_reads_zstd_databases_like_omarchy(self):
        compressed = subprocess.run(["zstd", "-q", "-c"], input=channels.decompress(
            subprocess.run(["gzip", "-dc"], input=database({"a": ("1-1", "0" * 64)}), capture_output=True).stdout),
            capture_output=True, check=True).stdout
        self.assertTrue(compressed.startswith(channels.ZSTD_MAGIC))
        self.assertEqual(channels.parse_db(compressed)["a"]["version"], "1-1")

    def test_exact_pins_skip_ranges_and_soname_provides(self):
        self.assertEqual(channels.exact_pins(["hyprland=0.56.2-4", "python>=3.11", "libfoo.so=1-64", "binutils"]),
                         {"hyprland": "0.56.2-4"})

    def test_header_version_drops_epoch_and_pkgrel(self):
        self.assertEqual(channels.header_version("0.56.2-4"), "0.56.2")
        self.assertEqual(channels.header_version("1:0.57.0-1.1"), "0.57.0")


class FetchTest(unittest.TestCase):
    def test_retries_server_errors_with_backoff(self):
        attempts, sleeps = [], []

        class Response(io.BytesIO):
            def __enter__(self):
                return self

            def __exit__(self, *exc):
                return False

        def opener(request, timeout):
            attempts.append(request.full_url)
            if len(attempts) < 3:
                raise urllib.error.HTTPError(request.full_url, 503, "busy", {}, None)
            return Response(b"ok")

        self.assertEqual(channels.fetch("https://x/db", opener=opener, sleep=sleeps.append), b"ok")
        self.assertEqual(sleeps, [2.0, 4.0])

    def test_does_not_retry_not_found(self):
        def opener(request, timeout):
            raise urllib.error.HTTPError(request.full_url, 404, "missing", {}, None)

        sleeps = []
        with self.assertRaises(urllib.error.HTTPError):
            channels.fetch("https://x/db", opener=opener, sleep=sleeps.append)
        self.assertEqual(sleeps, [])


class DetectTest(unittest.TestCase):
    def test_detect_resolves_channels_and_records_a_down_mirror(self):
        snapshot = channels.detect(fetcher=mirrors())
        # stable has no route: recorded, not fatal.
        self.assertIn("omarchy-stable", snapshot["errors"])
        self.assertEqual(set(snapshot["channels"]), {"arch", "omarchy-edge", "omarchy-rc"})
        self.assertEqual(snapshot["channels"]["arch"]["abi_key"], snapshot["channels"]["omarchy-edge"]["abi_key"])
        self.assertNotEqual(snapshot["channels"]["arch"]["abi_key"], snapshot["channels"]["omarchy-rc"]["abi_key"])
        edge = snapshot["channels"]["omarchy-edge"]
        self.assertTrue(edge["omarchy_plugin"]["installable"])
        self.assertEqual(edge["omarchy_driver"]["version"], "0.33.4-1")
        self.assertEqual(snapshot["upstream"]["version"], "0.56.2")

    def test_same_version_rebuild_is_a_new_build(self):
        before = channels.detect(["arch"], fetcher=mirrors())["channels"]["arch"]["abi_key"]
        rebuilt = channels.detect(["arch"], fetcher=mirrors(arch=tracked(sha="9" * 64)))["channels"]["arch"]["abi_key"]
        self.assertNotEqual(before, rebuilt)

    def test_pkgrel_bump_breaks_omarchy_pins(self):
        snapshot = channels.detect(["omarchy-edge"], fetcher=mirrors(edge=tracked(hyprland="0.56.2-5")))
        plugin = snapshot["channels"]["omarchy-edge"]["omarchy_plugin"]
        self.assertFalse(plugin["installable"])
        self.assertEqual(plugin["broken_pins"], {"hyprland": {"pinned": "0.56.2-4", "channel": "0.56.2-5"}})

    def test_omarchy_repo_wins_first_in_rc_order(self):
        rc_override = {"https://pkgs.omarchy.org/rc/": omarchy_db(extra={"hyprland": ("0.56.2-9", "e" * 64)})}
        fake = mirrors()
        fake.routes.update(rc_override)
        rc = channels.detect(["omarchy-rc"], fetcher=fake)["channels"]["omarchy-rc"]
        self.assertEqual(rc["packages"]["hyprland"]["version"], "0.56.2-9")
        self.assertTrue(rc["needs_omarchy_repo"])

    def test_missing_tracked_package_is_an_error(self):
        packages = tracked()
        del packages["aquamarine"]
        snapshot = channels.detect(["arch"], fetcher=mirrors(arch=packages))
        self.assertIn("aquamarine", snapshot["errors"]["arch"])

    def test_upstream_ahead_of_every_package(self):
        snapshot = channels.detect(["arch"], fetcher=mirrors(releases=[
            {"tag_name": "v0.57.0", "draft": False, "prerelease": False}]))
        self.assertTrue(channels.upstream_status(snapshot)["ahead_of_packages"])
        current = channels.detect(["arch"], fetcher=mirrors())
        self.assertFalse(channels.upstream_status(current)["ahead_of_packages"])


SOURCES = [{"ref": "main", "commit": "1" * 40, "tree": "a" * 40, "driver_version": ""},
           {"ref": "cua-driver-rs-v0.34.0", "commit": "2" * 40, "tree": "a" * 40, "driver_version": "0.34.0"},
           {"ref": "cua-driver-rs-v0.32.0", "commit": "3" * 40, "tree": "b" * 40, "driver_version": "0.32.0"}]


class PlanTest(unittest.TestCase):
    def setUp(self):
        self.snapshot = channels.detect(fetcher=mirrors())

    def test_one_build_per_abi_key_and_tree(self):
        include = channels.plan(self.snapshot, SOURCES)
        self.assertEqual(len(include), 4)  # 2 ABI keys x 2 distinct trees.
        main = [j for j in include if j["tree"] == "a" * 40]
        self.assertTrue(all(j["refs"] == ["main", "cua-driver-rs-v0.34.0"] for j in main))
        edge = [j for j in include if "omarchy-edge" in j["channels"]][0]
        self.assertEqual(edge["channel"], "arch")  # Shared key, built without the Omarchy repo.
        self.assertEqual(edge["expected_packages"]["hyprland"], "0.56.2-4")

    def test_finished_combinations_are_skipped_unless_forced(self):
        key = self.snapshot["channels"]["arch"]["abi_key"]
        previous = {"entries": [{"abi_key": key, "plugin": {"tree": "a" * 40}, "status": "pass"},
                                {"abi_key": key, "plugin": {"tree": "b" * 40}, "status": "build-only"}]}
        include = channels.plan(self.snapshot, SOURCES, previous)
        self.assertEqual(len(include), 3)  # build-only is retried.
        self.assertEqual(len(channels.plan(self.snapshot, SOURCES, previous, force=True)), 4)


@unittest.skipUnless(shutil.which("git"), "git not installed")
class SourcesTest(unittest.TestCase):
    def test_main_latest_tag_and_omarchy_packaged_source(self):
        with tempfile.TemporaryDirectory() as directory:
            repo = Path(directory)
            env = dict(os.environ, GIT_AUTHOR_NAME="t", GIT_AUTHOR_EMAIL="t@t", GIT_COMMITTER_NAME="t",
                       GIT_COMMITTER_EMAIL="t@t")

            def git(*args):
                subprocess.run(["git", "-C", str(repo), *args], check=True, capture_output=True, env=env)

            git("init", "-q")
            plugin = repo / channels.PLUGIN_PATH
            plugin.mkdir(parents=True)
            for version in ("0.32.0", "0.34.0", "0.9.0"):
                (plugin / "VERSION").write_text(version)
                git("add", "-A")
                git("commit", "-qm", version)
                git("tag", f"cua-driver-rs-v{version}")
            git("tag", "cua-driver-rs-v0.35.0-rc.1")
            snapshot = {"channels": {"omarchy-edge": {"omarchy_plugin": {"version": "0.32.0-3"}}}}
            found = channels.plugin_sources(repo, snapshot)
        self.assertEqual([s["ref"] for s in found], ["main", "cua-driver-rs-v0.34.0", "cua-driver-rs-v0.32.0"])
        self.assertEqual(found[1]["driver_version"], "0.34.0")
        self.assertTrue(all(len(s["tree"]) == 40 for s in found))

    def test_docs_and_tooling_do_not_change_the_source_id(self):
        with tempfile.TemporaryDirectory() as directory:
            repo = Path(directory)
            env = dict(os.environ, GIT_AUTHOR_NAME="t", GIT_AUTHOR_EMAIL="t@t", GIT_COMMITTER_NAME="t",
                       GIT_COMMITTER_EMAIL="t@t")

            def git(*args):
                return subprocess.run(["git", "-C", str(repo), *args], check=True, capture_output=True,
                                      env=env, text=True).stdout.strip()

            plugin = repo / channels.PLUGIN_PATH
            (plugin / "src").mkdir(parents=True)
            git("init", "-q")
            (plugin / "src" / "plugin.cpp").write_text("1")
            git("add", "-A")
            git("commit", "-qm", "a")
            first = channels.source_id(repo, git("rev-parse", "HEAD"))
            for name in ("requalify/x.py", "docs/y.md", "README.md"):
                (plugin / name).parent.mkdir(parents=True, exist_ok=True)
                (plugin / name).write_text("doc")
            git("add", "-A")
            git("commit", "-qm", "b")
            self.assertEqual(channels.source_id(repo, git("rev-parse", "HEAD")), first)
            (plugin / "src" / "plugin.cpp").write_text("2")
            git("commit", "-qam", "c")
            self.assertNotEqual(channels.source_id(repo, git("rev-parse", "HEAD")), first)


class QualifyTest(unittest.TestCase):
    def test_pacman_conf_keeps_channel_order(self):
        conf = qualify.pacman_conf("omarchy-rc", with_omarchy=True)
        self.assertLess(conf.index("[omarchy]"), conf.index("[core]"))
        self.assertIn("https://rc-mirror.omarchy.org/$repo/os/$arch", conf)
        self.assertNotIn("[omarchy]", qualify.pacman_conf("omarchy-edge", with_omarchy=False))

    def test_overall_status(self):
        passing = {"build": {"status": "pass"}, "ctest": {"status": "pass"},
                   "load": {"status": "pass"}, "smoke": {"status": "pass"}}
        self.assertEqual(qualify.overall(passing), "pass")
        self.assertEqual(qualify.overall({**passing, "load": {"status": "unavailable"},
                                          "smoke": {"status": "unavailable"}}), "build-only")
        self.assertEqual(qualify.overall({**passing, "smoke": {"status": "fail"}}), "fail")
        self.assertEqual(qualify.overall({**passing, "ctest": {"status": "fail"}}), "fail")
        self.assertEqual(qualify.overall({**passing, "build": {"status": "fail"}}), "fail")

    def test_ctest_summary_parsing(self):
        output = ("The following tests FAILED:\n\t  7 - cua_hyprland_status_test (Failed)\n"
                  "95% tests passed, 1 tests failed out of 20\n")
        with tempfile.TemporaryDirectory() as directory, \
                mock.patch.object(qualify, "run", return_value=(8, output)):
            result = qualify.ctest(Path(directory))
        self.assertEqual((result["status"], result["total"], result["failed"]), ("fail", 20, 1))
        self.assertEqual(result["failures"], ["cua_hyprland_status_test"])

    def test_no_drm_device_is_unavailable_not_failure(self):
        with mock.patch.object(qualify, "drm_cards", return_value=[]):
            load, smoke = qualify.session(Path("/nonexistent.so"), Path("/nonexistent"))
        self.assertEqual((load["status"], smoke["status"]), ("unavailable", "unavailable"))


class InputLaneTest(unittest.TestCase):
    def test_packets_follow_protocol_v3(self):
        lane = headless.InputLane.__new__(headless.InputLane)
        lane.sequence, lane.transcript, sent = 0, [], []
        lane.request = lambda packet: sent.append(packet) or {"ok": True}
        lane.target({"pid": 42, "address": "0x55aa"})
        lane.key({"target": "f" * 32, "revision": 1}, 46)
        lane.key({"target": "f" * 32, "revision": 2}, 28)
        self.assertEqual(sent, ["TARGET 42 55aa 2", f"KEY 1 {'f' * 32} 1 46 0", f"KEY 2 {'f' * 32} 2 28 0"])


def record(job, status, **checks):
    base = {"build": {"status": "pass", "module_sha256": "m" * 64, "options": qualify.BUILD_OPTIONS},
            "ctest": {"status": "pass", "total": 20, "failed": 0}, "load": {"status": "pass"},
            "smoke": {"status": "pass"}}
    base.update(checks)
    return {"job": job, "status": status, "finished_at": "2026-10-09T07:00:00Z",
            "measured": {"hyprland": {"package_version": job["hyprland"], "header_version": "0.56.2",
                                      "sha256": "h" * 64, "headers_sha256": "i" * 64},
                         "compiler": {"version": "16.2.1 20260810"}}, **base}


class DryRunTest(unittest.TestCase):
    """detect -> plan -> (simulated container results) -> manifest -> issue, with no network or Docker."""

    def run_pipeline(self, fake, previous=None, statuses=None):
        snapshot = channels.detect(fetcher=fake)
        snapshot["upstream"] = channels.upstream_status(snapshot)
        include = channels.plan(snapshot, SOURCES, previous)
        statuses = statuses or {}
        records = [record(job, statuses.get(job["tree"], "pass"),
                          **({"smoke": {"status": "fail", "reason": "target wrote nothing"}}
                             if statuses.get(job["tree"]) == "fail" else {}))
                   for job in include]
        return channels, manifest.merge(previous, snapshot, records, "https://run", now="2026-10-09T07:00:00Z")

    def test_all_pass_publishes_a_clean_manifest(self):
        _, (result, fresh) = self.run_pipeline(mirrors())
        self.assertEqual(len(fresh), 4)
        self.assertEqual(manifest.failures(result, fresh), [])
        edge = result["current"]["omarchy-edge"]
        self.assertEqual(edge["cua-driver-rs-v0.32.0"]["status"], "pass")
        self.assertEqual(set(edge), {"main", "cua-driver-rs-v0.34.0", "cua-driver-rs-v0.32.0"})
        table = manifest.matrix(result, fresh)
        self.assertIn("| omarchy-edge | 0.56.2-4 |", table)
        self.assertIn("pass (20/20)", table)
        json.dumps(result)  # Serializable.

    def test_failure_and_broken_pins_open_an_issue(self):
        fake = mirrors(edge=tracked(hyprland="0.56.2-5"))
        _, (result, fresh) = self.run_pipeline(fake, statuses={"b" * 40: "fail"})
        problems = manifest.failures(result, fresh)
        self.assertTrue(any("cua-driver-rs-v0.32.0" in p and "smoke" in p for p in problems))
        self.assertTrue(any("omarchy-edge" in p and "no longer installs" in p for p in problems))
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "failures.md"
            path.write_text("\n".join(problems) + "\n")
            out = subprocess.run([str(HERE / "report_failure.sh"), "https://run", str(path)], capture_output=True,
                                 text=True, env=dict(os.environ, DRY_RUN="1"), check=True).stdout
        self.assertIn("gh issue create --title Hyprland\\ plugin\\ requalification\\ failing", out)
        self.assertIn("hyprland-requalify", out)

    def test_second_run_skips_finished_builds_and_keeps_history(self):
        _, (first, _) = self.run_pipeline(mirrors())
        snapshot = channels.detect(fetcher=mirrors())
        self.assertEqual(channels.plan(snapshot, SOURCES, first), [])
        _, (second, fresh) = self.run_pipeline(mirrors(arch=tracked(sha="9" * 64)), previous=first)
        self.assertEqual(len(fresh), 2)  # Only the rebuilt Arch key.
        self.assertEqual(len(second["entries"]), 6)

    def test_main_writes_outputs(self):
        _, (result, _) = self.run_pipeline(mirrors())
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            snapshot = channels.detect(fetcher=mirrors())
            (base / "snapshot.json").write_text(json.dumps(snapshot))
            results = base / "results" / "requalify-result-x"
            (results / "session").mkdir(parents=True)
            job = channels.plan(snapshot, SOURCES)[0]
            (results / "result.json").write_text(json.dumps(record(job, "pass")))
            (results / "session" / "result.json").write_text("{}")
            env = dict(os.environ, GITHUB_OUTPUT=str(base / "output"))
            subprocess.run([sys.executable, str(HERE / "manifest.py"), "--snapshot", str(base / "snapshot.json"),
                            "--results", str(base / "results"), "--out", str(base / "compat")],
                           check=True, env=env, capture_output=True)
            written = json.loads((base / "compat" / "compatibility.json").read_text())
            self.assertEqual(len(written["entries"]), 1)
            self.assertIn("failed=false", (base / "output").read_text())
            self.assertFalse((base / "compat" / "failures.md").exists())


class WorkflowContractTest(unittest.TestCase):
    def setUp(self):
        self.workflow = (ROOT / ".github/workflows/nightly-hyprland-plugin-requalify.yml").read_text()
        self.code = "".join(line for line in self.workflow.splitlines(keepends=True)
                            if not line.lstrip().startswith("#"))

    def test_daily_and_manual(self):
        self.assertIn("  schedule:\n    # Daily", self.workflow)
        self.assertIn("  workflow_dispatch:", self.workflow)

    def test_never_publishes_packages_or_touches_omarchy(self):
        for forbidden in ("packages: write", "gh release", "softprops/", "omacom/", "docker push", "makepkg"):
            self.assertNotIn(forbidden, self.code)
        pushes = [line.strip() for line in self.code.splitlines() if "push" in line and "git" in line]
        self.assertEqual(pushes, ['git -C compat-branch push --quiet origin "HEAD:refs/heads/$COMPAT_BRANCH"'])

    def test_pull_requests_never_file_issues_or_push(self):
        self.assertIn("DRY_RUN: ${{ github.event_name == 'pull_request' && '1' || '0' }}", self.workflow)
        publish = self.workflow.split("- name: Commit the manifest", 1)[1].split("run:", 1)[0]
        self.assertIn("github.ref == 'refs/heads/main'", publish)
        self.assertNotIn("pull_request", publish)


if __name__ == "__main__":
    unittest.main()
