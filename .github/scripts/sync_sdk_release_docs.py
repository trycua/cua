import argparse
from pathlib import Path
import re


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Synchronize SDK release documentation.")
    parser.add_argument("--repo-root", dest="root", type=Path, default=Path.cwd())
    parser.add_argument("--from", dest="previous", required=True)
    args = parser.parse_args(argv)
    try:
        version = (args.root / "libs/cua/VERSION").read_text().strip()
        paths = [path for folder in ("cua-cli", "cua-sdk", "fleets")
                 for path in (args.root / "docs/content/docs" / folder).rglob("*.mdx")]
        for path in [*paths, args.root / "scripts/docs-generators/cli-specs/cua.json"]:
            content = path.read_text()
            updated = re.sub(r"(?<![\d.])" + re.escape(args.previous) + r"(?!\.?\d)", version, content)
            if updated != content:
                path.write_text(updated)
    except OSError as error:
        print(f"SDK release docs error: {error}")
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
