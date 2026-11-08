#!/usr/bin/env python3
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Write a pins file for one assembled private script app (see assemble_version.sh).

Takes the repository's pins.json and replaces its ``cua_main`` block with the identity of the build under test:
version, binary hash (the same for ``binary_sha256`` and ``script_binary_sha256``, there is one app), skills tree
hash and source commit. Runs with the VM's /usr/bin/python3 (3.9).
"""

import argparse
import hashlib
import json
import os
import subprocess
from pathlib import Path

HERE = Path(__file__).resolve().parent
PINS = HERE.parents[1] / "pins.json"


def sha256_file(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def sha256_tree(path):
    h = hashlib.sha256()
    for item in sorted(p for p in path.rglob("*") if p.is_file()):
        h.update(str(item.relative_to(path)).encode())
        h.update(sha256_file(item).encode())
    return h.hexdigest()


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--app", required=True, type=Path)
    p.add_argument("--skills", required=True, type=Path)
    p.add_argument("--git-sha", required=True)
    p.add_argument("--out", required=True, type=Path)
    a = p.parse_args()
    binary = a.app / "Contents/MacOS/cua-driver"
    version_string = subprocess.run([str(binary), "--version"], capture_output=True, text=True, check=True).stdout.strip()
    digest = sha256_file(binary)
    pins = json.loads(PINS.read_text("utf-8"))
    pins["cua_main"] = {
        "note": "Private script app assembled by tools/vm_main_build/assemble_version.sh",
        "git_sha": a.git_sha,
        "version": version_string.split()[-1],
        "version_string": version_string,
        "binary_sha256": digest,
        "script_binary_sha256": digest,
        "skills_tree_sha256": sha256_tree(a.skills),
        "skills_source": "the build's own skill directory",
    }
    observed = os.environ.get("CDB_OBSERVED_DIGESTS")  # JSON {pack_task: digest} of the VM's own task pack
    if observed:
        pins["cdb_pack"]["tree_sha256"].update(json.loads(Path(observed).read_text("utf-8")))
    a.out.write_text(json.dumps(pins, indent=2) + "\n", "utf-8")
    print("wrote", a.out, version_string, digest)


if __name__ == "__main__":
    main()
