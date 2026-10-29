#!/usr/bin/env python3
"""Extracts the protobuf descriptors of the Modal Python client's gRPC API.

The `modal` wheel (Apache-2.0, (c) Modal Labs) ships its generated
`modal_proto/*_pb2.py` modules, which embed the serialized file descriptors
of `api.proto` and `task_command_router.proto`. This script loads them from
a pinned wheel, writes one `FileDescriptorSet` (the files and the well-known
dependencies they import) and a `surface.json` listing every RPC and
message so a drift check can diff a newer client against the pinned one.

Only the descriptors are used: no client code is read or copied. The
gateway's server is generated from them at build time (`build.rs`) and the
implementation behind it is original.

Usage:
    extract_descriptors.py [--wheel PATH | --version X.Y.Z] [--out DIR]

With `--version` the wheel is downloaded with pip (hash-checked against
the pin when it is the pinned version). Output goes to DIR (default: this
directory) as `modal-<version>.fds` and `surface-<version>.json`; with
`--surface-only` only the surface JSON is written (the weekly drift job).
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import sys
import tempfile
import zipfile

HERE = os.path.dirname(os.path.abspath(__file__))
PINNED_VERSION = "1.6.2"
# sha256 of modal-1.6.2-py3-none-any.whl as published on PyPI.
PINNED_SHA256 = "1a6c8409e315785137615601cac940de3d72462c0059b9cd0faf5d17615bacf0"
FILES = ("modal_proto/api.proto", "modal_proto/task_command_router.proto")


def fetch_wheel(version: str, dest: str) -> str:
    """Downloads the wheel from PyPI (stdlib only) and checks the digest
    PyPI publishes for it."""
    import urllib.request

    with urllib.request.urlopen(f"https://pypi.org/pypi/modal/{version}/json", timeout=60) as r:
        info = json.load(r)
    wheels = [u for u in info["urls"] if u["packagetype"] == "bdist_wheel"]
    if not wheels:
        raise SystemExit(f"modal {version} has no wheel on PyPI")
    w = wheels[0]
    path = os.path.join(dest, w["filename"])
    with urllib.request.urlopen(w["url"], timeout=300) as r, open(path, "wb") as f:
        f.write(r.read())
    if sha256(path) != w["digests"]["sha256"]:
        raise SystemExit(f"{w['filename']}: digest differs from PyPI's")
    return path


def sha256(path: str) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def load(wheel: str, work: str):
    with zipfile.ZipFile(wheel) as z:
        names = [n for n in z.namelist() if n.startswith("modal_proto/") and n.endswith(".py")]
        z.extractall(work, members=names)
    sys.path.insert(0, work)
    from google.protobuf import descriptor_pb2  # noqa: F401
    from modal_proto import api_pb2, task_command_router_pb2  # noqa: E402
    return api_pb2, task_command_router_pb2


def collect(fd, seen: dict):
    """Depth-first: dependencies before dependents (the order protoc emits)."""
    from google.protobuf import descriptor_pb2

    if fd.name in seen:
        return
    for dep in fd.dependencies:
        collect(dep, seen)
    proto = descriptor_pb2.FileDescriptorProto()
    fd.CopyToProto(proto)
    seen[fd.name] = proto


def surface(modules) -> dict:
    out = {"services": {}, "messages": {}, "enums": {}}
    for mod in modules:
        fd = mod.DESCRIPTOR
        for svc in fd.services_by_name.values():
            out["services"][svc.full_name] = {
                m.name: {
                    "in": m.input_type.full_name,
                    "out": m.output_type.full_name,
                    "client_streaming": bool(m.client_streaming),
                    "server_streaming": bool(m.server_streaming),
                }
                for m in svc.methods
            }
        def walk(msg):
            out["messages"][msg.full_name] = {
                f.name: [f.number, f.type, bool(f.is_repeated), f.message_type.full_name if f.message_type else (f.enum_type.full_name if f.enum_type else "")]
                for f in msg.fields
            }
            for n in msg.nested_types:
                if not n.GetOptions().map_entry:
                    walk(n)
            for e in msg.enum_types:
                out["enums"][e.full_name] = {v.name: v.number for v in e.values}
        for msg in fd.message_types_by_name.values():
            walk(msg)
        for e in fd.enum_types_by_name.values():
            out["enums"][e.full_name] = {v.name: v.number for v in e.values}
    return out


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--wheel")
    ap.add_argument("--version", default=PINNED_VERSION)
    ap.add_argument("--out", default=HERE)
    ap.add_argument("--surface-only", action="store_true")
    args = ap.parse_args()

    with tempfile.TemporaryDirectory() as tmp:
        wheel = args.wheel or fetch_wheel(args.version, tmp)
        digest = sha256(wheel)
        version = args.version
        if args.wheel:
            version = os.path.basename(wheel).split("-")[1]
        if version == PINNED_VERSION and digest != PINNED_SHA256:
            raise SystemExit(f"modal {version} wheel hash {digest} != pinned {PINNED_SHA256}")
        api, tcr = load(wheel, os.path.join(tmp, "x"))
        surf = surface([api, tcr])
        surf["modal_version"] = version
        surf["wheel_sha256"] = digest
        os.makedirs(args.out, exist_ok=True)
        with open(os.path.join(args.out, f"surface-{version}.json"), "w") as f:
            json.dump(surf, f, indent=1, sort_keys=True)
            f.write("\n")
        if args.surface_only:
            return
        from google.protobuf import descriptor_pb2

        seen: dict = {}
        for mod in (api, tcr):
            collect(mod.DESCRIPTOR, seen)
        fds = descriptor_pb2.FileDescriptorSet(file=list(seen.values()))
        assert all(f in seen for f in FILES), list(seen)
        with open(os.path.join(args.out, f"modal-{version}.fds"), "wb") as f:
            f.write(fds.SerializeToString(deterministic=True))
        print(f"modal {version} ({digest[:12]}): {len(seen)} files, "
              f"{sum(len(s) for s in surf['services'].values())} rpcs, {len(surf['messages'])} messages")


if __name__ == "__main__":
    main()
