"""Install telemetry in install.sh (python3 -m unittest discover scripts/install/tests).

Hermetic: a throwaway HOME and a loopback server that serves a fake release
and records every telemetry POST. CUA_TELEMETRY_FORBID_NETWORK=1 is set on
every run, so nothing can reach a real endpoint. The payload must carry
exactly the properties of fixtures/installer-step.json, which the Rust
privacy tests validate against the telemetry schema.
"""

import http.server
import json
import os
import subprocess
import tarfile
import tempfile
import threading
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
SCRIPT = HERE.parent / "install.sh"
MANIFEST_TOOL = HERE.parent / "release_manifest.py"
FIXTURE = json.loads((HERE / "fixtures" / "installer-step.json").read_text())
VERSION = "1.2.3"
# Values that vary per run or per host; everything else must match the fixture.
VARYING = {"product_version", "os_family", "os_major", "arch", "process_session_id",
           "step", "outcome", "install_stage", "error_kind", "items", "is_ci"}


class Recorder(http.server.SimpleHTTPRequestHandler):
    posts = []

    def log_message(self, *args):
        pass

    def do_POST(self):
        n = int(self.headers.get("Content-Length", "0"))
        Recorder.posts.append(json.loads(self.rfile.read(n)))
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b"{}")


class InstallTelemetry(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory()
        root = Path(cls.tmp.name)
        cls.release = root / "release"
        tag = cls.release / f"cua-sdk-v{VERSION}"
        tag.mkdir(parents=True)
        stage = root / "stage"
        stage.mkdir()
        cua = stage / "cua"
        cua.write_text(f'#!/bin/sh\n[ "$1" = --version ] && echo "cua {VERSION}"\nexit 0\n')
        cua.chmod(0o755)
        with tarfile.open(tag / f"cua-cli-{VERSION}-linux-x64.tar.gz", "w:gz") as t:
            t.add(cua, arcname="cua")
        handler = lambda *a, **k: Recorder(*a, directory=str(cls.release), **k)  # noqa: E731
        cls.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
        threading.Thread(target=cls.server.serve_forever, daemon=True).start()
        cls.base = f"http://127.0.0.1:{cls.server.server_address[1]}"
        subprocess.run(
            ["python3", str(MANIFEST_TOOL), "--component", "cli", "--version", VERSION,
             "--dir", str(tag), "--base-url", f"{cls.base}/cua-sdk-v{VERSION}",
             "--out", str(tag / "release-artifacts.json")],
            check=True,
        )
        cls.manifest = f"{cls.base}/cua-sdk-v{VERSION}/release-artifacts.json"

    @classmethod
    def tearDownClass(cls):
        cls.server.shutdown()
        cls.tmp.cleanup()

    def setUp(self):
        Recorder.posts.clear()
        self.home = Path(tempfile.mkdtemp(dir=self.tmp.name))

    def run_installer(self, *args, env=None, manifest=None):
        e = {
            "PATH": os.environ["PATH"],
            "HOME": str(self.home),
            "CUA_HOME": str(self.home / ".cua"),
            "CUA_INSTALL_OS": "linux",
            "CUA_INSTALL_ARCH": "x86_64",
            "CUA_INSTALL_MANIFEST_URL": manifest or self.manifest,
            "CUA_TELEMETRY_FORBID_NETWORK": "1",
            "CUA_TELEMETRY_ENDPOINT": f"{self.base}/batch/",
            "CUA_POSTHOG_KEY": "phc_test",
        }
        e.update(env or {})
        p = subprocess.run(
            ["sh", str(SCRIPT), "--yes", "--no-onboarding", "--prefix", str(self.home / "p"), *args],
            env=e, stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=120,
        )
        return p

    def events(self):
        out = []
        for body in Recorder.posts:
            self.assertEqual(body["api_key"], "phc_test")
            out.extend(body["batch"])
        return out

    def assert_matches_fixture(self, ev):
        ref = FIXTURE["events"][0]
        self.assertEqual(ev["event"], ref["event"])
        self.assertEqual(set(ev["properties"]), set(ref["properties"]))
        for k, v in ref["properties"].items():
            if k not in VARYING:
                self.assertEqual(ev["properties"][k], v, k)
        self.assertRegex(ev["distinct_id"], r"^[0-9a-f]{32}$")
        self.assertRegex(ev["properties"]["process_session_id"], r"^[0-9a-f]{32}$")
        self.assertRegex(ev["uuid"], r"^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$")
        self.assertRegex(ev["timestamp"], r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ$")

    def test_a_finished_install_sends_started_then_completed(self):
        p = self.run_installer("--cli-only")
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
        evs = self.events()
        self.assertEqual([e["properties"]["step"] for e in evs], ["installer_started", "installer_completed"])
        for e in evs:
            self.assert_matches_fixture(e)
            self.assertEqual(e["properties"]["outcome"], "ok")
            self.assertEqual(e["properties"]["install_stage"], "none")
            self.assertEqual(e["properties"]["error_kind"], "none")
            self.assertEqual(e["properties"]["items"], "cli")
        self.assertEqual(evs[1]["properties"]["product_version"], VERSION)
        # The same anonymous id the Cua programs use afterwards, owner-only.
        id_file = self.home / ".cua" / "telemetry" / "install_id"
        self.assertEqual(id_file.read_text().strip(), evs[0]["distinct_id"])
        self.assertEqual(id_file.stat().st_mode & 0o077, 0)
        self.assertEqual(evs[0]["distinct_id"], evs[1]["distinct_id"])
        self.assertIn("anonymous data about this install", p.stdout)
        self.assertIn("DO_NOT_TRACK=1", p.stdout)
        # Nothing from the machine: no home path, no user name.
        raw = json.dumps(Recorder.posts)
        self.assertNotIn(str(self.home), raw)
        self.assertNotIn(os.environ.get("USER", "nobody-here"), raw)

    def test_healthy_qemu_host_sends_completed(self):
        self.check_runtime_completion(0)

    def test_deferred_runtime_failure_sends_failed_not_completed(self):
        self.check_runtime_completion(1)

    def test_healthy_docker_host_without_qemu_sends_completed(self):
        self.check_runtime_completion(0, docker=True)

    def check_runtime_completion(self, doctor_status, docker=False):
        cli = self.home / "cua"
        cli.write_text(f'#!/bin/sh\nif [ "$1" = --version ]; then echo "cua {VERSION}"; fi\nif [ "$*" = "runtime doctor" ]; then echo "doctor status {doctor_status}"; exit {doctor_status}; fi\nexit 0\n')
        cli.chmod(0o755)
        names = ("docker",) if docker else ("qemu-img", f"qemu-system-{os.uname().machine}")
        for name in names:
            binary = self.home / name
            binary.write_text("#!/bin/sh\nexit 0\n")
            binary.chmod(0o755)
        script = self.home / "install.sh"
        script.write_text(SCRIPT.read_text().replace(
            'main "$@"',
            f'install_cli() {{ cua_bin="{cli}"; }}\ninstall_app_linux() {{ :; }}\nmain "$@"',
        ))
        e = dict(os.environ, HOME=str(self.home), CUA_HOME=str(self.home / ".cua"),
                 PATH=f'{self.home}:{os.environ["PATH"]}',
                 CUA_INSTALL_OS="linux", CUA_INSTALL_ARCH="x86_64",
                 CUA_INSTALL_MANIFEST_URL=self.manifest,
                 CUA_TELEMETRY_FORBID_NETWORK="1",
                 CUA_TELEMETRY_ENDPOINT=f"{self.base}/batch/", CUA_POSTHOG_KEY="phc_test")
        if docker:
            self.assertFalse((self.home / "qemu-img").exists())
            script.write_text(script.read_text().replace(
                'main "$@"',
                'have() { case "$1" in qemu-img|qemu-system-*) return 1;; *) command -v "$1" >/dev/null 2>&1;; esac; }\nmain "$@"',
            ))
        p = subprocess.run(["sh", str(script), "--yes", "--no-onboarding", "--select", "spaces"],
                           env=e, capture_output=True, text=True, timeout=120)
        self.assertEqual(p.returncode, doctor_status, p.stdout + p.stderr)
        self.assertIn(f"doctor status {doctor_status}", p.stdout)
        self.assertIn("Next:", p.stdout)
        evs = self.events()
        self.assertEqual([e["properties"]["step"] for e in evs],
                         ["installer_started", "installer_failed" if doctor_status else "installer_completed"])
        self.assertEqual(evs[-1]["properties"]["install_stage"], "runtime" if doctor_status else "none")

    def test_an_existing_install_id_is_kept(self):
        d = self.home / ".cua" / "telemetry"
        d.mkdir(parents=True)
        (d / "install_id").write_text("0123456789abcdef0123456789abcdef")
        self.run_installer("--cli-only")
        self.assertTrue(self.events())
        for e in self.events():
            self.assertEqual(e["distinct_id"], "0123456789abcdef0123456789abcdef")

    def test_a_failed_install_says_where_and_why_never_the_message(self):
        p = self.run_installer("--cli-only", manifest=f"{self.base}/no-such-release/release-artifacts.json")
        self.assertNotEqual(p.returncode, 0)
        evs = self.events()
        self.assertEqual([e["properties"]["step"] for e in evs], ["installer_started", "installer_failed"])
        failed = evs[1]["properties"]
        self.assertEqual((failed["outcome"], failed["install_stage"], failed["error_kind"]),
                         ("error", "manifest", "network"))
        self.assert_matches_fixture(evs[1])
        self.assertNotIn("no-such-release", json.dumps(Recorder.posts))

    def test_every_switch_turns_it_off(self):
        cfg = self.home / ".cua"
        cases = [
            {"DO_NOT_TRACK": "1"},
            {"DO_NOT_TRACK": "1", "CUA_TELEMETRY": "1"},
            {"CUA_TELEMETRY": "0"},
            {"CUA_TELEMETRY_ENABLED": "false"},
            {"CUA_TELEMETRY_DISABLED": "1"},
            {"CI": "true"},
            {"GITHUB_ACTIONS": "true"},
            "config",
            "dry-run",
            # The test guard: a real endpoint is refused before anything happens.
            {"CUA_TELEMETRY_ENDPOINT": "https://eu.i.posthog.com/batch/"},
        ]
        for case in cases:
            with self.subTest(case=case):
                Recorder.posts.clear()
                self.home = Path(tempfile.mkdtemp(dir=self.tmp.name))
                cfg = self.home / ".cua"
                args, env = ["--cli-only"], {}
                if case == "config":
                    cfg.mkdir(parents=True)
                    (cfg / "config.toml").write_text('[other]\nenabled = "on"\n\n[telemetry]\nenabled = "off"\n')
                elif case == "dry-run":
                    args.append("--dry-run")
                else:
                    env = case
                p = self.run_installer(*args, env=env)
                self.assertEqual(Recorder.posts, [], p.stdout + p.stderr)
                self.assertNotIn("anonymous data about this install", p.stdout)
                self.assertFalse((cfg / "telemetry" / "install_id").exists())

    def test_ci_with_cua_telemetry_on_sends_and_is_marked(self):
        p = self.run_installer("--cli-only", env={"CI": "true", "CUA_TELEMETRY": "1"})
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
        evs = self.events()
        self.assertEqual(len(evs), 2)
        self.assertTrue(all(e["properties"]["is_ci"] is True for e in evs))

    def test_staging_sends_to_the_staging_project(self):
        self.run_installer("--cli-only", env={"CUA_ENV": "staging", "CUA_POSTHOG_KEY": ""})
        self.assertEqual(len(Recorder.posts), 2)
        for body in Recorder.posts:
            self.assertTrue(body["api_key"].startswith("phc_xDNL"), "staging project key")

    def test_a_config_file_on_wins_over_ci(self):
        cfg = self.home / ".cua"
        cfg.mkdir(parents=True)
        (cfg / "config.toml").write_text('[telemetry]\nenabled = true\n')
        self.run_installer("--cli-only", env={"CI": "1"})
        self.assertEqual(len(self.events()), 2)


if __name__ == "__main__":
    unittest.main()
