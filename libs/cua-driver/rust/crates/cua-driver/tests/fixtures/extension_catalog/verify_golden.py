#!/usr/bin/env python3
"""Reproduce the committed catalog bytes with Python and sign them with Node."""

import base64
import json
import pathlib
import subprocess

HERE = pathlib.Path(__file__).resolve().parent
GOLDEN = json.loads((HERE / "signed-catalog.golden.json").read_text())
CANONICAL = json.dumps(GOLDEN["payload"], separators=(",", ":")).encode()

assert CANONICAL.decode() == GOLDEN["canonical_payload"]
signed = json.loads(
    subprocess.run(
        ["node", str(HERE / "sign_catalog.js")],
        input=CANONICAL,
        check=True,
        capture_output=True,
    ).stdout
)
assert signed["public_key_base64"] == GOLDEN["public_key_base64"]
assert signed["signature"] == GOLDEN["signature"]
assert len(base64.b64decode(signed["public_key_base64"])) == 32
print("Python canonical catalog and Node Ed25519 signature match the Rust golden vector")
