from pathlib import Path
import shutil

from sync_lume_release_docs import DOC_PATHS, sync_lume_release_docs


REPO_ROOT = Path(__file__).resolve().parents[3]


def test_sync_lume_release_docs_updates_both_generated_markers(tmp_path: Path):
    version_path = tmp_path / "libs/lume/VERSION"
    version_path.parent.mkdir(parents=True)
    version_path.write_text("9.9.9\n")

    for relative in DOC_PATHS:
        source = REPO_ROOT / relative
        destination = tmp_path / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy(source, destination)

    sync_lume_release_docs(tmp_path)

    for relative in DOC_PATHS:
        content = (tmp_path / relative).read_text()
        assert "Version: 9.9.9" in content
        assert "Documented against Lume **9.9.9**." in content
