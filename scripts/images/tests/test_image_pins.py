"""Tests for image_pins.py (against a fake registry) and the wiring of
cd-images-spacesd-release.yml to the image workflows it calls."""

from __future__ import annotations

import contextlib
import io
import json
import os
import re
import shutil
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.dirname(HERE))
import image_pins  # noqa: E402

ROOT = os.path.abspath(os.path.join(HERE, "..", "..", ".."))
WORKFLOWS = os.path.join(ROOT, ".github", "workflows")
INDEX = "application/vnd.oci.image.index.v1+json"
# Built, not spelled out: dated pins are not catalog refs (check-image-refs.py).
MACOS = "ghcr.io/trycua/" + "macos"


def d(n: int) -> str:
    return "sha256:" + f"{n:064x}"


class FakeRegistry(image_pins.Registry):
    """tags: ref -> digest; manifests: repo@digest -> manifest;
    reports: repo@digest -> [annotations]."""

    def __init__(self) -> None:
        self.tag_digests: dict[str, str] = {}
        self.manifests: dict[str, dict] = {}
        self.report_map: dict[str, list[dict]] = {}

    def digest(self, ref: str) -> str:
        return self.tag_digests[ref]

    def manifest(self, ref: str) -> dict:
        return self.manifests.get(ref, {"mediaType": "application/vnd.oci.image.manifest.v1+json"})

    def tags(self, repo: str) -> list[str]:
        return [r.rsplit(":", 1)[1] for r in self.tag_digests if r.rsplit(":", 1)[0] == repo]

    def reports(self, ref: str) -> list[dict]:
        return self.report_map.get(ref, [])

    # helpers
    def index(self, repo: str, digest: str, children: dict[str, str]) -> None:
        self.manifests[f"{repo}@{digest}"] = {
            "mediaType": INDEX,
            "manifests": [{"digest": c, "platform": {"architecture": a, "os": "linux"}} for a, c in children.items()]
            + [{"digest": d(999), "platform": {"architecture": "unknown", "os": "unknown"}}],
        }

    def report(self, repo: str, digest: str, version: str, created: str = "2026-10-03T00:00:00Z", status: str = "pass") -> None:
        self.report_map.setdefault(f"{repo}@{digest}", []).append(
            {
                "ai.cua.doctor.spacesd": version,
                "ai.cua.doctor.status": status,
                "org.opencontainers.image.created": created,
            }
        )


class SpacesdOfTests(unittest.TestCase):
    def setUp(self) -> None:
        self.r = FakeRegistry()
        self.repo = "ghcr.io/trycua/linux"

    def test_index_children_agree(self) -> None:
        self.r.index(self.repo, d(1), {"amd64": d(2), "arm64": d(3)})
        self.r.report(self.repo, d(2), "0.5.2")
        self.r.report(self.repo, d(3), "0.5.2")
        self.assertEqual(image_pins.spacesd_of(self.r, self.repo, d(1)), "0.5.2")

    def test_child_without_report_does_not_count(self) -> None:
        # A hosted arm64 disk that only boot-smoked has no report.
        self.r.index(self.repo, d(1), {"amd64": d(2), "arm64": d(3)})
        self.r.report(self.repo, d(2), "0.5.2")
        self.assertEqual(image_pins.spacesd_of(self.r, self.repo, d(1)), "0.5.2")

    def test_newest_passing_report_wins(self) -> None:
        self.r.report(self.repo, d(1), "0.5.1", created="2026-10-02T00:00:00Z")
        self.r.report(self.repo, d(1), "0.5.2", created="2026-10-03T00:00:00Z")
        self.r.report(self.repo, d(1), "0.5.3", created="2026-10-04T00:00:00Z", status="fail")
        self.assertEqual(image_pins.spacesd_of(self.r, self.repo, d(1)), "0.5.2")

    def test_no_report_and_mixed(self) -> None:
        self.assertIsNone(image_pins.spacesd_of(self.r, self.repo, d(1)))
        self.r.index(self.repo, d(5), {"amd64": d(6), "arm64": d(7)})
        self.r.report(self.repo, d(6), "0.5.2")
        self.r.report(self.repo, d(7), "0.5.1")
        self.assertEqual(image_pins.spacesd_of(self.r, self.repo, d(5)), "mixed:0.5.1,0.5.2")


class PinsTests(unittest.TestCase):
    """A two-OS catalog: linux (two tags) and macos (one tag, a README row)."""

    def setUp(self) -> None:
        self.tmp = tempfile.mkdtemp()
        self.catalog = os.path.join(self.tmp, "catalog.json")
        self.readme = os.path.join(self.tmp, "README.md")
        self.data = {
            "images": [
                {"ref": "ghcr.io/trycua/linux:24.04", "group": "canonical", "spacesd": True, "digest": d(10)},
                {"ref": "ghcr.io/trycua/linux:24.04-disk", "group": "canonical", "spacesd": True, "digest": d(11)},
                {"ref": "ghcr.io/trycua/macos:26", "group": "canonical", "spacesd": True, "digest": d(20)},
                {"ref": "ghcr.io/trycua/macos:15", "group": "canonical", "spacesd": False, "digest": d(30)},
                {"ref": "ghcr.io/trycua/macos:26-xcode", "group": "canonical", "spacesd": True},
                {"ref": "ghcr.io/trycua/bench-web:1.0", "group": "benchmark", "spacesd": True, "digest": d(40)},
            ]
        }
        self._write_catalog()
        with open(self.readme, "w") as fh:
            fh.write(
                "| `ghcr.io/trycua/macos:26` | full tier; pin `26-20261001-aaaaaaa` |\n"
                "| `ghcr.io/trycua/macos:26-slim` | slim tier; pin `26-slim-20261001-aaaaaaa` |\n"
            )
        self.r = FakeRegistry()
        for ref, digest in (
            ("ghcr.io/trycua/linux:24.04", d(10)),
            ("ghcr.io/trycua/linux:24.04-disk", d(11)),
            ("ghcr.io/trycua/macos:26", d(20)),
            (MACOS + ":26-20261001-aaaaaaa", d(20)),
        ):
            self.r.tag_digests[ref] = digest
        for repo, digest in (("ghcr.io/trycua/linux", d(10)), ("ghcr.io/trycua/linux", d(11)), ("ghcr.io/trycua/macos", d(20))):
            self.r.report(repo, digest, "0.5.1")

    def tearDown(self) -> None:
        shutil.rmtree(self.tmp)

    def _write_catalog(self) -> None:
        with open(self.catalog, "w") as fh:
            json.dump(self.data, fh)

    def oses(self, version: str = "0.5.2") -> list[image_pins.OS]:
        oses = image_pins.catalog_oses(image_pins.load_catalog(self.catalog))
        image_pins.resolve(self.r, oses, version)
        return oses

    def publish(self, ref: str, digest: str, version: str, pin: str | None = None) -> None:
        self.r.tag_digests[ref] = digest
        self.r.report(image_pins.repo_of(ref), digest, version)
        if pin:
            self.r.tag_digests[f"{image_pins.repo_of(ref)}:{pin}"] = digest

    def fake_refresh(self, refs: list[str]) -> None:
        for img in self.data["images"]:
            if img["ref"] in refs:
                img["digest"] = self.r.tag_digests[img["ref"]]
        self._write_catalog()

    def test_only_spacesd_oses_with_digests(self) -> None:
        oses = image_pins.catalog_oses(image_pins.load_catalog(self.catalog))
        self.assertEqual([(o.key, [t.ref for t in o.tags]) for o in oses], [
            ("linux", ["ghcr.io/trycua/linux:24.04", "ghcr.io/trycua/linux:24.04-disk"]),
            ("macos", ["ghcr.io/trycua/macos:26"]),
        ])

    def test_green_run_that_pushed_nothing_is_pending_and_flagged(self) -> None:
        oses = self.oses()
        self.assertEqual([o.status for o in oses], ["pending", "pending"])
        silent = image_pins.silent_pushes(oses, {"linux": "success"})
        self.assertEqual([o.key for o in silent], ["linux"])
        self.assertIn("pushed nothing that reports 0.5.2", image_pins.body(oses, "0.5.2", results={"linux": "success"}))

    def test_an_os_moves_only_when_every_tag_reports_the_version(self) -> None:
        self.publish("ghcr.io/trycua/linux:24.04", d(12), "0.5.2")
        self.assertEqual(self.oses()[0].status, "pending")
        self.publish("ghcr.io/trycua/linux:24.04-disk", d(13), "0.5.2")
        oses = self.oses()
        self.assertEqual([o.status for o in oses], ["ready", "pending"])
        moved = image_pins.apply(self.r, oses, refresh=self.fake_refresh, catalog_path=self.catalog, readme_path=self.readme)
        self.assertEqual(moved, ["ghcr.io/trycua/linux:24.04", "ghcr.io/trycua/linux:24.04-disk"])
        self.assertEqual([o.status for o in self.oses()], ["pinned", "pending"])
        body = image_pins.body(oses, "0.5.2")
        self.assertIn("- [x] **Linux**", body)
        self.assertIn("- [ ] **macOS**", body)
        self.assertIn("scripts/images/release-macos.sh 0.5.2", body)
        self.assertIn(f"| `linux:24.04-disk` | `{d(13)}` | 0.5.2 |", body)

    def test_readme_pin_follows_the_tag(self) -> None:
        self.publish("ghcr.io/trycua/macos:26", d(21), "0.5.2", pin="26-20261003-bbbbbbb")
        self.r.tag_digests[MACOS + ":26-20261003-bbbbbbb-raw"] = d(77)
        oses = self.oses()
        image_pins.apply(self.r, oses, refresh=self.fake_refresh, catalog_path=self.catalog, readme_path=self.readme)
        with open(self.readme) as fh:
            text = fh.read()
        self.assertIn("full tier; pin `26-20261003-bbbbbbb` |", text)
        self.assertIn("pin `26-slim-20261001-aaaaaaa`", text)  # another tag's row is untouched

    def test_refuses_a_digest_that_moved_after_the_check(self) -> None:
        self.publish("ghcr.io/trycua/macos:26", d(21), "0.5.2", pin="26-20261003-bbbbbbb")
        oses = self.oses()

        def racing_refresh(refs: list[str]) -> None:
            self.r.tag_digests["ghcr.io/trycua/macos:26"] = d(22)
            self.fake_refresh(refs)

        with self.assertRaises(SystemExit):
            image_pins.apply(self.r, oses, refresh=racing_refresh, catalog_path=self.catalog, readme_path=self.readme)

    def test_lag_warns_per_os_and_never_fails(self) -> None:
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            oses = image_pins.catalog_oses(image_pins.load_catalog(self.catalog))
            behind = image_pins.lag(self.r, oses, "0.5.2")
            oses = image_pins.catalog_oses(image_pins.load_catalog(self.catalog))
            current = image_pins.lag(self.r, oses, "0.5.1")
        self.assertEqual(len(behind), 3)
        self.assertEqual(current, [])
        warnings = [line for line in out.getvalue().splitlines() if line.startswith("::warning")]
        self.assertEqual(len(warnings), 2)  # one per OS

    def test_refresh_keeps_stdout_for_the_summary(self) -> None:
        # pins-pr.sh parses apply's stdout as JSON: record-image-sizes.py's
        # progress lines must not land there (they once skipped the parity
        # goldens and left the pins PR red).
        script = os.path.join(self.tmp, "record-image-sizes.py")
        with open(script, "w") as fh:
            fh.write("import sys\nprint('record-image-sizes: %d image(s) measured' % sys.argv[1:].count('--ref'))\n")
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            image_pins.refresh_sizes(["ghcr.io/trycua/linux:24.04"], script=script)
        self.assertEqual(out.getvalue(), "")
        self.assertIn("record-image-sizes: 1 image(s) measured", err.getvalue())

    def test_parse_results(self) -> None:
        self.assertEqual(
            image_pins.parse_results("linux=success, omarchy=failure,windows=skipped"),
            {"linux": "success", "omarchy": "failure", "windows": "skipped"},
        )


def read(name: str) -> str:
    with open(os.path.join(WORKFLOWS, name)) as fh:
        return fh.read()


def top_block(text: str, key: str, indent: int) -> str:
    """The lines under `key:` at this indent, up to the next key at the same or lower indent."""
    lines = text.splitlines()
    pad = " " * indent
    for i, line in enumerate(lines):
        if line == f"{pad}{key}:" or line.startswith(f"{pad}{key}: "):
            out = []
            for nxt in lines[i + 1:]:
                stripped = nxt.lstrip(" ")
                if stripped and not stripped.startswith("#") and len(nxt) - len(stripped) <= indent:
                    break
                out.append(nxt)
            return "\n".join(out)
    raise AssertionError(f"no {key!r} at indent {indent}")


def job_permissions(text: str) -> dict[str, str]:
    """Union of every job-level permissions block: scope -> strongest grant."""
    rank = {"none": 0, "read": 1, "write": 2}
    out: dict[str, str] = {}
    for block in re.findall(r"^    permissions:\n((?:      [a-z-]+: \w+.*\n|      #.*\n)+)", text, re.M):
        for scope, grant in re.findall(r"^      ([a-z-]+): (\w+)", block, re.M):
            if rank[grant] > rank.get(out.get(scope, "none"), 0):
                out[scope] = grant
    return out


class ReleaseWiringTests(unittest.TestCase):
    FANOUT = "cd-images-spacesd-release.yml"
    CALLS = {
        "linux": ("cd-image-linux.yml", {"publish", "spacesd_source", "spacesd_version"}),
        "omarchy": ("cd-image-omarchy.yml", {"publish", "series"}),
        "windows": ("cd-image-windows.yml", {"stage"}),
        "bench": ("cd-bench-images.yml", {"bench", "push"}),
    }

    def setUp(self) -> None:
        self.fanout = read(self.FANOUT)
        self.jobs = top_block(self.fanout, "jobs", 0)

    def test_triggered_by_the_cua_spacesd_cd_by_its_exact_name(self) -> None:
        name = re.search(r'^name: "([^"]+)"', read("cd-cua-spacesd.yml"), re.M).group(1)
        self.assertIn(f'workflows: ["{name}"]', self.fanout)
        self.assertIn("startsWith(github.event.workflow_run.head_branch, 'cua-spacesd-v')", self.fanout)
        self.assertIn("github.event.workflow_run.conclusion == 'success'", self.fanout)
        self.assertIn("workflow_dispatch:", top_block(self.fanout, "on", 0))

    def test_calls_each_image_workflow_with_inputs_it_declares(self) -> None:
        for job, (callee, inputs) in self.CALLS.items():
            block = top_block(self.jobs, job, 2)
            self.assertIn(f"uses: ./.github/workflows/{callee}", block)
            passed = set(re.findall(r"^      ([a-z_]+):", top_block(block, "with", 4), re.M))
            self.assertEqual(passed, inputs, job)
            call = top_block(top_block(read(callee), "on", 0), "workflow_call", 2)
            declared = set(re.findall(r"^      ([a-z_]+):$", top_block(call, "inputs", 4), re.M))
            self.assertLessEqual(passed, declared, callee)
            # Manual runs keep working.
            self.assertIn("workflow_dispatch:", top_block(read(callee), "on", 0), callee)

    def test_publishes(self) -> None:
        self.assertIn("publish: true", top_block(self.jobs, "linux", 2))
        self.assertIn("spacesd_source: release", top_block(self.jobs, "linux", 2))
        self.assertIn("publish: true", top_block(self.jobs, "omarchy", 2))
        self.assertIn("stage: publish", top_block(self.jobs, "windows", 2))
        self.assertIn("push: true", top_block(self.jobs, "bench", 2))

    def test_grants_cover_every_called_job(self) -> None:
        rank = {"none": 0, "read": 1, "write": 2}
        for job, (callee, _) in self.CALLS.items():
            granted = dict(re.findall(r"^      ([a-z-]+): (\w+)", top_block(top_block(self.jobs, job, 2), "permissions", 4), re.M))
            for scope, grant in job_permissions(read(callee)).items():
                self.assertGreaterEqual(rank[granted.get(scope, "none")], rank[grant], f"{job}: {scope}: {grant}")

    def test_called_publish_paths_accept_the_callers_event(self) -> None:
        # A called workflow sees its caller's event (workflow_run).
        for callee in ("cd-image-linux.yml", "cd-image-omarchy.yml"):
            self.assertIn('workflow_dispatch|workflow_run) [ "$INPUT_PUBLISH" = true ] && publish=true', read(callee))
        meta = top_block(top_block(read("cd-bench-images.yml"), "jobs", 0), "meta", 2)
        self.assertIn("if: github.event_name != 'pull_request'", meta)

    def test_pins_job(self) -> None:
        pins = top_block(self.jobs, "pins", 2)
        self.assertIn("needs: [resolve, linux, omarchy, windows, bench]", pins)
        self.assertIn("!cancelled()", pins)
        self.assertIn("secrets.RELEASE_APP_ID", pins)
        self.assertIn("secrets.RELEASE_APP_PRIVATE_KEY", pins)
        self.assertIn('scripts/images/pins-pr.sh "$VERSION"', pins)
        self.assertTrue(os.access(os.path.join(ROOT, "scripts/images/pins-pr.sh"), os.X_OK))
        self.assertTrue(os.access(os.path.join(ROOT, "scripts/images/release-macos.sh"), os.X_OK))

    def test_pins_pr_regenerates_everything_the_catalog_feeds(self) -> None:
        with open(os.path.join(ROOT, "scripts/images/pins-pr.sh")) as fh:
            script = fh.read()
        # The summary is parsed strictly, outside a conditional that would
        # swallow a jq failure.
        self.assertRegex(script, r"(?m)^moved=\"\$\(jq -e '\.moved \| length'")
        self.assertIn("UPDATE_PARITY=1 cargo test --locked -p cua-spaces-app-core --test parity", script)
        # Docs: the generators the docs check routes the change to.
        self.assertIn("scripts/docs-generators/runner.ts --changed-files-file", script)
        self.assertIn("libs/cua/crates/cua-spaces-app-core/parity/golden docs/content)", script)
        self.assertIn('git add -A -- "${FILES[@]}"', script)

    def test_docs_generators_fed_by_the_catalog_write_where_pins_pr_stages(self) -> None:
        with open(os.path.join(ROOT, "scripts/docs-generators/config.json")) as fh:
            generators = json.load(fh)["generators"]
        fed = {
            k: g for k, g in generators.items()
            if g.get("enabled") and {"libs/images/sandbox-images.json", "libs/images/README.md"} & set(g.get("watchPaths", []))
        }
        self.assertIn("sandbox", fed)
        for key, g in fed.items():
            self.assertTrue(g["docsOutputPath"].startswith("docs/content/"), key)
            self.assertFalse(g.get("buildCommand"), f"{key} needs a build pins-pr.sh does not run")

    def test_macos_command_refreshes_this_workflow(self) -> None:
        with open(os.path.join(ROOT, "scripts/images/release-macos.sh")) as fh:
            script = fh.read()
        self.assertIn(f"gh workflow run {self.FANOUT}", script)
        self.assertIn("-f build=false", script)
        self.assertIn("build:", top_block(top_block(self.fanout, "on", 0), "workflow_dispatch", 2))

    def test_macos_keychain_check_gates_each_publish(self) -> None:
        with open(os.path.join(ROOT, "scripts/images/release-macos.sh")) as fh:
            script = fh.read()
        loop = script[script.index('for tier in slim full; do'):]
        check = loop.index('keychain_check "$PREFIX-$tier"')
        self.assertLess(check, loop.index("--resume --publish"))
        self.assertIn("security unlock-keychain -p lume", script)
        self.assertIn("grep -q '^RENAMED=0$'", script)

    def test_lag_check_never_blocks(self) -> None:
        lag = read("ci-images-spacesd-lag.yml")
        self.assertIn("continue-on-error: true", lag)
        self.assertIn("python3 scripts/images/image_pins.py lag", lag)
        for path in ("libs/images/sandbox-images.json", "libs/cua-spacesd/VERSION", ".release-please-manifest.json"):
            self.assertIn(f'- "{path}"', lag)


if __name__ == "__main__":
    unittest.main()
