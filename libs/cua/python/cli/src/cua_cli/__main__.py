"""`python -m cua_cli fetch` downloads and verifies the cua CLI for this
machine ahead of time; `python -m cua_cli path` prints the binary `cua` runs."""

from __future__ import annotations

import sys

from . import FetchError, binary, fetch, release


def main(argv: list[str] | None = None) -> int:
    args = sys.argv[1:] if argv is None else argv
    cmd = args[0] if args else "path"
    try:
        if cmd == "fetch":
            exe = fetch() if release() is not None else binary()
        elif cmd == "path":
            exe = binary()
        else:
            sys.stderr.write("usage: python -m cua_cli [fetch|path]\n")
            return 2
    except FetchError as e:
        sys.stderr.write(f"cua-cli: {e}\n")
        return 1
    print(exe)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
