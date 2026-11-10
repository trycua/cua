import argparse
from pathlib import Path
import re

CONTRACT_LINE = "Contract version "


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Synchronize SDK release documentation.")
    parser.add_argument("--repo-root", dest="root", type=Path, default=Path.cwd())
    parser.add_argument("--from", dest="previous", required=True)
    args = parser.parse_args(argv)
    version = (args.root / "libs/cua/VERSION").read_text().strip()
    paths = [path for folder in ("cua-cli", "cua-sdk")
             for path in (args.root / "docs/content/docs" / folder).rglob("*.mdx")]
    for path in [*paths, args.root / "scripts/docs-generators/cli-specs/cua.json"]:
        content = path.read_text()
        # The MCP contract has its own version (CONTRACT_VERSION in
        # cua-spaces-contract), which the generated reference prints on a
        # "Contract version" line; it does not follow the SDK release.
        updated = "".join(
            line if line.startswith(CONTRACT_LINE) else
            re.sub(r"(?<![\d.])" + re.escape(args.previous) + r"(?!\.?\d)", version, line)
            for line in content.splitlines(keepends=True)
        )
        path.write_text(updated)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
