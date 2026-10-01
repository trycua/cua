from pathlib import Path
import json
import shutil

from sync_driver_release_docs import driver_reference_paths, sync_driver_release_docs


REPO_ROOT = Path(__file__).resolve().parents[3]


def test_sync_driver_release_docs_updates_generated_markers(tmp_path: Path):
    version_path = tmp_path / "libs/cua-driver/rust/VERSION"
    version_path.parent.mkdir(parents=True)
    version_path.write_text("9.9.9\n")

    config_path = tmp_path / "scripts/docs-generators/config.json"
    config_path.parent.mkdir(parents=True)
    config = json.loads((REPO_ROOT / "scripts/docs-generators/config.json").read_text())
    config["generators"]["cua-driver"]["outputs"].append({
        "type": "mcp", "outputFile": "mcp-tools-test.mdx",
        "platform": {"host": "test", "name": "Test"},
    })
    config_path.write_text(json.dumps(config))

    for relative in driver_reference_paths(REPO_ROOT):
        destination = tmp_path / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy(REPO_ROOT / relative, destination)

    docs = tmp_path / "docs/content/docs/reference/cua-driver"
    (docs / "mcp-tools-test.mdx").write_text("  Version: 0.0.0\n")
    notes = docs / "mcp-tool-notes.mdx"
    notes.write_text("Shared guidance without a release marker.\n")
    sync_driver_release_docs(tmp_path)

    assert "Documented against Cua Driver **9.9.9**." in (docs / "cli-reference.mdx").read_text()
    for relative in driver_reference_paths(tmp_path):
        assert "Version: 9.9.9" in (tmp_path / relative).read_text()
    assert notes.read_text() == "Shared guidance without a release marker.\n"
