#!/usr/bin/env python3
"""Lints the docs' cua-sandbox YAML examples with actionlint.

Each ```yaml block that uses the action (in the pages below) is wrapped into a
complete workflow: a block with `jobs:` gets `on:`; a steps fragment gets a
job around it. The local `uses: trycua/cua/.github/actions/cua-sandbox@...` is
rewritten to `./.github/actions/cua-sandbox`, so actionlint also checks the
inputs against action.yml.

    lint-docs-yaml.py [ACTIONLINT]      # default: actionlint on PATH
"""

from __future__ import annotations

import re
import subprocess
import sys
import tempfile
import textwrap
from pathlib import Path

REPO = Path(__file__).resolve().parents[3]
PAGES = [
    "docs/content/docs/cua-sdk/guides/test-in-ci.mdx",
    "docs/content/docs/cua-driver/guides/test-in-a-sandbox.mdx",
    "docs/content/docs/start-here/run-in-github-actions.mdx",
]
FENCE = re.compile(r"^```yaml[^\n]*\n(.*?)^```", re.M | re.S)
REMOTE = re.compile(r"trycua/cua/\.github/actions/cua-sandbox@\S+")


def workflows() -> list[tuple[str, str]]:
    out = []
    for page in PAGES:
        for n, m in enumerate(FENCE.finditer((REPO / page).read_text())):
            code = m.group(1)
            if "cua-sandbox" not in code:
                continue
            code = REMOTE.sub("./.github/actions/cua-sandbox", code)
            if code.lstrip().startswith("jobs:"):
                body = code
            else:
                steps = textwrap.indent(textwrap.dedent(code), " " * 6)
                body = (
                    "jobs:\n  test:\n    runs-on: ubuntu-latest\n    steps:\n"
                    "      - uses: actions/checkout@v4\n" + steps
                )
            out.append((f"{Path(page).stem}-{n}", "name: docs\non: push\n" + body))
    return out


def main() -> int:
    actionlint = sys.argv[1] if len(sys.argv) > 1 else "actionlint"
    found = workflows()
    if not found:
        print("no cua-sandbox YAML examples found", file=sys.stderr)
        return 1
    with tempfile.TemporaryDirectory(dir=REPO / ".github" / "workflows") as tmp:
        paths = []
        for name, text in found:
            path = Path(tmp) / f"{name}.yml"
            path.write_text(text)
            paths.append(str(path))
        rc = subprocess.run([actionlint, "-shellcheck=", "-pyflakes=", *paths], cwd=REPO).returncode
    print(f"actionlint on {len(found)} docs example(s): {'ok' if rc == 0 else 'FAILED'}")
    return rc


if __name__ == "__main__":
    sys.exit(main())
