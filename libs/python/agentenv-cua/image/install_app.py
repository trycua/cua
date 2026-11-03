#!/usr/bin/env python3
"""Install one macOS app env into its folder the way a computer's build installs a Linux one, until AgentEnv core
composes macOS computers: fetch the declared downloads (each checked against its sha256), run the declared scripts, then
run the app's install steps, in the order its class defines them.

    install_app.py <app folder> <Class> [--sudo]

Run it with the app's own venv (it has the protocol's ``app`` extra). ``--sudo`` runs the system-scope scripts and steps
as root through ``sudo -n``, as a build does; the session ones always run as the invoking user."""

from __future__ import annotations

import argparse
import hashlib
import os
import subprocess
import sys
import urllib.request
from pathlib import Path

from agentenv_protocol.computer._declare import declare


def fetch(url: str) -> bytes:
    with urllib.request.urlopen(url, timeout=600) as reply:
        return reply.read()


def install(
    folder: Path, cls: str, *, sudo: bool = False, get=fetch, run=subprocess.run
) -> list[str]:
    """Install the app; returns what it did, one line each."""
    decl = declare(folder, "app.py", cls)
    spec = decl.os.get("macos")
    if spec is None:
        raise SystemExit(f"{cls} declares no macOS install")
    done: list[str] = []
    for d in spec.install.get("downloads", []):
        data = get(d["url"])
        digest = hashlib.sha256(data).hexdigest()
        if digest != d["sha256"]:
            raise SystemExit(f"{d['url']}: sha256 {digest}, not the pinned {d['sha256']}")
        target = folder / d["to"]
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)
        done.append(f"download {d['to']}")
    for script in spec.install.get("run", []):
        run(
            _as(script.get("scope", "system"), sudo, ["sh", script["script"]]),
            cwd=folder,
            check=True,
        )
        done.append(f"run {script['script']}")
    for step in decl.install_steps:
        if step.os is not None and "macos" not in step.os:
            continue
        argv = [
            sys.executable,
            "-m",
            "agentenv_protocol.computer.run_install_step",
            f"app.py:{cls}",
            step.name,
        ]
        run(_as(step.scope, sudo, argv), cwd=folder, check=True)
        done.append(f"step {step.name}")
    return done


def _as(scope: str, sudo: bool, argv: list[str]) -> list[str]:
    return ["sudo", "-n", "-E", *argv] if sudo and scope == "system" else argv


def main() -> None:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("folder", type=Path)
    parser.add_argument("cls")
    parser.add_argument("--sudo", action="store_true")
    args = parser.parse_args()
    os.chdir(args.folder)
    for line in install(args.folder.resolve(), args.cls, sudo=args.sudo):
        print(f"[install_app] {line}")


if __name__ == "__main__":
    main()
