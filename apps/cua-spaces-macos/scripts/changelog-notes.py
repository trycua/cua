#!/usr/bin/env python3
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.
"""Prints one version's section of a Release Please CHANGELOG.md (the
Sparkle appcast's release notes when the release has none of its own).

    changelog-notes.py 0.2.0 apps/cua-spaces-macos/CHANGELOG.md
"""
import re
import sys

version, path = sys.argv[1:3]
out, on = [], False
for line in open(path, encoding="utf-8"):
    if line.startswith("## "):
        if on:
            break
        on = re.match(r"## \[?" + re.escape(version) + r"(?![\w.-])", line) is not None
        continue
    if on:
        out.append(line)
print("".join(out).strip() or f"Cua Spaces {version}.")
