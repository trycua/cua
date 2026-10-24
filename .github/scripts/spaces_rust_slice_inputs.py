#!/usr/bin/env python3
"""What the SwiftUI app's compiled Rust slice is a function of.

`.github/actions/spaces-rust-slice` keys the compiled library (libcua_spaces_ffi
and the cua-spaces-cli binary) by a hash of its inputs. A restored library is
only correct if that hash covers every file the compiler and the build scripts
read. `libs/cua` is hashed whole, so the question is which files *outside* it
(or inside it, but left out of the hash on purpose) the build reads. This
script answers it from the code instead of from a list someone has to keep:

  closure   the workspace crates `cua-spaces-ffi` and `cua-spaces-cli` depend on
            (normal and build dependencies, read from the Cargo.toml files;
            dev-dependencies are not built)
  reads     in those crates' `src/` and `build.rs`: `include_str!`,
            `include_bytes!`, `include!` and `include_dir!` with a literal or
            `concat!(env!("CARGO_MANIFEST_DIR"), ...)` path, `#[path = "..."]`,
            and every `"../..."` literal in a build script (they copy files
            from elsewhere in the repository into OUT_DIR). Code under
            `#[cfg(test)]` and `#[test]` is not part of the library and is
            skipped. Anything it cannot classify counts as a read.
  key       sha256 of the `git ls-files -s` listing of libs/cua (minus the
            language packages and Markdown, which the build does not read)
            plus the reads that fall outside that, plus the cargo configuration
            files Cargo merges into a build under libs/cua.

Usage:
  spaces_rust_slice_inputs.py key              print the hash (the action's key)
  spaces_rust_slice_inputs.py inputs           print the extra input paths
  spaces_rust_slice_inputs.py closure          print the crates in the closure
  spaces_rust_slice_inputs.py verify DIR       DIR is a finished build's target
                                               directory (target/<triple>): fail
                                               if the compiler or a build script
                                               read a tracked file the key misses

`verify` is the ground truth: it reads the dep-info files rustc wrote and the
`rerun-if-changed` lines of the build scripts, so a read this scanner cannot
see (a new macro, a proc macro that opens a file) still fails the producer.
"""

from __future__ import annotations

import hashlib
import os
import re
import subprocess
import sys
from pathlib import Path

try:  # Python 3.11+
    import tomllib as _toml

    def _load_toml(path: Path) -> dict:
        with open(path, "rb") as handle:
            return _toml.load(handle)

except ModuleNotFoundError:  # pragma: no cover - depends on the interpreter
    try:
        import toml as _toml_pkg

        def _load_toml(path: Path) -> dict:
            return _toml_pkg.load(str(path))

    except ModuleNotFoundError:
        _load_toml = None  # type: ignore[assignment]

ROOTS = ("cua-spaces-ffi", "cua-spaces-cli")
WORKSPACE = "libs/cua"
# What the hash leaves out of libs/cua: the Kotlin, Python and TypeScript
# packages and Markdown (see `_covered_by_base`). Anything the build reads from
# there is added back through `extra_inputs` (libs/cua/skills is, by the
# cua-agent-setup build script, which embeds its Markdown).
HASHED_OUT = ("kotlin", "python", "typescript")
# Cargo reads `.cargo/config.toml` from the working directory (libs/cua) up to
# the root: rustflags and [env] of the build. The one inside libs/cua is
# already hashed with the tree.
CARGO_CONFIG_DIRS = (".cargo", "libs/.cargo")
KEY_VERSION = "v1"


# ----------------------------------------------------------------------------
# Repository helpers


def repo_root() -> Path:
    out = subprocess.run(
        ["git", "rev-parse", "--show-toplevel"], capture_output=True, text=True, check=True
    )
    return Path(out.stdout.strip()).resolve()


def _rel(root: Path, path: Path) -> str:
    return os.path.relpath(os.path.normpath(path), root).replace(os.sep, "/")


# ----------------------------------------------------------------------------
# The workspace closure


def _workspace_members(root: Path) -> dict[str, Path]:
    """Crate name -> directory, for every Cargo.toml under libs/cua."""
    found: dict[str, Path] = {}
    base = root / WORKSPACE
    for dirpath, dirnames, filenames in os.walk(base):
        dirnames[:] = [d for d in dirnames if d not in ("target", "node_modules", ".git")]
        if "Cargo.toml" in filenames and Path(dirpath) != base:
            manifest = Path(dirpath) / "Cargo.toml"
            if _load_toml is None:
                match = re.search(r'(?m)^name\s*=\s*"([^"]+)"', manifest.read_text())
                name = match.group(1) if match else Path(dirpath).name
            else:
                name = _load_toml(manifest).get("package", {}).get("name", Path(dirpath).name)
            found[name] = Path(dirpath)
    return found


def _dependency_names(manifest: dict) -> list[tuple[str, dict]]:
    """(key, spec) of every normal and build dependency, target tables included."""
    tables = []
    for kind in ("dependencies", "build-dependencies"):
        tables.append(manifest.get(kind, {}))
        for target in manifest.get("target", {}).values():
            tables.append(target.get(kind, {}))
    out = []
    for table in tables:
        for key, spec in table.items():
            out.append((key, spec if isinstance(spec, dict) else {}))
    return out


def closure(root: Path) -> dict[str, Path]:
    """The workspace crates the slice's two packages build (name -> directory).

    Without a TOML parser (an old interpreter) every crate counts: a larger
    closure only makes the key stricter."""
    members = _workspace_members(root)
    if _load_toml is None:
        return members
    workspace = _load_toml(root / WORKSPACE / "Cargo.toml")
    shared = workspace.get("workspace", {}).get("dependencies", {})
    by_dir = {path.resolve(): name for name, path in members.items()}
    seen: dict[str, Path] = {}
    stack = [name for name in ROOTS if name in members]
    while stack:
        name = stack.pop()
        if name in seen:
            continue
        crate_dir = members[name]
        seen[name] = crate_dir
        manifest = _load_toml(crate_dir / "Cargo.toml")
        for key, spec in _dependency_names(manifest):
            real = spec.get("package", key)
            path = spec.get("path")
            if spec.get("workspace"):
                shared_spec = shared.get(key, {})
                shared_spec = shared_spec if isinstance(shared_spec, dict) else {}
                real = shared_spec.get("package", real)
                path = shared_spec.get("path")
                base = root / WORKSPACE
            else:
                base = crate_dir
            if path:
                target = (base / path).resolve()
                real = by_dir.get(target, real)
            if real in members:
                stack.append(real)
    return seen


# ----------------------------------------------------------------------------
# Rust source scanning


def _mask(src: str) -> str:
    """`src` with comments and the inside of string and char literals blanked
    (same length, newlines kept), so braces and attributes can be found safely."""
    out = list(src)
    n = len(src)
    i = 0

    def blank(a: int, b: int) -> None:
        for k in range(a, b):
            if out[k] != "\n":
                out[k] = " "

    while i < n:
        two = src[i : i + 2]
        c = src[i]
        if two == "//":
            j = src.find("\n", i)
            j = n if j < 0 else j
            blank(i, j)
            i = j
        elif two == "/*":
            depth, j = 1, i + 2
            while j < n and depth:
                if src.startswith("/*", j):
                    depth += 1
                    j += 2
                elif src.startswith("*/", j):
                    depth -= 1
                    j += 2
                else:
                    j += 1
            blank(i, j)
            i = j
        elif c in "rb" and (i == 0 or not (src[i - 1].isalnum() or src[i - 1] == "_")) and re.match(
            r'(?:br|r)(#*)"', src[i : i + 40]
        ):
            m = re.match(r'(?:br|r)(#*)"', src[i : i + 40])
            assert m
            end = '"' + m.group(1)
            start = i + m.end()
            j = src.find(end, start)
            j = n if j < 0 else j + len(end)
            blank(start, j - len(end))
            i = j
        elif c == '"':
            j = i + 1
            while j < n and src[j] != '"':
                j += 2 if src[j] == "\\" else 1
            blank(i + 1, j)
            i = j + 1
        elif c == "'":
            m = re.match(r"'(?:\\(?:u\{[0-9a-fA-F_]+\}|x[0-9a-fA-F]{2}|.)|[^\\'\n])'", src[i : i + 14])
            if m:
                blank(i + 1, i + m.end() - 1)
                i += m.end()
            else:
                i += 1  # a lifetime
        else:
            i += 1
    return "".join(out)


_TEST_ATTR = re.compile(r"#\s*\[\s*(?:cfg\s*\(\s*test\s*\)|test|tokio\s*::\s*test\b[^\]]*)\s*\]")
_INNER_TEST = re.compile(r"#\s*!\s*\[\s*cfg\s*\(\s*test\s*\)\s*\]")


def test_spans(src: str, masked: str | None = None) -> list[tuple[int, int]]:
    """Character ranges of items that only exist under `cfg(test)`."""
    masked = _mask(src) if masked is None else masked
    if _INNER_TEST.search(masked):
        return [(0, len(src))]
    spans = []
    n = len(masked)
    for m in _TEST_ATTR.finditer(masked):
        i = m.end()
        # More attributes on the same item.
        while True:
            while i < n and masked[i].isspace():
                i += 1
            if masked.startswith("#", i):
                j = masked.find("[", i)
                depth = 0
                while j < n:
                    depth += masked[j] == "["
                    depth -= masked[j] == "]"
                    j += 1
                    if depth == 0:
                        break
                i = j
            else:
                break
        paren = 0
        j = i
        end = None
        while j < n:
            ch = masked[j]
            if ch in "([":
                paren += 1
            elif ch in ")]":
                paren -= 1
            elif ch == ";" and paren == 0:
                end = j + 1
                break
            elif ch == "{" and paren == 0:
                depth = 0
                while j < n:
                    depth += masked[j] == "{"
                    depth -= masked[j] == "}"
                    j += 1
                    if depth == 0:
                        break
                end = j
                break
            j += 1
        if end is not None:
            spans.append((m.start(), end))
    return spans


_MAY_READ = re.compile(r"\binclude(?:_str|_bytes|_dir)?!|#\s*\[\s*path\b")
_INCLUDE_CALL = re.compile(r"\b(?:include_str|include_bytes|include|include_dir)!\s*\(")
_INCLUDE_ARG = re.compile(
    r"\s*(concat!\s*\([^()]*(?:\([^()]*\)[^()]*)*\)|\"[^\"]*\")\s*\)",
    re.S,
)
_PATH_ATTR = re.compile(r"#\s*\[\s*path\s*=\s*(?=\")")


def _string_literals(src: str, masked: str) -> list[tuple[int, str]]:
    """(offset of the opening quote, text) of every string literal. The mask
    keeps the quote characters, so they pair up in order."""
    quotes = [i for i, ch in enumerate(masked) if ch == '"']
    return [(a, src[a + 1 : b]) for a, b in zip(quotes[::2], quotes[1::2])]


def _in_spans(pos: int, spans: list[tuple[int, int]]) -> bool:
    return any(a <= pos < b for a, b in spans)


def _resolve_concat(arg: str, crate_dir: Path) -> str | None:
    parts = []
    for literal, env in re.findall(r'"([^"]*)"|(env!\s*\(\s*"[A-Z_]+"\s*\))', arg):
        if env:
            if "CARGO_MANIFEST_DIR" not in env:
                return None  # OUT_DIR and the like: not a source file
            parts.append(str(crate_dir))
        else:
            parts.append(literal)
    return "".join(parts)


def source_reads(crate_dir: Path) -> set[Path]:
    """Every file or directory the crate's non-test code reads at build time."""
    reads: set[Path] = set()
    files = []
    for dirpath, dirnames, filenames in os.walk(crate_dir / "src"):
        dirnames[:] = [d for d in dirnames if d != "target"]
        files += [Path(dirpath) / f for f in filenames if f.endswith(".rs")]
    build_rs = crate_dir / "build.rs"
    if build_rs.exists():
        files.append(build_rs)
    for path in files:
        src = path.read_text(errors="replace")
        # The lexer is slow in Python: only files that can read something.
        if path != build_rs and not _MAY_READ.search(src):
            continue
        masked = _mask(src)
        spans = test_spans(src, masked)
        for call in _INCLUDE_CALL.finditer(masked):
            if _in_spans(call.start(), spans):
                continue
            arg = _INCLUDE_ARG.match(src, call.end())
            if not arg:
                continue
            text = arg.group(1)
            if text.startswith("concat"):
                resolved = _resolve_concat(text, crate_dir)
                if resolved is None:
                    continue
                reads.add(Path(os.path.normpath(resolved)))
            else:
                reads.add(Path(os.path.normpath(path.parent / text.strip('"'))))
        for attr in _PATH_ATTR.finditer(masked):
            if _in_spans(attr.start(), spans):
                continue
            literal = next((t for a, t in _string_literals(src, masked) if a == attr.end()), None)
            if literal:
                reads.add(Path(os.path.normpath(path.parent / literal)))
        if path == build_rs:
            # A build script runs with the crate directory as its working
            # directory and joins "../..." literals onto CARGO_MANIFEST_DIR.
            for offset, literal in _string_literals(src, masked):
                if literal.startswith("../") and not _in_spans(offset, spans):
                    reads.add(Path(os.path.normpath(crate_dir / literal)))
    return reads


# ----------------------------------------------------------------------------
# The key


def _git_ls_files(root: Path, specs: list[str], stage: bool = True) -> list[str]:
    out = subprocess.run(
        ["git", "-c", "core.quotepath=off", "ls-files", *(["-s"] if stage else []), "-z", "--", *specs],
        cwd=root,
        capture_output=True,
        text=True,
        check=True,
    )
    return [e for e in out.stdout.split("\0") if e]


def _is_git_internal(rel: str) -> bool:
    return rel == ".git" or rel.startswith(".git/") or "/.git/" in rel


def _covered_by_base(rel: str) -> bool:
    """Is a tracked path inside the hashed part of libs/cua?

    Decided here, not with `:(exclude)` pathspecs: git's `:(exclude,glob)**/*.md`
    did not exclude libs/cua's Markdown, so the earlier key rebuilt on every
    README edit."""
    if not rel.startswith(WORKSPACE + "/"):
        return False
    for excluded in HASHED_OUT:
        if rel == f"{WORKSPACE}/{excluded}" or rel.startswith(f"{WORKSPACE}/{excluded}/"):
            return False
    return not rel.endswith(".md")


def extra_inputs(root: Path) -> list[str]:
    """Repository paths the build reads that the hashed part of libs/cua lacks.

    Files and directories, sorted, relative to the repository root. A directory
    is listed once; `git ls-files` expands it."""
    extra: set[str] = set()
    for crate_dir in closure(root).values():
        for read in source_reads(crate_dir):
            try:
                rel = _rel(root, read)
            except ValueError:
                continue
            if rel.startswith("../") or _is_git_internal(rel):
                continue
            if (root / rel).is_dir():
                files = _git_ls_files(root, [f":(literal){rel}"], stage=False)
                if files and all(_covered_by_base(f) for f in files):
                    continue
            elif _covered_by_base(rel):
                continue
            extra.add(rel)
    for config_dir in CARGO_CONFIG_DIRS:
        if (root / config_dir).is_dir():
            extra.add(config_dir)
    return sorted(extra)


def listing(root: Path) -> list[str]:
    """The sorted, de-duplicated `git ls-files -s` lines the key hashes."""
    lines = {
        line
        for line in _git_ls_files(root, [WORKSPACE])
        if _covered_by_base(line.split("\t", 1)[1])
    }
    extra = extra_inputs(root)
    if extra:
        lines |= set(_git_ls_files(root, [f":(literal){p}" for p in extra]))
    return sorted(lines, key=lambda line: line.split("\t", 1)[1])


def hash_listing(lines: list[str]) -> str:
    payload = "\n".join(sorted(set(lines), key=lambda line: line.split("\t", 1)[1])) + "\n"
    return hashlib.sha256((KEY_VERSION + "\n" + payload).encode()).hexdigest()


def key(root: Path) -> str:
    return hash_listing(listing(root))


# ----------------------------------------------------------------------------
# Ground truth from a finished build


def _depinfo_paths(text: str) -> set[str]:
    paths: set[str] = set()
    for line in text.splitlines():
        line = line.rstrip("\\ ").strip()
        if not line:
            continue
        # `target/.../name.d` lists `output: input input ...`; later lines are
        # `input:` entries or empty rules.
        if ": " in line and not line.startswith(" "):
            line = line.split(": ", 1)[1]
        line = line.rstrip(":")
        token = ""
        escaped = False
        for ch in line:
            if escaped:
                token += ch
                escaped = False
            elif ch == "\\":
                escaped = True
            elif ch == " ":
                if token:
                    paths.add(token)
                token = ""
            else:
                token += ch
        if token:
            paths.add(token)
    return {p.rstrip(":") for p in paths}


def build_reads(root: Path, target_dir: Path) -> set[str]:
    """Repository-relative files a finished build read (rustc dep-info and build
    script `rerun-if-changed` lines), outside target/ and the registry."""
    found: set[str] = set()
    for d_file in (target_dir / "deps").glob("*.d"):
        for p in _depinfo_paths(d_file.read_text(errors="replace")):
            found.add(p)
    members = _workspace_members(root)
    for output in (target_dir / "build").glob("*/output"):
        crate_name = output.parent.name.rsplit("-", 1)[0]
        crate_dir = members.get(crate_name)
        for line in output.read_text(errors="replace").splitlines():
            m = re.match(r"cargo:rerun-if-changed=(.*)", line)
            if not m:
                continue
            p = m.group(1)
            if not os.path.isabs(p) and crate_dir is not None:
                p = str(crate_dir / p)
            found.add(p)
    repo: set[str] = set()
    for p in found:
        path = Path(p)
        if not path.is_absolute():
            path = root / WORKSPACE / path  # rustc runs from the workspace root
        try:
            rel = _rel(root, Path(os.path.normpath(path)))
        except ValueError:
            continue
        if rel.startswith("../") or "/target/" in f"/{rel}" or _is_git_internal(rel):
            continue
        repo.add(rel)
    return repo


def verify(root: Path, target_dir: Path) -> list[str]:
    """Tracked files the build read that the key does not hash."""
    hashed = {line.split("\t", 1)[1] for line in listing(root)}
    tracked = set(
        subprocess.run(
            ["git", "-c", "core.quotepath=off", "ls-files", "-z"],
            cwd=root,
            capture_output=True,
            text=True,
            check=True,
        ).stdout.split("\0")
    )
    missing: set[str] = set()
    for rel in sorted(build_reads(root, target_dir)):
        if (root / rel).is_dir():
            # `rerun-if-changed` on a directory: every file in it is read.
            candidates = [t for t in tracked if t.startswith(rel + "/")]
        else:
            candidates = [rel] if rel in tracked else []
        missing.update(c for c in candidates if c not in hashed)
    return sorted(missing)


def main(argv: list[str]) -> int:
    if len(argv) < 2 or argv[1] not in {"key", "inputs", "closure", "verify"}:
        print(__doc__)
        return 2
    root = repo_root()
    command = argv[1]
    if command == "key":
        print(key(root))
    elif command == "inputs":
        print("\n".join(extra_inputs(root)))
    elif command == "closure":
        print("\n".join(sorted(closure(root))))
    else:
        if len(argv) != 3:
            print("usage: verify <target dir>", file=sys.stderr)
            return 2
        missing = verify(root, Path(argv[2]).resolve())
        if missing:
            for rel in missing:
                print(f"::error::the build read {rel}, which the slice cache key does not cover")
            print(
                "Add it to extra_inputs() in .github/scripts/spaces_rust_slice_inputs.py "
                "(or fix the scanner) and to the paths of ci-cua-spaces-rust-slice.yml.",
                file=sys.stderr,
            )
            return 1
        print("The slice cache key covers every tracked file the build read.")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
