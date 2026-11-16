"""Tests for bin/ci: which checks a change selects, fix parsing, the hook, and workflow sync."""

from __future__ import annotations

import importlib.machinery
import importlib.util
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[3]


def load():
    loader = importlib.machinery.SourceFileLoader("bin_ci", str(REPO / "bin/ci"))
    spec = importlib.util.spec_from_loader("bin_ci", loader)
    mod = importlib.util.module_from_spec(spec)
    sys.modules["bin_ci"] = mod  # dataclasses look the module up
    loader.exec_module(mod)
    return mod


ci = load()


def sh_script(path):
    """Argv that runs a #!/bin/sh script: Windows cannot execute one, so Git for Windows' bash does."""
    return [ci.BASH, Path(path).as_posix()] if os.name == "nt" else [str(path)]


class FakeContext:
    """Routes without cargo or the docs runner."""

    def __init__(self, generators=(), crate_hits=()):
        self.notes = []
        self.rust_env = {}
        self._generators = list(generators)
        self._crate_hits = set(crate_hits)

    def docs_generators(self, changes):
        return self._generators

    def crate_trigger(self, changes, ws_root, roots, path_rx):
        return bool(changes.touches(path_rx)) or bool(self._crate_hits & set(roots))


def ids(files, **kw):
    changes = ci.Changes("HEAD", "HEAD", files)
    return {c.id for c in ci.static_checks(changes, FakeContext(**kw))}


class Routing(unittest.TestCase):
    def test_workflow_change_runs_actionlint_and_sync(self):
        got = ids([".github/workflows/ci-gate.yml"])
        self.assertIn("actionlint", got)
        self.assertIn("bin-ci-tests", got)

    def test_spaces_app_change_runs_prettier_on_the_changed_files(self):
        self.assertIn("spaces-prettier", ids(["apps/cua-spaces-web/src/main.tsx"]))
        self.assertIn("spaces-prettier", ids(["apps/cua-spaces/package.json"]))
        self.assertNotIn("spaces-prettier", ids(["apps/cua-spaces-macos/README.md"]))
        self.assertNotIn("spaces-prettier", ids(["apps/cua-spaces-web/gone.ts"]))

    def test_nested_cargo_manifest_runs_the_cargo_lockfile_check(self):
        # PR 6358: a libs/cua crate's manifest changed and nested workspaces'
        # Cargo.lock files (examples/, tests/e2e/) drifted.
        changes = ci.Changes("HEAD", "HEAD", ["libs/cua/crates/cua-vmm/Cargo.toml"])
        lock = [c for c in ci.static_checks(changes, FakeContext()) if c.id == "lockfiles"]
        self.assertEqual(len(lock), 1)
        self.assertIn("--kind cargo", lock[0].cmd)
        self.assertNotIn("--kind uv", lock[0].cmd)

    def test_sdk_source_runs_the_bindings_checks(self):
        got = ids(["libs/cua/crates/cua-sdk/src/lib.rs"])
        self.assertIn("sdk-bindings", got)
        self.assertIn("spaces-app-bindings", got)

    def test_spaces_crate_runs_the_contract_check(self):
        self.assertIn("spaces-contract", ids(["libs/cua/crates/cua-spaces/src/spaces.rs"], crate_hits={"cua-spaces"}))
        self.assertNotIn("spaces-contract", ids(["libs/cua/crates/cua-image/src/manifest.rs"]))

    def test_driver_contract_crate_runs_contract_and_bindings(self):
        got = ids(["libs/cua-driver/rust/crates/cua-driver-contract/src/outputs.rs"],
                  crate_hits={"cua-driver-contract", "cua-driver-sdk"})
        self.assertIn("driver-contract", got)
        self.assertIn("driver-bindings", got)

    def test_docs_page_runs_code_block_and_image_ref_checks(self):
        got = ids(["docs/content/docs/spaces/guides/connect-your-agent.mdx"])
        self.assertIn("docs-code-blocks", got)
        self.assertIn("docs-image-refs", got)

    def test_docs_reference_rows_come_from_the_router(self):
        got = ids(["libs/cua/crates/cua-spaces/src/x.rs"], generators=["spaces", "cua-sdk"])
        self.assertIn("docs-ref:spaces", got)
        self.assertIn("docs-ref:cua-sdk", got)

    def test_image_refs_gate_runs_for_any_change(self):
        self.assertIn("image-refs", ids(["README.md"]))

    def test_python_checks_mirror_ci_scope(self):
        # PR 6318: a uv.lock change made black fail a file the PR never touched.
        for files, want in ((["tests/e2e/cua-sdk/python/conftest.py"], 1), (["uv.lock"], 1), (["libs/cua-s1/x.py"], 0)):
            checks = ci.python_checks(ci.Changes("HEAD", "HEAD", files))
            self.assertEqual(len(checks), want, files)
        self.assertIn("black --check libs/python tests", ci.python_checks(ci.Changes("HEAD", "HEAD", ["uv.lock"]))[0].cmd)

    def test_web_ui_change_runs_typecheck_design_lint_and_tests(self):
        changes = ci.Changes("HEAD", "HEAD", ["apps/cua-spaces-web/src/main.tsx"])
        got = {c.id for c in ci.web_checks(changes)}
        self.assertTrue({"web:apps/cua-spaces-web:typecheck", "web:apps/cua-spaces-web:design-lint",
                         "web:apps/cua-spaces-web:test"} <= got)
        self.assertFalse(any(i.startswith("web:apps/cua-spaces-desktop") for i in got))

    def test_full_mode_selects_everything(self):
        changes = ci.Changes("HEAD", "HEAD", [], full=True)
        got = {c.id for c in ci.static_checks(changes, FakeContext(generators=["spaces"]))}
        for want in ("lockfiles", "actionlint", "sdk-bindings", "spaces-contract", "driver-contract", "docs-ref:spaces"):
            self.assertIn(want, got)


class Graph(unittest.TestCase):
    ws = ci.Workspace("libs/x", {"a": "a", "b": "b", "c": "c", "d": "d"},
                      {"a": [], "b": ["a"], "c": ["b"], "d": []})

    def test_dependents_are_transitive(self):
        self.assertEqual(ci.dependents(self.ws, ["a"]), ["b", "c"])
        self.assertEqual(ci.dependents(self.ws, ["d"]), [])

    def test_deps_closure(self):
        self.assertEqual(ci.deps_closure(self.ws, ["c"]), {"a", "b", "c"})

    def test_package_of_finds_the_owning_crate(self):
        self.assertEqual(ci.package_of("libs/cua/crates/cua-sdk/src/lib.rs"), ("libs/cua/crates/cua-sdk", "cua-sdk"))
        self.assertIsNone(ci.package_of("README.md"))


class Fixes(unittest.TestCase):
    def test_lockfile_fix_commands_come_from_the_checker_output(self):
        check = ci.Check("lockfiles", "t", "true", fix_from_output=r"^FAIL .*?; fix: (.+)$")
        with tempfile.NamedTemporaryFile("w", suffix=".log", delete=False) as fh:
            fh.write("ok    libs/cua/Cargo.lock\n"
                     "FAIL  examples/streaming/rust/Cargo.lock drifted; fix: (cd examples/streaming/rust && cargo update --workspace)\n"
                     "::error::lockfile drift: (cd examples/streaming/rust && cargo update --workspace)\n")
        try:
            r = ci.Result(check, "FAIL", log=Path(fh.name))
            self.assertEqual(ci.fix_commands(r), ["(cd examples/streaming/rust && cargo update --workspace)"])
        finally:
            os.unlink(fh.name)

    def test_excerpt_prefers_error_lines(self):
        with tempfile.NamedTemporaryFile("w", suffix=".log", delete=False) as fh:
            fh.write("   Compiling foo v1\nnoise\nDiff in /x/src/a.rs:776:\nmore noise\n")
        try:
            self.assertEqual(ci.excerpt(Path(fh.name)), ["Diff in /x/src/a.rs:776:"])
        finally:
            os.unlink(fh.name)


class Windows(unittest.TestCase):
    def test_a_rustc_error_fails(self):
        # PR 6355: Unix-only code left an unused import on Windows.
        log = "error: unused import: `AsyncBufReadExt`\n  --> crates\\cua-spaces\\src\\tree_hub.rs:32:17\nerror: could not compile `cua-spaces` (lib)\n"
        self.assertEqual(ci.classify_xwin(log, 101)[0], "FAIL")

    def test_a_c_dependency_that_cannot_cross_build_only_warns(self):
        log = "error: failed to run custom build command for `audiopus_sys v0.2.2`\nwarning: build failed, waiting for other jobs to finish...\n"
        status, detail = ci.classify_xwin(log, 101)
        self.assertEqual(status, "warn")
        self.assertIn("audiopus_sys", detail)

    def test_clean_build_passes(self):
        self.assertEqual(ci.classify_xwin("Finished", 0)[0], "pass")


class Hook(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.repo = Path(self.tmp.name)
        subprocess.run(["git", "init", "-q", str(self.repo)], check=True)
        self._root = ci.ROOT
        ci.ROOT = self.repo
        self.hooks = self.repo / ".git/hooks"

    def tearDown(self):
        ci.ROOT = self._root
        self.tmp.cleanup()

    def test_install_keeps_and_chains_an_existing_hook_then_uninstall_restores_it(self):
        self.hooks.mkdir(exist_ok=True)
        (self.hooks / "pre-push").write_text("#!/bin/sh\necho mine\n")
        self.assertEqual(ci.install_hook(force=False), 0)
        hook = (self.hooks / "pre-push").read_text()
        self.assertIn(ci.HOOK_MARKER, hook)
        self.assertIn(ci.CHAINED_HOOK, hook)
        self.assertEqual((self.hooks / ci.CHAINED_HOOK).read_text(), "#!/bin/sh\necho mine\n")
        self.assertEqual(ci.install_hook(force=False), 0)  # idempotent
        self.assertEqual(ci.uninstall_hook(), 0)
        self.assertEqual((self.hooks / "pre-push").read_text(), "#!/bin/sh\necho mine\n")

    def test_hook_runs_the_chained_hook_first_and_stops_on_its_failure(self):
        self.hooks.mkdir(exist_ok=True)
        (self.hooks / "pre-push").write_text("#!/bin/sh\necho guard-ran; exit 3\n")
        (self.hooks / "pre-push").chmod(0o755)
        ci.install_hook(force=False)
        out = subprocess.run(sh_script(self.hooks / "pre-push") + ["origin", "url"], input="refs/heads/x 1 refs/heads/x 0\n",
                             capture_output=True, text=True, cwd=self.repo)
        self.assertEqual(out.returncode, 3)
        self.assertIn("guard-ran", out.stdout)

    def test_skip_variable(self):
        ci.install_hook(force=False)
        out = subprocess.run(sh_script(self.hooks / "pre-push") + ["origin", "url"], input="", capture_output=True, text=True,
                             cwd=self.repo, env={**os.environ, "CUA_SKIP_LOCAL_CI": "1"})
        self.assertEqual(out.returncode, 0)
        self.assertIn("skipped", out.stderr)

    def test_deleting_a_branch_checks_nothing(self):
        zero = "0" * 40
        self.assertTrue(ci.hook_refs_are_deletions(f"(delete) {zero} refs/heads/x abc\n"))
        self.assertFalse(ci.hook_refs_are_deletions(f"refs/heads/x abc refs/heads/x {zero}\n"))


class Sync(unittest.TestCase):
    def test_every_mirrored_command_is_still_in_its_workflow(self):
        missing = []
        for cid, wf, text in ci.all_check_refs():
            path = REPO / ".github/workflows" / wf
            if not path.exists() or text not in path.read_text():
                missing.append(f"{cid}: {wf}: {text}")
        self.assertEqual(missing, [])


if __name__ == "__main__":
    unittest.main()
