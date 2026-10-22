"""Tests for uninstall.sh (python3 -m unittest discover scripts/install/tests).

Hermetic, in the style of libs/cua-driver/scripts/tests/test_uninstall_*.py: a
throwaway HOME, a PATH that holds only symlinks to a few real utilities plus
recording fakes (cua, launchctl, systemctl, security, ...), and an `rm` that
refuses anything outside the sandbox. Nothing here touches the real home
folder, /Applications, the real Keychain or a real service.
"""

import http.server
import os
import shlex
import shutil
import subprocess
import sys
import tempfile
import threading
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent.parent
SCRIPT = HERE / "uninstall.sh"

# Real utilities the script needs; everything else is a fake or absent.
REAL_TOOLS = (
    "awk", "cat", "cmp", "curl", "cut", "dirname", "env", "grep", "head", "kill", "ls", "mkdir",
    "mktemp", "printf", "readlink", "rmdir", "sed", "sort", "tr", "wc", "bash",
)

FAKE_CUA = """#!/bin/sh
printf 'cua[%s]: %s\\n' "${CUA_HOME:-}" "$*" >>"$FAKE_CALLS"
case "$*" in
"--version") echo "cua 1.2.3 (test)" ;;
"sandbox ls --local --no-size")
    if [ -f "$FAKE_SPACES" ]; then cat "$FAKE_SPACES"; else echo "No sandboxes found."; fi
    ;;
"host remove --force --uninstalling") rm -rf "${CUA_HOME:-$HOME/.cua}/host" ;;
esac
exit 0
"""


def write_executable(path, body):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(body, encoding="utf-8")
    path.chmod(0o755)


class Sandbox:
    """A fake computer: HOME, a recording PATH and an rm that stays inside."""

    def __init__(self, root, os_name="Darwin", shell="/bin/sh"):
        self.root = Path(root).resolve()
        self.home = self.root / "home"
        self.bin = self.root / "bin"
        self.calls = self.root / "calls.log"
        self.spaces = self.root / "spaces.txt"
        self.shell = shell
        self.os_name = os_name
        self.home.mkdir()
        (self.root / "tmp").mkdir()
        self.calls.write_text("")
        for tool in REAL_TOOLS:
            real = shutil.which(tool)
            if real:
                (self.bin / tool).parent.mkdir(parents=True, exist_ok=True)
                (self.bin / tool).symlink_to(real)
        # rm keeps real semantics but may only act inside the sandbox.
        guard = self.root / "safe_rm.py"
        guard.write_text(
            "import os, pathlib, sys\n"
            f"root = pathlib.Path({str(self.root)!r})\n"
            "for arg in sys.argv[1:]:\n"
            "    if arg.startswith('-'): continue\n"
            "    parent = pathlib.Path(os.path.abspath(arg)).parent\n"
            "    assert root == parent or root in parent.parents, 'rm outside the sandbox: ' + arg\n"
            "os.execv('/bin/rm', ['/bin/rm', *sys.argv[1:]])\n"
        )
        write_executable(self.bin / "rm", f"#!/bin/sh\nexec {shlex.quote(sys.executable)} {shlex.quote(str(guard))} \"$@\"\n")
        # Records every sleep: the timeout watchdog must never park a long one.
        write_executable(self.bin / "sleep", "#!/bin/sh\nprintf 'sleep: %s\\n' \"$*\" >>\"$FAKE_CALLS\"\nexec " + shlex.quote(shutil.which("sleep")) + " \"$@\"\n")
        write_executable(self.bin / "id", '#!/bin/sh\nif [ "${1:-}" = -u ]; then echo "${FAKE_UID:-1000}"; fi\n')
        write_executable(self.bin / "pgrep", "#!/bin/sh\nexit 1\n")
        for name in (
            "pkill", "launchctl", "systemctl", "security", "osascript", "defaults", "umount",
            "diskutil", "pkgutil", "apt-get", "sudo", "secret-tool", "fusermount", "tccutil",
        ):
            self.fake_recording(name)
        write_executable(self.bin / "mount", "#!/bin/sh\ncat \"${FAKE_MOUNTS:-/dev/null}\"\n")
        self.fake_security()
        write_executable(self.bin / "cua", FAKE_CUA)
        self.env = {
            "HOME": str(self.home),
            "PATH": str(self.bin),
            "TMPDIR": str(self.root / "tmp"),
            # The macOS system python writes bytecode caches under $HOME.
            "PYTHONDONTWRITEBYTECODE": "1",
            "FAKE_CALLS": str(self.calls),
            "FAKE_SPACES": str(self.spaces),
            "CUA_UNINSTALL_OS": os_name,
            "CUA_UNINSTALL_NONINTERACTIVE": "1",
            # /Applications, /usr/local/bin, /opt and /etc live in here.
            "CUA_UNINSTALL_ROOT": str(self.root / "sys"),
        }
        self.sys = self.root / "sys"

    def fake_recording(self, name):
        write_executable(self.bin / name, f"#!/bin/sh\nprintf '{name}: %s\\n' \"$*\" >>\"$FAKE_CALLS\"\nexit 0\n")

    def fake_security(self):
        """A Keychain with items listed in keychain.txt as `service|account` lines."""
        store = self.root / "keychain.txt"
        store.write_text("")
        write_executable(
            self.bin / "security",
            f"""#!/bin/sh
store={shlex.quote(str(store))}
printf 'security: %s\\n' "$*" >>"$FAKE_CALLS"
verb="$1"; shift
service=""; account=""
while [ $# -gt 0 ]; do
    case "$1" in
    -s) service="$2"; shift ;;
    -a) account="$2"; shift ;;
    esac
    shift
done
match() {{ awk -F'|' -v s="$service" -v a="$account" '$1 == s && (a == "" || $2 == a) {{ print; exit }}' "$store"; }}
case "$verb" in
find-generic-password) [ -n "$(match)" ] ;;
delete-generic-password)
    line="$(match)"
    [ -n "$line" ] || exit 44
    grep -vxF "$line" "$store" >"$store.new" || true
    cat "$store.new" >"$store"
    ;;
esac
""",
        )
        self.keychain = store

    # -- fixtures -------------------------------------------------------------
    def put(self, relative, text="x\n", mode=None):
        path = self.home / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
        if mode is not None:
            path.chmod(mode)
        return path

    def put_sys(self, relative, text="x\n", mode=None):
        path = self.sys / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
        if mode is not None:
            path.chmod(mode)
        return path

    def cli(self, relative=".local/bin/cua"):
        path = self.home / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy(self.bin / "cua", path)
        return path

    def keychain_add(self, *items):
        with open(self.keychain, "a") as fh:
            for item in items:
                fh.write(item + "\n")

    def keychain_items(self):
        return [line for line in self.keychain.read_text().splitlines() if line]

    def run(self, *args, env=None, shell=None):
        e = dict(self.env)
        e.update(env or {})
        return subprocess.run(
            [shell or self.shell, str(SCRIPT), *args],
            env=e, text=True, capture_output=True, cwd=str(self.root), check=False,
        )

    def call_log(self):
        return self.calls.read_text().splitlines()

    def index(self, fragment):
        for i, line in enumerate(self.call_log()):
            if fragment in line:
                return i
        return -1

    def exists(self, relative):
        path = self.home / relative
        return path.exists() or path.is_symlink()


def shells():
    found = ["/bin/sh"]
    dash = shutil.which("dash")
    if dash and Path(dash).resolve() != Path("/bin/sh").resolve():
        found.append(dash)
    return found


PATH_BLOCK = '\n# Added by the cua installer\nexport PATH="{home}/.local/bin:$PATH"\n'


def installed_mac(box):
    """Everything install.sh and the Cua Spaces app leave on a Mac (sandboxed)."""
    box.put("Applications/Cua Spaces.app/Contents/MacOS/CuaSpacesMac", "app\n", 0o755)
    box.cli()
    box.put(".zshrc", "alias a=b\n" + PATH_BLOCK.format(home=box.home) + "export KEEP=1\n")
    box.put("Library/LaunchAgents/com.trycua.spacesd.host.plist", "plist\n")
    box.put(".cua/host/config.json", "{}\n")
    box.put(".cua/host/bin/cua-spacesd", "bin\n")
    box.put(".cua/config.toml", "default.on = 'local'\n")
    box.put(".cua/credentials.json", "{}\n")
    box.put(".cua/cache/blob", "blob\n")
    box.put(".cua/telemetry/install_channel", "install_script\n")
    box.put(".cua/volume/cache/block", "cached\n")
    box.put(".cua/volume/data/notes.txt", "my notes\n")
    box.put(".cua/sandboxes/studio/disk.img", "disk\n")
    box.put(".cua/vmm/state", "vm\n")
    box.put("Library/Application Support/com.trycua.spaces.macos/settings.json", "{}\n")
    box.put("Library/Caches/com.trycua.spaces.macos/c", "cache\n")
    box.put("Library/Preferences/com.trycua.spaces.macos.plist", "prefs\n")
    box.keychain_add("run.cua.ai|cua-cli", "run.cua.ai|cua-cli.device-key", "run.cua.ai|cua-cli.agent-keys",
                     "Cua Keyvault|vault-1", "Cua Keyvault Generation|vault-1", "Other App|me")


class UninstallMacTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)

    def box(self, **kw):
        return Sandbox(self.tmp.name, "Darwin", **kw)

    def test_dry_run_prints_the_plan_and_changes_nothing(self):
        for shell in shells():
            with self.subTest(shell=shell):
                box = Sandbox(tempfile.mkdtemp(dir=self.tmp.name), "Darwin", shell=shell)
                installed_mac(box)
                before = sorted(str(p) for p in box.home.rglob("*"))
                zshrc = (box.home / ".zshrc").read_text()
                result = box.run("--dry-run", "--all", "--delete-spaces")
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertIn("[dry-run] remove", result.stdout)
                self.assertIn("cua host remove --force --uninstalling", result.stdout)
                self.assertIn("[dry-run] bash lume-uninstall.sh --force --purge", result.stdout)
                self.assertEqual(before, sorted(str(p) for p in box.home.rglob("*")))
                self.assertEqual(zshrc, (box.home / ".zshrc").read_text())
                self.assertEqual(len(box.keychain_items()), 6)
                log = "\n".join(box.call_log())
                # The timeout watchdog polls with one-second sleeps, so nothing is left parked.
                self.assertEqual(sorted({l for l in log.splitlines() if l.startswith("sleep:")} - {"sleep: 1"}), [])
                for destructive in ("delete-generic-password", "bootout", "agents remove", "daemon stop", "host remove"):
                    self.assertNotIn(destructive, log)

    def test_default_removes_programs_and_settings_and_keeps_spaces_data(self):
        for shell in shells():
            with self.subTest(shell=shell):
                box = Sandbox(tempfile.mkdtemp(dir=self.tmp.name), "Darwin", shell=shell)
                installed_mac(box)
                result = box.run("--yes", "--prefix", str(box.home))
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                # The app, the CLI, the host service, settings and caches are gone.
                for gone in (
                    "Applications/Cua Spaces.app", ".local/bin/cua",
                    "Library/LaunchAgents/com.trycua.spacesd.host.plist", ".cua/host", ".cua/config.toml",
                    ".cua/credentials.json", ".cua/cache", ".cua/telemetry", ".cua/volume/cache",
                    "Library/Application Support/com.trycua.spaces.macos", "Library/Caches/com.trycua.spaces.macos",
                    "Library/Preferences/com.trycua.spaces.macos.plist",
                ):
                    self.assertFalse(box.exists(gone), gone)
                # Spaces, their disks and the local Cua Volume files stay.
                self.assertEqual((box.home / ".cua/volume/data/notes.txt").read_text(), "my notes\n")
                self.assertTrue(box.exists(".cua/sandboxes/studio/disk.img"))
                self.assertTrue(box.exists(".cua/vmm/state"))
                self.assertIn("kept in", result.stdout)
                # The PATH block goes; the rest of the profile is untouched.
                self.assertEqual((box.home / ".zshrc").read_text(), "alias a=b\nexport KEEP=1\n")
                # Sign-in and device key leave the Keychain; Keyvault and other apps stay.
                self.assertEqual(box.keychain_items(), ["Cua Keyvault|vault-1", "Cua Keyvault Generation|vault-1", "Other App|me"])
                # The right steps, in a safe order: the app, hosting, agents, daemon, sign-out.
                steps = [
                    "host remove --force --uninstalling", "agents remove --agents all --yes",
                    "daemon stop", "auth logout",
                ]
                indexes = [box.index(step) for step in steps]
                self.assertTrue(all(i >= 0 for i in indexes), box.call_log())
                self.assertEqual(indexes, sorted(indexes))
                self.assertGreaterEqual(box.index("launchctl: bootout"), 0)

    def test_the_app_helper_goes_but_its_permissions_stay(self):
        box = self.box()
        installed_mac(box)
        # Cua Spaces 0.10+: the host service is the app's login item.
        box.put("Applications/Cua Spaces.app/Contents/Library/LaunchAgents/com.trycua.spaces.helper.plist", "plist\n")
        write_executable(box.home / "Applications/Cua Spaces.app/Contents/MacOS/CuaSpacesMac",
                         "#!/bin/sh\nprintf 'app: %s\\n' \"$*\" >>\"$FAKE_CALLS\"\n")
        box.put(".cua/helper/service.json", "{}\n")
        dry = box.run("--dry-run")
        self.assertEqual(dry.returncode, 0, dry.stdout + dry.stderr)
        self.assertIn("--service-helper unregister", dry.stdout)
        self.assertNotIn("tccutil", dry.stdout)
        self.assertEqual(box.index("tccutil"), -1)
        self.assertTrue(box.exists(".cua/helper/service.json"))
        result = box.run("--yes", "--prefix", str(box.home))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        # Unregistered by the app before the app goes, then stopped.
        self.assertGreaterEqual(box.index("app: --service-helper unregister"), 0)
        self.assertGreaterEqual(box.index("launchctl: bootout gui/1000/com.trycua.spaces.helper"), 0)
        self.assertEqual(box.index("tccutil"), -1)
        self.assertIn("minus button", result.stdout)
        for gone in (".cua/helper", "Applications/Cua Spaces.app",
                     "Library/LaunchAgents/com.trycua.spacesd.host.plist", ".cua/host/bin/cua-spacesd"):
            self.assertFalse(box.exists(gone), gone)

    def test_keep_data_keeps_the_cua_home_but_still_signs_out(self):
        box = self.box()
        installed_mac(box)
        result = box.run("--yes", "--keep-data", "--prefix", str(box.home))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        for kept in (".cua/config.toml", ".cua/cache/blob", ".cua/volume/cache/block", ".cua/sandboxes/studio/disk.img",
                     "Library/Application Support/com.trycua.spaces.macos/settings.json"):
            self.assertTrue(box.exists(kept), kept)
        for gone in ("Applications/Cua Spaces.app", ".local/bin/cua", ".cua/credentials.json",
                     "Library/LaunchAgents/com.trycua.spacesd.host.plist"):
            self.assertFalse(box.exists(gone), gone)
        # Only the session item goes; the device key and agent keys stay.
        self.assertIn("run.cua.ai|cua-cli.device-key", box.keychain_items())
        self.assertNotIn("run.cua.ai|cua-cli", box.keychain_items())
        self.assertGreaterEqual(box.index("auth logout"), 0)

    def test_running_spaces_are_listed_and_stopped_but_not_deleted(self):
        box = self.box()
        installed_mac(box)
        box.spaces.write_text("ID            STATUS   LOCATION  KIND       RUNTIME  EXPIRES\n"
                              "local:studio  ready    local     container  gvisor   -\n"
                              "local:old     stopped  local     vm         qemu     -\n")
        result = box.run("--yes", "--prefix", str(box.home))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("local:studio", result.stdout)
        self.assertGreaterEqual(box.index("spaces stop local:studio"), 0)
        self.assertEqual(box.index("spaces stop local:old"), -1)
        self.assertEqual(box.index("sandbox rm"), -1)
        self.assertLess(box.index("spaces stop local:studio"), box.index("daemon stop"))
        self.assertTrue(box.exists(".cua/sandboxes/studio/disk.img"))

    def test_delete_spaces_stops_then_deletes_everything_including_the_keyvault(self):
        box = self.box()
        installed_mac(box)
        box.spaces.write_text("ID            STATUS   LOCATION  KIND       RUNTIME  EXPIRES\n"
                              "local:studio  ready    local     container  gvisor   -\n"
                              "local:old     stopped  local     vm         qemu     -\n")
        result = box.run("--yes", "--delete-spaces", "--prefix", str(box.home))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("DELETES the Spaces", result.stdout)
        self.assertLess(box.index("spaces stop local:studio"), box.index("sandbox rm local:studio --force"))
        self.assertGreaterEqual(box.index("sandbox rm local:old --force"), 0)
        self.assertFalse(box.exists(".cua"))
        self.assertEqual(box.keychain_items(), ["Other App|me"])

    def test_a_running_app_is_asked_to_quit_before_anything_else(self):
        box = self.box()
        installed_mac(box)
        running = box.root / "app-running"
        running.write_text("")
        write_executable(box.bin / "pgrep", f'#!/bin/sh\n[ -f {shlex.quote(str(running))} ]\n')
        write_executable(box.bin / "osascript", f'#!/bin/sh\nprintf "osascript: %s\\n" "$*" >>"$FAKE_CALLS"\nrm -f {shlex.quote(str(running))}\n')
        result = box.run("--yes", "--prefix", str(box.home))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        quit_at = box.index('osascript: -e tell application id "com.trycua.spaces.macos" to quit')
        self.assertGreaterEqual(quit_at, 0, box.call_log())
        self.assertLess(quit_at, box.index("daemon stop"))
        self.assertEqual(box.index("pkill"), -1)

    def test_nothing_is_deleted_without_asking_when_there_is_no_terminal(self):
        box = self.box()
        installed_mac(box)
        before = sorted(str(p) for p in box.home.rglob("*"))
        result = box.run("--prefix", str(box.home))
        self.assertEqual(result.returncode, 1)
        self.assertIn("rerun with --yes", result.stderr)
        self.assertEqual(before, sorted(str(p) for p in box.home.rglob("*")))

    def test_idempotent_and_safe_when_nothing_or_only_a_part_is_installed(self):
        box = self.box()
        result = box.run("--yes")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("nothing to remove", result.stdout)
        # Only the CLI is there.
        box.cli()
        result = box.run("--yes")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertFalse(box.exists(".local/bin/cua"))
        again = box.run("--yes")
        self.assertEqual(again.returncode, 0)
        self.assertIn("nothing to remove", again.stdout)

    def test_a_cua_that_is_not_the_cua_cli_is_left_alone(self):
        box = self.box()
        box.put(".local/bin/cua", "#!/usr/bin/env python3\nprint('cua 0.9')\n", 0o755)
        box.put(".cua/config.toml", "x\n")
        result = box.run("--yes")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertTrue(box.exists(".local/bin/cua"))
        self.assertFalse(box.exists(".cua/config.toml"))

    def test_the_path_line_stays_while_cua_driver_lives_in_the_same_folder(self):
        box = self.box()
        box.cli()
        box.put(".local/bin/cua-driver", "driver\n", 0o755)
        box.put(".zshrc", PATH_BLOCK.format(home=box.home))
        result = box.run("--yes")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("# Added by the cua installer", (box.home / ".zshrc").read_text())
        self.assertIn("Cua Driver is separate", result.stdout)

    def test_no_modify_path_never_edits_the_profile(self):
        box = self.box()
        box.cli()
        box.put(".zshrc", PATH_BLOCK.format(home=box.home))
        before = (box.home / ".zshrc").read_text()
        result = box.run("--yes", "--no-modify-path")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual((box.home / ".zshrc").read_text(), before)

    def test_a_volume_that_cannot_be_unmounted_keeps_every_file(self):
        box = self.box()
        installed_mac(box)
        mounts = box.root / "mounts.txt"
        mounts.write_text(f"localhost:/ on {box.home}/Cua Volume (nfs, nodev, nosuid)\n")
        result = box.run("--yes", "--delete-spaces", "--prefix", str(box.home), env={"FAKE_MOUNTS": str(mounts)})
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("still mounted", result.stderr)
        self.assertTrue(box.exists(".cua/volume/data/notes.txt"))
        self.assertTrue(box.exists(".cua/sandboxes/studio/disk.img"))

    def test_a_machine_wide_install_is_removed_too(self):
        box = self.box()
        box.put_sys("Applications/Cua Spaces.app/Contents/MacOS/CuaSpacesMac", "app\n", 0o755)
        link = box.sys / "usr/local/bin/cua"
        link.parent.mkdir(parents=True)
        link.symlink_to("/Applications/Cua Spaces.app/Contents/MacOS/cua")
        box.put_sys("usr/local/bin/other", "keep\n", 0o755)
        result = box.run("--yes")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertFalse((box.sys / "Applications/Cua Spaces.app").exists())
        self.assertFalse(link.is_symlink())
        self.assertTrue((box.sys / "usr/local/bin/other").exists())

    def test_an_app_with_another_bundle_id_is_left_alone(self):
        box = self.box()
        app = box.put_sys("Applications/Cua Spaces.app/Contents/Info.plist", "plist\n").parent.parent
        write_executable(box.bin / "defaults", "#!/bin/sh\necho com.example.other\n")
        result = box.run("--yes")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertTrue(app.exists())

    def test_the_contradicting_flags_are_refused(self):
        result = self.box().run("--keep-data", "--delete-spaces", "--yes")
        self.assertEqual(result.returncode, 1)
        self.assertIn("contradict", result.stderr)

    def test_unknown_options_and_help(self):
        box = self.box()
        self.assertEqual(box.run("--nope").returncode, 1)
        helped = box.run("--help")
        self.assertEqual(helped.returncode, 0)
        for flag in ("--keep-data", "--delete-spaces", "--all", "--dry-run", "--yes", "--prefix"):
            self.assertIn(flag, helped.stdout)

    def test_a_home_that_is_the_users_home_is_never_deleted(self):
        box = self.box()
        box.put("Documents/keep.txt", "mine\n")
        box.put("sandboxes/a/disk", "disk\n")
        result = box.run("--yes", "--delete-spaces", env={"CUA_HOME": str(box.home)})
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertTrue(box.exists("Documents/keep.txt"))
        self.assertIn("not touching", result.stderr)

    def test_a_custom_cua_home_without_cua_files_is_left_alone(self):
        box = self.box()
        box.put("elsewhere/notes.txt", "mine\n")
        box.put(".cua/config.toml", "x\n")
        result = box.run("--yes", "--delete-spaces", env={"CUA_HOME": str(box.home / "elsewhere")})
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertTrue(box.exists("elsewhere/notes.txt"))
        self.assertIn("no Cua files", result.stderr)


class OtherToolsTest(unittest.TestCase):
    """--all runs the Cua Driver and Lume uninstallers (served here, not cua.ai)."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        served = Path(self.tmp.name) / "served"
        (served / "driver").mkdir(parents=True)
        (served / "lume").mkdir(parents=True)
        self.log = Path(self.tmp.name) / "tools.log"
        for name in ("driver", "lume"):
            (served / name / "uninstall.sh").write_text(f'#!/bin/bash\necho "{name} $*" >>{shlex.quote(str(self.log))}\n')

        class Handler(http.server.SimpleHTTPRequestHandler):
            def __init__(self, *a, **k):
                super().__init__(*a, directory=str(served), **k)

            def log_message(self, *a):
                pass

        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        self.addCleanup(self.server.shutdown)
        self.base = f"http://127.0.0.1:{self.server.server_address[1]}"

    def env(self):
        return {
            "CUA_UNINSTALL_DRIVER_URL": f"{self.base}/driver/uninstall.sh",
            "CUA_UNINSTALL_LUME_URL": f"{self.base}/lume/uninstall.sh",
        }

    def test_all_runs_both_uninstallers_and_keeps_lume_vms_by_default(self):
        box = Sandbox(tempfile.mkdtemp(dir=self.tmp.name), "Darwin")
        box.cli()
        result = box.run("--yes", "--all", env=self.env())
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self.log.read_text().splitlines(), ["driver ", "lume --force"])

    def test_all_with_delete_spaces_purges_lume(self):
        box = Sandbox(tempfile.mkdtemp(dir=self.tmp.name), "Darwin")
        box.cli()
        result = box.run("--yes", "--all", "--delete-spaces", env=self.env())
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self.log.read_text().splitlines(), ["driver ", "lume --force --purge"])

    def test_all_never_revokes_the_driver_macos_permissions(self):
        """The driver uninstaller keeps Accessibility and Screen Recording unless
        asked with --reset-permissions; --all must never ask (18 Oct 2026: an
        uninstall wiped the permissions of a Mac that was meant to keep them)."""
        box = Sandbox(tempfile.mkdtemp(dir=self.tmp.name), "Darwin")
        box.cli()
        result = box.run("--yes", "--all", "--delete-spaces", env=self.env())
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self.log.read_text().splitlines()[0], "driver ")
        self.assertEqual([call for call in box.call_log() if call.startswith("tccutil")], [])
        for name in ("uninstall.sh", "uninstall.ps1"):
            script = (Path(__file__).resolve().parent.parent / name).read_text()
            self.assertNotIn("tccutil", script, name)
            self.assertNotIn("reset-permissions", script, name)

    def test_on_linux_lume_is_skipped_and_the_driver_runs_even_with_nothing_else_installed(self):
        box = Sandbox(tempfile.mkdtemp(dir=self.tmp.name), "Linux")
        result = box.run("--yes", "--all", env=self.env())
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self.log.read_text().splitlines(), ["driver "])

    def test_a_plain_http_uninstaller_url_is_refused(self):
        box = Sandbox(tempfile.mkdtemp(dir=self.tmp.name), "Linux")
        result = box.run("--yes", "--all", env={"CUA_UNINSTALL_DRIVER_URL": "http://example.com/uninstall.sh"})
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("non-HTTPS", result.stderr)
        self.assertFalse(self.log.exists())


class UninstallLinuxTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)

    def installed(self, box):
        box.cli()
        box.put(".local/bin/cua-spaces", "appimage\n", 0o755)
        box.put(".local/share/applications/cua-spaces.desktop", "[Desktop Entry]\nName=Cua Spaces\nExec=cua-spaces\n")
        box.put(".config/autostart/cua-spaces.desktop", "[Desktop Entry]\nName=Cua Spaces\nExec=cua-spaces --hidden\n")
        box.put(".config/autostart/other.desktop", "[Desktop Entry]\nName=Other\nExec=other\n")
        box.put(".config/systemd/user/cua-spacesd-host.service", "[Unit]\n")
        box.put(".config/Cua Spaces/settings.json", "{}\n")
        box.put(".local/share/cua-spaces/daemon/1/cua", "daemon\n", 0o755)
        box.put(".bashrc", PATH_BLOCK.format(home=box.home))
        box.put(".cua/host/config.json", "{}\n")
        box.put(".cua/credentials.json", "{}\n")
        box.put(".cua/sandboxes/a/disk", "disk\n")

    def test_removes_the_appimage_autostart_service_and_state(self):
        for shell in shells():
            with self.subTest(shell=shell):
                box = Sandbox(tempfile.mkdtemp(dir=self.tmp.name), "Linux", shell=shell)
                self.installed(box)
                result = box.run("--yes", "--prefix", str(box.home / "nowhere"))
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                for gone in (".local/bin/cua", ".local/bin/cua-spaces", ".local/share/applications/cua-spaces.desktop",
                             ".config/autostart/cua-spaces.desktop", ".config/systemd/user/cua-spacesd-host.service",
                             ".config/Cua Spaces", ".local/share/cua-spaces", ".cua/host", ".cua/credentials.json"):
                    self.assertFalse(box.exists(gone), gone)
                self.assertTrue(box.exists(".config/autostart/other.desktop"))
                self.assertTrue(box.exists(".cua/sandboxes/a/disk"))
                self.assertEqual((box.home / ".bashrc").read_text(), "")
                self.assertGreaterEqual(box.index("systemctl: --user disable --now cua-spacesd-host"), 0)
                self.assertGreaterEqual(box.index("host remove --force --uninstalling"), 0)

    def test_the_deb_is_removed_with_apt_as_root(self):
        box = Sandbox(self.tmp.name, "Linux")
        write_executable(box.bin / "dpkg-query", "#!/bin/sh\nprintf 'install ok installed'\n")
        result = box.run("--yes", env={"FAKE_UID": "0"})
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertGreaterEqual(box.index("apt-get: remove -y cua-spaces"), 0)

    def test_the_deb_without_privileges_says_what_to_run(self):
        box = Sandbox(self.tmp.name, "Linux")
        write_executable(box.bin / "dpkg-query", "#!/bin/sh\nprintf 'install ok installed'\n")
        write_executable(box.bin / "sudo", "#!/bin/sh\nexit 1\n")
        result = box.run("--yes")
        self.assertEqual(result.returncode, 1)
        self.assertIn("sudo apt-get remove cua-spaces", result.stderr)


if __name__ == "__main__":
    unittest.main()
