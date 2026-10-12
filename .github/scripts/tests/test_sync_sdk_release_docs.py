from pathlib import Path

from sync_sdk_release_docs import main


def test_sync_sdk_release_docs_updates_standalone_versions(tmp_path: Path):
    version = tmp_path / "libs/cua/VERSION"
    version.parent.mkdir(parents=True)
    version.write_text("0.5.0\n")
    content = (
        '  Version: cua 0.4.1, cua-sandbox 0.9.0\n'
        'cua-sdk-v0.4.1\n@trycua/cua@0.4.1\n"0.4.1"\n'
        'uses the `cua` 0.4.1 CLI.\nSentence ends 0.4.1.\n'
        '10.4.1 0.4.12 0.4.1.2 1.0.4.1\n'
    )
    paths = [
        tmp_path / f"docs/content/docs/{folder}/nested/page.mdx"
        for folder in ("cua-cli", "cua-sdk", "fleets")
    ]
    paths.append(tmp_path / "scripts/docs-generators/cli-specs/cua.json")
    outside = tmp_path / "docs/content/docs/other/page.mdx"
    for path in [*paths, outside]:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)

    assert main(["--repo-root", str(tmp_path), "--from", "0.4.1"]) == 0

    expected = content.replace("0.4.1", "0.5.0").replace(
        "10.5.0 0.5.02 0.5.0.2 1.0.5.0", "10.4.1 0.4.12 0.4.1.2 1.0.4.1"
    )
    for path in paths:
        assert path.read_text() == expected
    assert outside.read_text() == content
