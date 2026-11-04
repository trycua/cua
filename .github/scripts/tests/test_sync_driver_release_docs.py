from pathlib import Path
import shutil

from sync_driver_release_docs import driver_reference_paths, sync_driver_release_docs


REPO_ROOT = Path(__file__).resolve().parents[3]
DOCS = "docs/content/docs/cua-driver/reference"


def test_sync_driver_release_docs_updates_generated_markers(tmp_path: Path):
    version_path = tmp_path / "libs/cua-driver/rust/VERSION"
    version_path.parent.mkdir(parents=True)
    version_path.write_text("9.9.9\n")

    config = "scripts/docs-generators/config.json"
    (tmp_path / config).parent.mkdir(parents=True)
    shutil.copy(REPO_ROOT / config, tmp_path / config)
    shutil.copytree(REPO_ROOT / DOCS, tmp_path / DOCS)
    spec = "scripts/docs-generators/cli-specs/cua-driver.json"
    (tmp_path / spec).parent.mkdir(parents=True)
    shutil.copy(REPO_ROOT / spec, tmp_path / spec)

    docs = tmp_path / DOCS
    notes = docs / "mcp-tools" / "notes.mdx"
    notes.write_text("Shared guidance without a release marker.\n")
    sync_driver_release_docs(tmp_path)

    paths = driver_reference_paths(tmp_path)
    # One page per command group and per tool group, nested folders included.
    assert any("/cli/" in relative for relative in paths)
    assert any("/mcp-tools/" in relative for relative in paths)
    for relative in paths:
        assert "Version: 9.9.9" in (tmp_path / relative).read_text()
    assert notes.read_text() == "Shared guidance without a release marker.\n"
    assert '\n "version": "9.9.9"\n' in (tmp_path / spec).read_text()
