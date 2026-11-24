"""Tests for the repo image-ref gate and the image constants generator (python3 -m unittest).

Fixture refs are assembled from pieces so this file itself holds no image
reference for the gate to find.
"""

from __future__ import annotations

import importlib.util
import json
import os
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", "..", ".."))


def load(name: str, file: str):
    spec = importlib.util.spec_from_file_location(name, os.path.join(os.path.dirname(HERE), file))
    mod = importlib.util.module_from_spec(spec)
    sys.modules[name] = mod  # dataclasses look their module up
    spec.loader.exec_module(mod)
    return mod


gate = load("check_image_refs", "check-image-refs.py")
gen = load("gen_image_constants", "gen-image-constants.py")

GH = "ghcr" + ".io/" + "trycua"
ECR = "public" + ".ecr.aws/" + "k5j5w0x5"
HUB = "docker" + ".io/library"
LINUX = f"{GH}/linux:24.04"
BENCH = f"{GH}/bench-web"
PIN = "sha256:" + "7" * 64
OLD = "sha256:" + "6" * 64


def catalog() -> dict:
    return {
        "schemaVersion": 1,
        "groups": [{"id": "canonical", "label": "Cua"}, {"id": "benchmark", "label": "Bench"}],
        "images": [
            {"ref": LINUX, "group": "canonical", "published": True},
            {
                "ref": f"{BENCH}:1.0",
                "group": "benchmark",
                "published": False,
                "lock": "libs/images/bench/web/lock.json",
            },
        ],
    }


def lock() -> dict:
    return {
        "repository": BENCH,
        "index": {"ref": f"{BENCH}:1.0-20260101-abcdef0", "digest": PIN},
        "disk_index": {"ref": f"{BENCH}:1.0-disk-20260101-abcdef0", "digest": "sha256:" + "8" * 64},
        "children": [{"arch": "amd64", "rootfs": "sha256:" + "5" * 64}],
    }


def allowlist(**over) -> dict:
    base = {
        "exclude": ["blog/**"],
        "generated": ["gen/*"],
        "thirdParty": {f"{HUB}/*": "upstream"},
        "refs": {
            f"{GH}/cua-desktop-linux:latest": {"reason": "legacy", "paths": ["tests/legacy.rs"]}
        },
        "repos": {f"{ECR}/cua-ubuntu-24.04": "cloud"},
    }
    base.update(over)
    return base


class GateTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.root = self.tmp.name
        self.write("libs/images/sandbox-images.json", json.dumps(catalog()))
        self.write("libs/images/bench/web/lock.json", json.dumps(lock()))
        self.write("scripts/images/image-refs-allowlist.json", json.dumps(allowlist()))
        self.write("docs/guide.mdx", f"Run `{LINUX}`. Base: {HUB}/python:3.12-slim.\n")
        self.write("tests/legacy.rs", f'let r = "{GH}/cua-desktop-linux:latest";\n')
        self.write("bench.py", f'IMAGE = "{BENCH}@{PIN}"\n')
        self.write("gen/out.md", f"{LINUX}\n")
        self.write("ci.yml", f"repo: {ECR}/cua-ubuntu-24.04\n")

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def write(self, rel: str, text: str) -> None:
        path = os.path.join(self.root, rel)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w", encoding="utf-8") as fh:
            fh.write(text)

    def errors(self) -> list[str]:
        return gate.run(self.root).errors

    def test_clean_tree_passes(self) -> None:
        self.assertEqual(self.errors(), [])

    def test_unknown_cua_ref_fails(self) -> None:
        self.write("samples/app.ts", f'const image = "{GH}/linux:23.10"\n')
        (err,) = self.errors()
        self.assertIn("samples/app.ts:1", err)
        self.assertIn("stale tag", err)
        self.write("samples/app.ts", f'const image = "{GH}/brand-new:1"\n')
        self.assertIn("unknown cua image", self.errors()[0])

    def test_bare_docker_hub_trycua_ref_is_checked(self) -> None:
        self.write("Dockerfile", "FROM " + "try" + "cua/old-desktop:latest\n")
        self.assertIn("docker" + ".io/try" + "cua/old-desktop:latest", self.errors()[0])

    def test_bench_pin_ok_but_old_digest_fails(self) -> None:
        self.write("bench.py", f'IMAGE = "{BENCH}@{OLD}"\n')
        (err,) = self.errors()
        self.assertIn("unpinned digest", err)

    def test_allowlisted_ref_only_in_its_paths(self) -> None:
        self.write("docs/old.mdx", f"`{GH}/cua-desktop-linux:latest`\n")
        (err,) = self.errors()
        self.assertIn("only allowed in tests/legacy.rs", err)

    def test_frozen_legacy_name_fails_outside_its_paths(self) -> None:
        old = "cua-desktop" + "-linux"
        legacy = {old: {"reason": "frozen", "use": "linux",
                        "pattern": r"(?<![\w-])" + old + r"(?![\w-])",
                        "paths": ["tests/legacy.rs"]}}
        self.write("scripts/images/image-refs-allowlist.json", json.dumps(allowlist(legacy=legacy)))
        self.assertEqual(self.errors(), [])
        # Any new use fails: a path, a local tag, prose; not a longer name.
        self.write("scripts/build.sh", f"libs/images/{old}/build.sh\nIMAGE=cua-e2e-local/{old}:x\n")
        self.write("docs/a.md", f"The {old} image.\nThe cua-e2e-desktop-linux-docker tag.\n")
        errs = self.errors()
        self.assertEqual(len(errs), 3, errs)
        self.assertTrue(all("new uses are not allowed" in e for e in errs), errs)
        self.assertIn("docs/a.md:1", " ".join(errs))
        # A legacy path that no longer holds the name is stale.
        self.write("tests/legacy.rs", "let r = 1;\n")
        self.write("scripts/images/image-refs-allowlist.json",
                   json.dumps(allowlist(legacy=legacy, refs={})))
        self.write("scripts/build.sh", "ok\n")
        self.write("docs/a.md", "ok\n")
        (err,) = self.errors()
        self.assertIn("legacy[" + old + "] path tests/legacy.rs matches nothing", err)

    def test_unknown_third_party_fails(self) -> None:
        self.write("x.py", "IMG = 'quay" + ".io/someone/tool:1'\n")
        self.assertIn("unknown third-party image", self.errors()[0])

    def test_stale_allowlist_entries_fail(self) -> None:
        os.remove(os.path.join(self.root, "tests/legacy.rs"))
        os.remove(os.path.join(self.root, "ci.yml"))
        self.write(
            "scripts/images/image-refs-allowlist.json",
            json.dumps(
                allowlist(
                    thirdParty={f"{HUB}/*": "upstream", "quay.io/unused/*": "nothing"},
                    generated=["gen/*", "nowhere/*"],
                )
            ),
        )
        errs = "\n".join(self.errors())
        self.assertIn("path tests/legacy.rs matches nothing", errs)
        self.assertIn("thirdParty quay.io/unused/* matches nothing", errs)
        self.assertIn("repos public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04 matches nothing", errs)
        self.assertIn("generated nowhere/* matches no file", errs)

    def test_catalog_ref_in_allowlist_is_redundant(self) -> None:
        self.write(
            "scripts/images/image-refs-allowlist.json",
            json.dumps(
                allowlist(
                    refs={
                        f"{GH}/cua-desktop-linux:latest": {
                            "reason": "legacy",
                            "paths": ["tests/legacy.rs"],
                        },
                        LINUX: {"reason": "dup", "paths": ["docs/guide.mdx"]},
                    }
                )
            ),
        )
        self.assertIn("is a catalog image or pin", "\n".join(self.errors()))

    def test_unknown_cua_repo_fails(self) -> None:
        self.write("ci.yml", f"repo: {GH}/mystery\n")
        self.assertIn("unknown cua repository", "\n".join(self.errors()))

    def test_excluded_files_and_templates_are_skipped(self) -> None:
        self.write("blog/post.md", f"{GH}/ancient:1\n")
        self.write(
            "docs/ci.mdx", f"push {GH}/cua-desktop-linux:e2e-<sha> and {GH}/linux:${{TAG}}\n"
        )
        self.assertEqual(self.errors(), [])

    def test_sentence_period_is_not_part_of_the_tag(self) -> None:
        self.write("docs/p.mdx", f"Use {LINUX}.\n")
        self.assertEqual(self.errors(), [])

    def test_generated_files_and_regions_are_counted_apart(self) -> None:
        self.write(
            "docs/page.mdx",
            f"{{/* GENERATED:x:start */}}\n{LINUX}\n{{/* GENERATED:x:end */}}\n{LINUX}\n",
        )
        result = gate.run(self.root)
        page = [h.generated for h in result.hits if h.path == "docs/page.mdx"]
        self.assertEqual(page, [True, False])
        self.assertTrue(all(h.generated for h in result.hits if h.path == "gen/out.md"))
        text = gate.inventory(result, self.root)
        self.assertIn("## By image", text)
        self.assertIn("## By file kind", text)


class RepoTests(unittest.TestCase):
    def test_repo_passes_the_gate(self) -> None:
        errors = gate.run(ROOT).errors
        self.assertEqual(errors, [], "\n".join(errors[:20]))

    def test_generated_constants_are_current(self) -> None:
        with open(os.path.join(ROOT, gen.OUT), encoding="utf-8") as fh:
            self.assertEqual(
                fh.read(), gen.render(), "run python3 scripts/images/gen-image-constants.py"
            )

    def test_bench_constants_are_the_lock_pins(self) -> None:
        with open(os.path.join(ROOT, gate.CATALOG), encoding="utf-8") as fh:
            images = json.load(fh)["images"]
        rendered = gen.render()
        for image in images:
            if image["group"] == "benchmark" and image["variant"] == "container":
                with open(os.path.join(ROOT, image["lock"]), encoding="utf-8") as fh:
                    digest = json.load(fh)["index"]["digest"]
                self.assertIn(f'"{image["ref"].split(":")[0]}@{digest}"', rendered)


if __name__ == "__main__":
    unittest.main()
