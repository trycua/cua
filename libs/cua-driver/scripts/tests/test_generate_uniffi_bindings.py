from __future__ import annotations

import json
import os
import shutil
import subprocess
from pathlib import Path

import pytest


GENERATOR = Path(__file__).resolve().parents[1] / "generate-uniffi-bindings.mjs"


def test_windows_generator_preserves_paths_and_canonical_normalization(tmp_path: Path) -> None:
    node = shutil.which("node")
    if node is None:
        pytest.skip("Node.js is required to exercise the binding generator")

    driver = tmp_path / "repo with spaces" / "libs" / "cua-driver"
    script = driver / "scripts" / GENERATOR.name
    script.parent.mkdir(parents=True)
    shutil.copyfile(GENERATOR, script)
    library = driver / "rust" / "target" / "release" / "cua_driver_sdk.dll"
    library.parent.mkdir(parents=True)
    library.touch()
    manifest = (
        driver / "typescript" / "node_modules" / "uniffi-bindgen-react-native"
        / "crates" / "ubrn_cli" / "Cargo.toml"
    )
    manifest.parent.mkdir(parents=True)
    manifest.touch()
    # Keep the old launcher present: regression must fail on dispatch, not setup.
    launcher = driver / "typescript" / "node_modules" / ".bin" / "ubrn.cmd"
    launcher.parent.mkdir(parents=True)
    launcher.touch()
    calls = tmp_path / "calls.jsonl"
    preload = tmp_path / "intercept.mjs"
    preload.write_text(
        r'''
import assert from "node:assert/strict";
import childProcess from "node:child_process";
import { appendFileSync, mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { syncBuiltinESMExports } from "node:module";

Object.defineProperty(process, "platform", { value: "win32" });
childProcess.spawnSync = (command, args, options) => {
  assert.equal(command, "cargo", "never launch an npm .cmd through spawnSync");
  assert.equal(options.shell, undefined, "generator must preserve argument boundaries");
  appendFileSync(process.env.CUA_TEST_CALLS, JSON.stringify({ command, args, cwd: options.cwd }) + "\n");
  if (args.includes("--ts-dir")) {
    const output = args[args.indexOf("--ts-dir") + 1];
    mkdirSync(output, { recursive: true });
    writeFileSync(join(output, "cua_driver_sdk-ffi.ts"), 'import lib from "@ubjs/node";\r\n');
    writeFileSync(join(output, "cua_driver_contract-ffi.ts"), 'import lib from "@ubjs/node";\r\nconst options = { crateName: "cua_driver_contract" };\r\n');
    writeFileSync(join(output, "index.ts"), 'export * from "./cua_driver_sdk";  \r\n');
  } else if (args.includes("--out-dir")) {
    const output = args[args.indexOf("--out-dir") + 1];
    mkdirSync(output, { recursive: true });
    writeFileSync(join(output, "cua_driver_sdk.py"), 'def _uniffi_future_dropped_callback(handle):\r\n    eventloop.call_soon(_uniffi_cancel_task, task)  \r\n');
    writeFileSync(join(output, "cua_driver_contract.py"), "# contract  \r\n");
  }
  return { status: 0 };
};
syncBuiltinESMExports();
''',
        encoding="utf-8",
    )
    environment = os.environ.copy()
    environment.pop("CARGO_TARGET_DIR", None)
    environment["CUA_TEST_CALLS"] = str(calls)
    for arguments in ([], ["--check"]):
        result = subprocess.run(
            [node, "--import", preload.as_uri(), str(script), *arguments],
            env=environment, text=True, capture_output=True, check=False, timeout=15,
        )
        assert result.returncode == 0, result.stdout + result.stderr

    invocations = [json.loads(line) for line in calls.read_text().splitlines()]
    assert len(invocations) == 6
    typescript = invocations[2]
    assert typescript["cwd"] == str(driver / "rust")
    assert typescript["args"][:9] == [
        "run", "--quiet", "--manifest-path", str(manifest), "--",
        "generate", "napi", "bindings", "--library",
    ]
    assert typescript["args"][9] == str(library)
    assert typescript["args"][-4:] == [
        "--lib-package-base", "@trycua/cua-driver", "--lib-node-triple", "--no-format",
    ]
    native = driver / "typescript" / "src" / "native"
    assert (native / "index.ts").read_text() == 'export * from "./cua_driver_sdk.js";\n'
    assert (native / "cua_driver_sdk-ffi.ts").read_text() == 'import lib from "./node-runtime.js";\n'
    assert 'crateName: "cua_driver_sdk"' in (native / "cua_driver_contract-ffi.ts").read_text()
    python = driver / "python" / "src" / "cua_driver" / "_native.py"
    assert "eventloop.call_soon_threadsafe(_uniffi_cancel_task, task)" in python.read_text()
