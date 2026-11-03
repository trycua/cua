"""Recipe lint and drift guards for the macOS Space with developer tools (the
xcode tier of libs/images/macos): its pins and claims agree, every script
parses, the tier is published like the others, and what carries Xcode says
whose software it is and where it may run."""

from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", "..", ".."))
MACOS = os.path.join(ROOT, "libs", "images", "macos")
CATALOG = os.path.join(ROOT, "libs", "images", "sandbox-images.json")
WORKFLOW = os.path.join(ROOT, ".github", "workflows", "cd-image-macos.yml")
SCRIPTS = [
    "build.sh",
    "derive-local.sh",
    "release-tier.sh",
    "push.sh",
    "tiers.sh",
    "files/tier-xcode.sh",
    "files/tier-full.sh",
    "files/fetch.sh",
    "files/xcode-source.sh",
    "annotate.sh",
    "dev-image-gate.sh",
    "files/dev-image-gate-guest.sh",
]
# The xcode tier's tools: image.json claim -> versions.env key.
XCODE_TOOLS = {
    "xcodebuild": "XCODE_VERSION",
    "sccache": "SCCACHE_VERSION",
    "xcodegen": "XCODEGEN_VERSION",
    "pod": "COCOAPODS_VERSION",
}
MACOS_REPO = "ghcr.io/trycua/" + "macos"  # not a catalog ref: a repository


def versions() -> dict[str, str]:
    env = {}
    for line in open(os.path.join(MACOS, "versions.env")).read().splitlines():
        if line and not line.startswith("#") and "=" in line:
            k, v = line.split("=", 1)
            env[k] = v.strip('"')
    return env


def run(args, **kw):
    return subprocess.run(args, capture_output=True, text=True, **kw)


class Recipe(unittest.TestCase):
    def test_scripts_parse(self):
        for s in SCRIPTS:
            r = run(["bash", "-n", os.path.join(MACOS, s)])
            self.assertEqual(r.returncode, 0, f"{s}: {r.stderr}")

    @unittest.skipUnless(shutil.which("shellcheck"), "shellcheck not installed")
    def test_new_scripts_pass_shellcheck(self):
        for s in ["derive-local.sh", "files/tier-xcode.sh", "files/fetch.sh", "files/xcode-source.sh"]:
            r = run(["shellcheck", "-x", "-S", "warning", os.path.join(MACOS, s)])
            self.assertEqual(r.returncode, 0, f"{s}: {r.stdout}")

    def test_pins_are_complete_and_checksummed(self):
        env = versions()
        for name in ("SCCACHE", "XCODEGEN"):
            self.assertIn(f"{name}_VERSION", env)
            self.assertTrue(env[f"{name}_URL"].startswith("https://"), name)
            self.assertIn(env[f"{name}_VERSION"], env[f"{name}_URL"], name)
            self.assertRegex(env[f"{name}_SHA256"], r"^[0-9a-f]{64}$", name)
        self.assertRegex(env["COCOAPODS_VERSION"], r"^\d+\.\d+\.\d+$")
        self.assertTrue(env["RUST_IOS_TARGETS"].split())
        for t in env["RUST_IOS_TARGETS"].split():
            self.assertTrue(t.endswith("-ios") or t.endswith("-ios-sim"), t)

    def test_xcode_claims_name_the_pinned_versions(self):
        env = versions()
        claims = json.load(open(os.path.join(MACOS, "image.json")))["tiers"]["xcode"]["claims"]
        for tool, key in XCODE_TOOLS.items():
            expect = claims["tools"][tool]["expect"]
            version = env[key]
            self.assertTrue(
                re.search(expect, version) or re.escape(version) in expect,
                f"{tool}: {expect!r} does not name {key}={version}",
            )
        self.assertEqual(claims["simulator_runtimes"], ["iOS " + env["XCODE_IOS_RUNTIME"]])

    def test_tier_script_installs_every_pinned_tool(self):
        script = open(os.path.join(MACOS, "files", "tier-xcode.sh")).read()
        for needle in ("RUST_IOS_TARGETS", "SCCACHE_URL", "XCODEGEN_URL", "COCOAPODS_VERSION",
                       "DevToolsSecurity", "_developer", "xcodebuild -license accept",
                       "-downloadPlatform iOS", "simctl create"):
            self.assertIn(needle, script)

    def test_lume_wrapper_never_calls_itself(self):
        # A `lume` shell function defaulting to the program named lume would
        # recurse until bash crashes: the default must be a resolved path.
        for s in ("derive-local.sh", "dev-image-gate.sh"):
            text = open(os.path.join(MACOS, s)).read()
            self.assertIn("type -P lume", text, s)
            self.assertNotIn("command lume", text, s)

    def test_derive_stages_what_the_tier_script_reads(self):
        derive = open(os.path.join(MACOS, "derive-local.sh")).read()
        for needle in ("tier-xcode.sh", "versions.env", "fetch_xcode_tools", "xcode-source.sh"):
            self.assertIn(needle, derive)


class Gate(unittest.TestCase):
    def test_fixture_covers_what_the_gate_claims(self):
        fx = os.path.join(MACOS, "fixtures", "HelloSim")
        spec = open(os.path.join(fx, "project.yml")).read()
        for target in ("HelloSim:", "HelloSimTests:", "HelloSimUITests:"):
            self.assertIn(target, spec)
        self.assertTrue(os.path.isfile(os.path.join(fx, "rust", "Cargo.lock")), "locked: the gate builds offline-stable")
        guest = open(os.path.join(MACOS, "files", "dev-image-gate-guest.sh")).read()
        env = versions()
        for needle in ("xcodebuild test", "aarch64-apple-ios-sim", "xcresulttool export attachments",
                       "simctl io", "RUSTC_WRAPPER=sccache", env["SIMULATOR_DEVICE_NAME"]):
            self.assertIn(needle, guest)


class Published(unittest.TestCase):
    def test_workflow_still_builds_gates_and_publishes_every_tier(self):
        wf = open(WORKFLOW).read()
        self.assertIn("upto=xcode publish=true", wf)
        self.assertRegex(wf, r"slim\) tag=26-slim ;; full\) tag=26 ;; xcode\) tag=26-xcode")

    def test_ci_needs_no_apple_credentials(self):
        build = open(os.path.join(MACOS, "build.sh")).read()
        # The pinned Xcode comes from the runner (an installed Xcode.app or a
        # cached .xip), never from a download with an Apple ID.
        self.assertIn("xcode-source.sh", build)
        self.assertIn("XCODE_BUILD", build)
        for secret in ("XCODES_USERNAME", "XCODES_PASSWORD", "APPLE_ID", "xcodes download", "xcodes install"):
            for name in ("build.sh", "files/tier-xcode.sh", "derive-local.sh", "files/xcode-source.sh"):
                self.assertNotIn(secret, open(os.path.join(MACOS, name)).read(), (secret, name))
        self.assertNotIn("XCODES_", open(WORKFLOW).read())

    def test_the_tier_says_it_is_apple_software_for_apple_hardware(self):
        image = json.load(open(os.path.join(MACOS, "image.json")))
        desc = image["tiers"]["xcode"]["description"]
        self.assertIn("Apple software", desc)
        self.assertIn("Apple-branded hardware", desc)
        annotate = open(os.path.join(MACOS, "annotate.sh")).read()
        self.assertIn("ai.cua.image.notice", annotate)
        self.assertIn("Apple-branded hardware", annotate)

    def test_tag_safety_lets_the_xcode_tier_float(self):
        tmp = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, tmp, True)
        crane = os.path.join(tmp, "crane")
        with open(crane, "w") as f:
            f.write("#!/bin/sh\necho 'MANIFEST_UNKNOWN' >&2\nexit 1\n")
        os.chmod(crane, 0o755)
        guard = os.path.join(ROOT, "scripts", "images", "check-tag-safety.sh")
        r = run([guard, f"{MACOS_REPO}:26-xcode-20261101-abcdef1"], env=dict(os.environ, CRANE=crane))
        self.assertEqual(r.returncode, 0, r.stderr)


@unittest.skipUnless(sys.platform == "darwin", "derive-local.sh reads Xcode bundles (macOS)")
class DeriveLocal(unittest.TestCase):
    script = os.path.join(MACOS, "derive-local.sh")

    def fake_xcode(self, tmp: str) -> str:
        app = os.path.join(tmp, "Xcode.app", "Contents")
        os.makedirs(app)
        with open(os.path.join(app, "Info.plist"), "w") as f:
            f.write('<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict>'
                    "<key>CFBundleShortVersionString</key><string>26.6</string>"
                    "<key>LSMinimumSystemVersion</key><string>26.2</string></dict></plist>")
        return os.path.dirname(app)

    def test_refuses_off_apple_hardware_in_the_script(self):
        text = open(self.script).read()
        self.assertIn("Apple-branded hardware", text)
        self.assertIn('uname -m', text)

    def test_needs_a_base_vm(self):
        r = run([self.script])
        self.assertEqual(r.returncode, 2)
        self.assertIn("--base-vm", r.stderr)

    def test_refuses_early_when_the_disk_is_short(self):
        tmp = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, tmp, True)
        env = dict(os.environ, CUA_DEV_IMAGE_FLOOR_GB="100000000", CUA_LUME_STORAGE=os.path.join(tmp, "lume"))
        r = run([self.script, "--base-vm", "x", "--xcode", self.fake_xcode(tmp), "--out", os.path.join(tmp, "out")], env=env)
        self.assertEqual(r.returncode, 4, r.stderr)
        self.assertIn("Make room", r.stderr)
        self.assertIn("Not enough free disk", r.stderr)
        self.assertFalse(os.path.exists(os.path.join(tmp, "out", "setup")), "nothing is staged before the disk check")

    def test_says_what_is_needed_when_there_is_no_xcode(self):
        if (shutil.which("xcode-select") and "Xcode" in run(["xcode-select", "-p"]).stdout) or any(
            n.startswith("Xcode") for n in os.listdir("/Applications")
        ):
            self.skipTest("this Mac has an Xcode")
        env = dict(os.environ, HOME=tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, env["HOME"], True)
        r = run([self.script, "--base-vm", "x"], env=env)
        self.assertEqual(r.returncode, 3, r.stderr)
        self.assertIn("App Store", r.stderr)
        self.assertIn("developer.apple.com", r.stderr)



class CuaVmmRecipeCopies(unittest.TestCase):
    """cua-vmm embeds committed copies of the recipe (so the Rust build reads
    only libs/cua); they must equal libs/images/macos."""

    COPIES = {
        "derive-local.sh": "macos-derive-local.sh",
        "versions.env": "macos-versions.env",
        "files/tier-xcode.sh": "macos-tier-xcode.sh",
        "files/fetch.sh": "macos-fetch.sh",
        "files/xcode-source.sh": "macos-xcode-source.sh",
    }

    def test_the_copies_match_the_recipe(self):
        recipe = os.path.join(ROOT, "libs", "cua", "crates", "cua-vmm", "recipe")
        for source, name in self.COPIES.items():
            with open(os.path.join(MACOS, source), "rb") as a, open(os.path.join(recipe, name), "rb") as b:
                self.assertEqual(
                    a.read(), b.read(), f"{name} is behind {source}: run scripts/images/sync-cua-vmm-recipe.sh"
                )

if __name__ == "__main__":
    unittest.main()
