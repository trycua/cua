"""Offline corpus parity tests, using real SQLite/FTS and synthetic Lance vectors."""

import ast
import asyncio
import json
import sys
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import AsyncMock

import lancedb
import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "docs-mcp-server"))
import docs_index as index


@pytest.fixture(params=["crawl_docs.py", "modal_app.py"])
def load_docs_page(request):
    """Exercise both production helpers without starting Modal or Playwright."""
    path = Path(__file__).resolve().parents[1] / request.param
    function = next(
        node
        for node in ast.parse(path.read_text()).body
        if isinstance(node, ast.AsyncFunctionDef) and node.name == "load_docs_page"
    )
    namespace = {}
    exec(compile(ast.Module(body=[function], type_ignores=[]), str(path), "exec"), namespace)
    return namespace["load_docs_page"]


def docs_page(response):
    return SimpleNamespace(
        goto=AsyncMock(return_value=response),
        wait_for_selector=AsyncMock(),
        content=AsyncMock(return_value="<main><article>Docs body</article></main>"),
    )


def test_page_readiness_does_not_require_network_idle(load_docs_page):
    page = docs_page(SimpleNamespace(ok=True, status=200))

    async def navigate(url, *, wait_until, timeout):
        if wait_until == "networkidle":
            raise TimeoutError("Background requests never become idle")
        return SimpleNamespace(ok=True, status=200)

    page.goto.side_effect = navigate
    assert (
        asyncio.run(load_docs_page(page, "https://example.com/docs")) == page.content.return_value
    )
    page.goto.assert_awaited_once_with(
        "https://example.com/docs", wait_until="domcontentloaded", timeout=30_000
    )
    page.wait_for_selector.assert_awaited_once_with(
        "article, main", state="attached", timeout=30_000
    )


@pytest.mark.parametrize("status", [None, 404, 500])
def test_page_readiness_rejects_missing_or_failed_response(load_docs_page, status):
    page = docs_page(None if status is None else SimpleNamespace(ok=False, status=status))
    with pytest.raises(RuntimeError, match=f"HTTP {status if status else 'no response'}"):
        asyncio.run(load_docs_page(page, "https://example.com/docs"))
    page.wait_for_selector.assert_not_awaited()
    page.content.assert_not_awaited()


def test_page_readiness_rejects_missing_docs_body(load_docs_page):
    page = docs_page(SimpleNamespace(ok=True, status=200))
    page.wait_for_selector.side_effect = TimeoutError("Docs body never attached")
    with pytest.raises(TimeoutError, match="Docs body never attached"):
        asyncio.run(load_docs_page(page, "https://example.com/docs"))
    page.content.assert_not_awaited()


def test_page_readiness_propagates_navigation_timeout(load_docs_page):
    page = docs_page(SimpleNamespace(ok=True, status=200))
    page.goto.side_effect = TimeoutError("Navigation timeout")
    with pytest.raises(TimeoutError, match="Navigation timeout"):
        asyncio.run(load_docs_page(page, "https://example.com/docs"))
    page.wait_for_selector.assert_not_awaited()
    page.content.assert_not_awaited()


def corpus():
    pages = json.loads((Path(__file__).parent / "fixtures/indexing/pages.json").read_text())
    return index.capture_corpus(pages, [p["url"] for p in pages])


def vector_writer(path, chunks):
    lancedb.connect(path).create_table("docs", data=[{**c, "vector": [0.1] * 384} for c in chunks])


def test_missing_discovered_page_fails():
    source = corpus()
    with pytest.raises(ValueError, match="discovered URLs"):
        index.capture_corpus(source["pages"][:-1], source["expected_urls"])
    with pytest.raises(ValueError, match="failed"):
        index.capture_corpus(
            source["pages"], source["expected_urls"], ["https://example.com/docs/failed"]
        )


def test_corrupted_snapshot_fails(tmp_path):
    source = corpus()
    source["pages"][0]["markdown"] = "Changed after capture"
    index.write_json(tmp_path / "corpus.json", source)
    with pytest.raises(ValueError, match="identity mismatch"):
        index.load_corpus(tmp_path / "corpus.json")


def test_one_corpus_includes_short_pages_in_both_indexes(tmp_path):
    path, manifest = index.build_generation(corpus(), tmp_path, vector_writer)
    table = index.open_vectors(path)
    with index.validate_pair(path, manifest, table) as conn:
        sql_urls = {r[0] for r in conn.execute("SELECT url FROM pages")}
        vector_urls = {r["url"] for r in table.search().select(["url"]).to_list()}
        assert sql_urls == vector_urls == set(manifest["included_urls"])
        assert len(sql_urls) == 3
        assert (
            conn.execute(
                "SELECT COUNT(*) FROM pages_fts WHERE pages_fts MATCH 'searchable'"
            ).fetchone()[0]
            == 2
        )
        for url in sql_urls:
            assert table.search([0.1] * 384).where(f"url = '{url}'").limit(1).to_list()
    assert manifest["exclusions"] == [
        {"url": "https://example.com/docs/empty", "reason": "no searchable text"}
    ]


def test_missing_vector_page_rejects_build(tmp_path):
    def incomplete(path, chunks):
        vector_writer(path, chunks[:-1])

    with pytest.raises(ValueError, match="corpus or build identity"):
        index.build_generation(corpus(), tmp_path, incomplete)
    assert not (tmp_path / "current.json").exists()


def test_mixed_vector_build_id_rejects_build(tmp_path):
    def mixed(path, chunks):
        vector_writer(path, [{**c, "build_id": "old-build"} for c in chunks])

    with pytest.raises(ValueError, match="build identity"):
        index.build_generation(corpus(), tmp_path, mixed)


def test_failed_embedding_build_preserves_previous_generation(tmp_path):
    _, manifest = index.build_generation(corpus(), tmp_path, vector_writer)
    index.activate(tmp_path, manifest)

    def fail(path, chunks):
        raise RuntimeError("Synthetic embedding failure")

    with pytest.raises(RuntimeError, match="embedding failure"):
        index.build_generation(corpus(), tmp_path, fail)
    assert json.loads((tmp_path / "current.json").read_text()) == index.pointer_for(manifest)


def test_reader_writer_dependency_pins_match():
    import tomllib

    scripts = Path(__file__).resolve().parents[1]
    project = tomllib.loads((scripts / "docs-mcp-server/pyproject.toml").read_text())
    lock = tomllib.loads((scripts / "docs-mcp-server/uv.lock").read_text())
    versions = {p["name"]: p["version"] for p in lock["package"]}
    strings = {
        n.value
        for n in ast.walk(ast.parse((scripts / "modal_app.py").read_text()))
        if isinstance(n, ast.Constant) and isinstance(n.value, str)
    }
    for dependency in ("lancedb", "pyarrow", "sentence-transformers"):
        pin = f"{dependency}=={versions[dependency]}"
        assert pin in project["project"]["dependencies"]
        assert pin in strings
        assert index.WRITER_VERSIONS[dependency] == versions[dependency]
