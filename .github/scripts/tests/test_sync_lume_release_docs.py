from pathlib import Path
import shutil

from sync_lume_release_docs import lume_reference_paths, sync_lume_release_docs


REPO_ROOT = Path(__file__).resolve().parents[3]
DOCS = "docs/content/docs/lume/reference"


def test_sync_lume_release_docs_updates_every_generated_marker(tmp_path: Path):
    version_path = tmp_path / "libs/lume/VERSION"
    version_path.parent.mkdir(parents=True)
    version_path.write_text("9.9.9\n")

    config = "scripts/docs-generators/config.json"
    (tmp_path / config).parent.mkdir(parents=True)
    shutil.copy(REPO_ROOT / config, tmp_path / config)
    shutil.copytree(REPO_ROOT / DOCS, tmp_path / DOCS)

    sync_lume_release_docs(tmp_path)

    paths = lume_reference_paths(tmp_path)
    assert f"{DOCS}/http-api.mdx" in paths
    assert any("/cli/" in relative for relative in paths)
    for relative in paths:
        assert "Version: 9.9.9" in (tmp_path / relative).read_text()
    assert "Documented against Lume **9.9.9**." in (tmp_path / DOCS / "http-api.mdx").read_text()
