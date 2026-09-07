from pathlib import Path
import shutil

from sync_driver_release_docs import DOC_PATHS, sync_driver_release_docs


REPO_ROOT = Path(__file__).resolve().parents[3]


def test_sync_driver_release_docs_updates_generated_markers(tmp_path: Path):
    version_path = tmp_path / "libs/cua-driver/rust/VERSION"
    version_path.parent.mkdir(parents=True)
    version_path.write_text("9.9.9\n")

    for relative in DOC_PATHS:
        source = REPO_ROOT / relative
        destination = tmp_path / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy(source, destination)

    landing = tmp_path / "docs/content/docs/reference/cua-driver/mcp-tools.mdx"
    landing.write_text("Shared guidance without a release marker.\n")
    sync_driver_release_docs(tmp_path)

    cli = (tmp_path / DOC_PATHS[0]).read_text()
    assert "Version: 9.9.9" in cli
    assert "Documented against Cua Driver **9.9.9**." in cli
    for relative in DOC_PATHS[1:]:
        assert "Version: 9.9.9" in (tmp_path / relative).read_text()
    assert landing.read_text() == "Shared guidance without a release marker.\n"
