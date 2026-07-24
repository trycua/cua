from __future__ import annotations

import argparse
import ast
import os
import filecmp
import shutil
import subprocess
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SOURCE = ROOT / "docs/sphinx/cua-sandbox"
PACKAGE_INIT = ROOT / "libs/python/cua-sandbox/cua_sandbox/__init__.py"
PUBLIC_EXPORTS = "public-exports.rst"
REQUIREMENTS = ROOT / "scripts/docs-generators/requirements-sphinx.txt"
ARTIFACT = ROOT / "docs/public/reference/cua-sandbox-sphinx"
MAX_ARTIFACT_BYTES = 300_000
MAX_HTML_LINES = 3_000


def public_exports() -> tuple[str, ...]:
    tree = ast.parse(PACKAGE_INIT.read_text(), filename=str(PACKAGE_INIT))
    for node in tree.body:
        if not isinstance(node, ast.Assign) or not any(
            isinstance(target, ast.Name) and target.id == "__all__" for target in node.targets
        ):
            continue
        value = ast.literal_eval(node.value)
        if not isinstance(value, list) or not all(isinstance(name, str) for name in value):
            break
        return tuple(value)
    raise RuntimeError(f"{PACKAGE_INIT} must assign __all__ to a literal list of strings")


def render_public_exports(exports: tuple[str, ...]) -> str:
    names = "\n".join(f"   cua_sandbox.{name}" for name in exports)
    return f".. autosummary::\n   :nosignatures:\n\n{names}\n"


def build(output: Path) -> None:
    source = output.parent / "source"
    shutil.copytree(SOURCE, source)
    (source / PUBLIC_EXPORTS).write_text(render_public_exports(public_exports()))
    doctrees = output / ".doctrees"
    command = [
        "uv",
        "run",
        "--no-project",
        "--with-requirements",
        str(REQUIREMENTS),
        "python3",
        "-m",
        "sphinx",
        "-W",
        "--keep-going",
        "-b",
        "singlehtml",
        "-d",
        str(doctrees),
        str(source),
        str(output),
    ]
    subprocess.run(
        command,
        cwd=ROOT,
        check=True,
        env={**os.environ, "CUA_SPHINX_ROOT": str(ROOT)},
    )
    shutil.rmtree(doctrees)
    normalize_text_files(output)


def normalize_text_files(directory: Path) -> None:
    for path in directory.rglob("*"):
        if path.suffix not in {".css", ".html", ".js"}:
            continue
        lines = [line.rstrip() for line in path.read_text().splitlines()]
        path.write_text("\n".join(lines).rstrip() + "\n")


def artifact_files(directory: Path) -> list[Path]:
    return sorted(path for path in directory.rglob("*") if path.is_file())


def validate(directory: Path) -> None:
    index = directory / "index.html"
    if not index.is_file():
        raise RuntimeError("Sphinx did not create index.html")
    size = sum(path.stat().st_size for path in artifact_files(directory))
    if size > MAX_ARTIFACT_BYTES:
        raise RuntimeError(f"artifact is {size} bytes; limit is {MAX_ARTIFACT_BYTES}")
    lines = len(index.read_text().splitlines())
    if lines > MAX_HTML_LINES:
        raise RuntimeError(f"artifact has {lines} lines; limit is {MAX_HTML_LINES}")


def matches(generated: Path, committed: Path) -> bool:
    generated_files = [path.relative_to(generated) for path in artifact_files(generated)]
    committed_files = (
        [path.relative_to(committed) for path in artifact_files(committed)]
        if committed.exists()
        else []
    )
    if generated_files != committed_files:
        return False
    return all(
        filecmp.cmp(generated / path, committed / path, shallow=False) for path in generated_files
    )


def sync(generated: Path) -> None:
    if ARTIFACT.exists():
        shutil.rmtree(ARTIFACT)
    shutil.copytree(generated, ARTIFACT)


def main() -> int:
    parser = argparse.ArgumentParser(description="Build the bounded CUA Sandbox Sphinx artifact")
    parser.add_argument(
        "--check", action="store_true", help="fail when the committed artifact differs"
    )
    args = parser.parse_args()

    with tempfile.TemporaryDirectory(prefix="cua-sphinx-") as tempdir:
        generated = Path(tempdir) / "html"
        build(generated)
        validate(generated)
        if args.check:
            if not matches(generated, ARTIFACT):
                raise RuntimeError("Sphinx artifact is stale; run sphinx_cua_sandbox.py")
        else:
            sync(generated)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
