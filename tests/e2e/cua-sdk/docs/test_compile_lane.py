"""Unit tests for the swift / kotlin / rust compile lanes (no toolchain needed)."""

from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

import compile_lane as cl
import extract


class CompileLaneTest(unittest.TestCase):
    def test_target_name(self):
        self.assertEqual(
            cl.target_name("reference/cua-sdk/index#connect-url"),
            "DocsReferenceCuaSdkIndexConnectUrl",
        )

    def test_swift_wrap_hoists_imports(self):
        out = cl.wrap(
            "import Cua\n\nlet c = try Cua.embedded()\nprint(c)\n",
            "swift",
            "DocsX",
            "import Foundation\n",
        )
        lines = out.splitlines()
        self.assertEqual(lines[:2], ["import Foundation", "import Cua"])
        self.assertIn("func docsX() async throws {", out)
        self.assertIn("    let c = try Cua.embedded()", out)
        self.assertTrue(out.rstrip().endswith("}"))

    def test_kotlin_wrap(self):
        out = cl.wrap(
            'import ai.cua.sdk.imageAlias\nprintln(imageAlias("linux"))\n', "kotlin", "DocsY"
        )
        self.assertTrue(out.startswith("package docs.docsy\n"))
        self.assertIn("import ai.cua.sdk.imageAlias", out)
        self.assertIn("suspend fun docsY() {", out)

    def test_generated_projects(self):
        tmp = Path(tempfile.mkdtemp())
        (tmp / "p.mdx").write_text('```swift test="swift" id="a"\nimport Cua\nlet x = 1\n```\n')
        blocks = extract.all_blocks(tmp)
        names = cl.swift_package(tmp / "pkg", blocks)
        pkg = (tmp / "pkg" / "Package.swift").read_text()
        self.assertIn('.target(name: "DocsPA"', pkg)
        self.assertIn(str(cl.SWIFT_PKG), pkg)
        self.assertTrue((tmp / "pkg" / "Sources" / names[0] / "Block.swift").exists())
        kt = cl.kotlin_project(tmp / "kt", blocks)
        self.assertTrue((tmp / "kt" / "src" / "main" / "kotlin" / f"{kt[0]}.kt").exists())

    def test_attribute_errors(self):
        tmp = Path(tempfile.mkdtemp())
        (tmp / "p.mdx").write_text(
            '```swift test="swift" id="a"\nx\n```\n```swift test="swift" id="b"\ny\n```\n'
        )
        blocks = extract.all_blocks(tmp)
        names = [cl.target_name(b.id) for b in blocks]
        out = f"/w/Sources/{names[1]}/Block.swift:3:5: error: bad\ne: file:///w/src/{names[0]}.kt:1:1 wrong\n"
        errs = cl.attribute(out, names, blocks)
        self.assertEqual(sorted(errs), ["p#a", "p#b"])

    def test_rust_wrap(self):
        whole = "use cua_sdk::Cua;\n\n#[tokio::main]\nasync fn main() {}\n"
        self.assertEqual(cl.wrap(whole, "rust", "DocsR"), whole)
        out = cl.wrap("use cua_sdk::{Cua, CuaConfig};\n\nlet c = Cua::embedded(CuaConfig::default())?;\n", "rust", "DocsR")
        lines = out.splitlines()
        self.assertEqual(lines[:2], ["#![allow(unused)]", "use cua_sdk::{Cua, CuaConfig};"])
        self.assertIn("async fn docs_r() -> Result<(), Box<dyn std::error::Error>> {", out)
        self.assertIn("    let c = Cua::embedded(CuaConfig::default())?;", out)
        self.assertIn("fn main() {}", out)

    def test_rust_package(self):
        tmp = Path(tempfile.mkdtemp())
        (tmp / "p.mdx").write_text('```rust test="rust" id="a"\nlet x = 1;\n```\n')
        blocks = extract.all_blocks(tmp)
        names = cl.rust_package(tmp / "rs", blocks)
        manifest = (tmp / "rs" / "Cargo.toml").read_text()
        self.assertIn(str(cl.RUST_SDK), manifest)
        self.assertIn("[workspace]", manifest)
        self.assertTrue((tmp / "rs" / "Cargo.lock").exists())
        self.assertTrue((tmp / "rs" / "src" / "bin" / f"{names[0]}.rs").exists())

    def test_attribute_rust_errors(self):
        tmp = Path(tempfile.mkdtemp())
        (tmp / "p.mdx").write_text(
            '```rust test="rust" id="a"\nx\n```\n```rust test="rust" id="b"\ny\n```\n'
        )
        blocks = extract.all_blocks(tmp)
        names = [cl.target_name(b.id) for b in blocks]
        out = f"error[E0599]: no method named `x`\n  --> src/bin/{names[1]}.rs:3:5\n"
        self.assertEqual(sorted(cl.attribute(out, names, blocks)), ["p#b"])


if __name__ == "__main__":
    unittest.main()
