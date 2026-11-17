from pathlib import Path

from sync_sandbox_release_docs import main


def test_only_cua_sandbox_lines_change(tmp_path: Path) -> None:
    docs = tmp_path / "docs/content/docs/cua-sdk"
    (docs / "guides").mkdir(parents=True)
    (tmp_path / "libs/python/cua-sandbox").mkdir(parents=True)
    (tmp_path / "libs/python/cua-sandbox/VERSION").write_text("0.10.0\n")
    (docs / "page.mdx").write_text("`cua-sandbox` 0.9.1 here\n`cua-agent` 0.9.1 there\ncua-sandbox==0.9.10\n")
    (docs / "guides/cua-cloud-ended.mdx").write_text("`cua-sandbox` 0.9.1\n")

    assert main(["--repo-root", str(tmp_path), "--from", "0.9.1"]) == 0

    assert (docs / "page.mdx").read_text() == "`cua-sandbox` 0.10.0 here\n`cua-agent` 0.9.1 there\ncua-sandbox==0.9.10\n"
    assert (docs / "guides/cua-cloud-ended.mdx").read_text() == "`cua-sandbox` 0.9.1\n"
