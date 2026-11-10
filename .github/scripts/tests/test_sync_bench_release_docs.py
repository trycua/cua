from pathlib import Path
import shutil

from sync_bench_release_docs import bench_reference_paths, sync_bench_release_docs


REPO_ROOT = Path(__file__).resolve().parents[3]
DOCS = "docs/content/docs/cua-bench/reference"


def test_sync_bench_release_docs_updates_every_generated_marker(tmp_path: Path):
    pyproject = tmp_path / "libs/cua-bench/pyproject.toml"
    pyproject.parent.mkdir(parents=True)
    pyproject.write_text('[project]\nname = "cua-bench"\nversion = "9.9.9"\n')

    config = "scripts/docs-generators/config.json"
    (tmp_path / config).parent.mkdir(parents=True)
    shutil.copy(REPO_ROOT / config, tmp_path / config)
    shutil.copytree(REPO_ROOT / DOCS, tmp_path / DOCS)
    spec = "scripts/docs-generators/cli-specs/cb.json"
    (tmp_path / spec).parent.mkdir(parents=True)
    shutil.copy(REPO_ROOT / spec, tmp_path / spec)

    sync_bench_release_docs(tmp_path)

    paths = bench_reference_paths(tmp_path)
    assert f"{DOCS}/cli.mdx" in paths
    for relative in paths:
        assert "Version: 9.9.9" in (tmp_path / relative).read_text()
    # The generator's CLI spec records the documented version; a release
    # that leaves it behind fails the cua-bench reference check.
    assert '\n "version": "9.9.9"\n' in (tmp_path / spec).read_text()
