#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11"
# dependencies = [
#     "aiohttp>=3.9",
#     "beautifulsoup4>=4.12",
#     "lancedb>=0.6",
#     "openai>=1.0",
# ]
# ///
"""
docs-mcp standalone crawler
============================

Lightweight version of the Modal crawl job for local development and
one-off re-indexing without deploying to Modal.

Usage:
    uv run docs/scripts/crawl_docs.py [--out-dir ./docs_db_local]

Requirements:
    OPENAI_API_KEY env var (for embedding; skip with --no-embed to crawl only)

This script uses the same URL filter logic as modal_app.py so local
test results match production behaviour.
"""

from __future__ import annotations

import argparse
import asyncio
import hashlib
import logging
import re
import sqlite3
import tempfile
from pathlib import Path
from urllib.parse import urljoin, urlparse

import aiohttp
from bs4 import BeautifulSoup

logger = logging.getLogger(__name__)

# ---------------------------------------------------------------------------
# Config (kept in sync with modal_app.py)
# ---------------------------------------------------------------------------

BASE_URL = "https://cua.ai"

# Seed URLs — the crawler starts here and follows links that pass is_valid_url().
# Add every top-level product / docs page that does NOT live under /docs/.
SEED_URLS: list[str] = [
    # Main docs root
    f"{BASE_URL}/docs",
    # Product pages that live outside /docs/ and must be explicitly seeded.
    # Without these the crawler would never discover them because it only
    # follows links from already-visited cua.ai pages.
    f"{BASE_URL}/cua-driver",
]

# URL path prefixes that are valid docs/product content pages.
# Extend this list whenever a new top-level product page is added to the
# website's routes.ts that should appear in the Docs Assistant index.
VALID_PATH_PREFIXES: tuple[str, ...] = (
    "/docs",        # primary docs tree
    "/cua-driver",  # product landing page (moved out of /docs in 2026-06 migration)
)

EMBED_MODEL = "text-embedding-3-small"
EMBED_DIMENSIONS = 1536
CHUNK_SIZE = 800
CHUNK_OVERLAP = 100

MAX_CONCURRENT_REQUESTS = 5
REQUEST_TIMEOUT = 30
CRAWL_DELAY = 0.25


# ---------------------------------------------------------------------------
# URL filtering
# ---------------------------------------------------------------------------

def is_valid_url(url: str) -> bool:
    """Return True if *url* should be crawled and indexed.

    Rules:
    - Must be on cua.ai (exact hostname match, no subdomains).
    - Path must start with one of VALID_PATH_PREFIXES.
    - Skip non-HTML resources and internal/admin paths.
    """
    try:
        parsed = urlparse(url)
    except ValueError:
        return False

    if parsed.netloc not in ("cua.ai", "www.cua.ai"):
        return False

    path = parsed.path.rstrip("/") or "/"

    # Skip API, auth, admin, and other non-content paths
    skip_prefixes = (
        "/api/",
        "/admin",
        "/signin",
        "/signup",
        "/cli-auth",
        "/connect/",
        "/dashboard",
        "/maintenance",
        "/docs/api/copilotkit",
        "/docs/llms",
        "/docs-md",
    )
    if any(path.startswith(p) for p in skip_prefixes):
        return False

    # Only index paths that belong to docs or known product pages
    if not any(path.startswith(prefix) for prefix in VALID_PATH_PREFIXES):
        return False

    return True


# ---------------------------------------------------------------------------
# Crawling
# ---------------------------------------------------------------------------

async def fetch_page(session: aiohttp.ClientSession, url: str) -> tuple[str, str] | None:
    """Fetch *url* and return ``(final_url, html)`` or ``None`` on failure."""
    try:
        async with session.get(url, timeout=aiohttp.ClientTimeout(total=REQUEST_TIMEOUT)) as resp:
            if resp.status != 200:
                logger.warning("HTTP %d for %s", resp.status, url)
                return None
            content_type = resp.headers.get("content-type", "")
            if "text/html" not in content_type:
                return None
            html = await resp.text()
            return (str(resp.url), html)
    except Exception as exc:
        logger.warning("Failed to fetch %s: %s", url, exc)
        return None


def extract_links(base_url: str, html: str) -> list[str]:
    """Extract and normalise valid links from *html* relative to *base_url*."""
    soup = BeautifulSoup(html, "html.parser")
    links: list[str] = []
    for tag in soup.find_all("a", href=True):
        href = tag["href"].strip()
        if not href or href.startswith("#") or href.startswith("mailto:"):
            continue
        absolute = urljoin(base_url, href)
        parsed = urlparse(absolute)
        clean = parsed._replace(fragment="", query="").geturl()
        if is_valid_url(clean):
            links.append(clean)
    return links


def extract_text(url: str, html: str) -> dict:
    """Strip navigation chrome from *html* and return a ``{url, title, text}`` dict."""
    soup = BeautifulSoup(html, "html.parser")
    for tag in soup(["script", "style", "nav", "footer", "header"]):
        tag.decompose()
    title_tag = soup.find("title")
    title = title_tag.get_text(strip=True) if title_tag else url
    main = soup.find("main") or soup.find("article") or soup.body or soup
    text = re.sub(r"\n{3,}", "\n\n", main.get_text(separator="\n", strip=True))
    return {"url": url, "title": title, "text": text}


async def crawl(seed_urls: list[str]) -> list[dict]:
    """BFS crawl starting from *seed_urls*; return list of ``{url, title, text}`` dicts."""
    visited: set[str] = set()
    queue: list[str] = list(seed_urls)
    pages: list[dict] = []

    connector = aiohttp.TCPConnector(limit=MAX_CONCURRENT_REQUESTS)
    headers = {"User-Agent": "cua-docs-mcp-crawler/1.0 (+https://cua.ai)"}

    async with aiohttp.ClientSession(connector=connector, headers=headers) as session:
        while queue:
            batch, queue = queue[:MAX_CONCURRENT_REQUESTS], queue[MAX_CONCURRENT_REQUESTS:]
            tasks = [fetch_page(session, u) for u in batch if u not in visited]
            for url in batch:
                visited.add(url)

            results = await asyncio.gather(*tasks)
            for result in results:
                if result is None:
                    continue
                final_url, html = result
                visited.add(final_url)
                page = extract_text(final_url, html)
                if page["text"].strip():
                    pages.append(page)
                    logger.info("Crawled: %s (%d chars)", final_url, len(page["text"]))
                for link in extract_links(final_url, html):
                    if link not in visited:
                        queue.append(link)

            await asyncio.sleep(CRAWL_DELAY)

    logger.info("Crawl complete. %d pages indexed.", len(pages))
    return pages


# ---------------------------------------------------------------------------
# Chunking
# ---------------------------------------------------------------------------

def chunk_text(text: str, chunk_size: int = CHUNK_SIZE, overlap: int = CHUNK_OVERLAP) -> list[str]:
    """Split *text* into overlapping chunks of roughly *chunk_size* chars."""
    if len(text) <= chunk_size:
        return [text]
    chunks: list[str] = []
    start = 0
    while start < len(text):
        end = min(start + chunk_size, len(text))
        boundary = text.rfind("\n\n", start, end)
        if boundary > start + overlap:
            end = boundary
        chunks.append(text[start:end].strip())
        if end >= len(text):
            break
        start = end - overlap
    return [c for c in chunks if c]


# ---------------------------------------------------------------------------
# Database
# ---------------------------------------------------------------------------

def build_databases(pages: list[dict], out_dir: Path) -> None:
    """Embed *pages* with OpenAI and write LanceDB + SQLite FTS5 databases to *out_dir*."""
    import os
    import lancedb
    import openai

    oai = openai.OpenAI(api_key=os.environ["OPENAI_API_KEY"])

    rows: list[dict] = []
    for page in pages:
        for i, chunk in enumerate(chunk_text(page["text"])):
            chunk_id = hashlib.sha256(f"{page['url']}#{i}".encode()).hexdigest()[:16]
            rows.append({
                "id": chunk_id,
                "url": page["url"],
                "title": page["title"],
                "chunk_index": i,
                "text": chunk,
            })

    if not rows:
        logger.warning("No chunks to index.")
        return

    logger.info("Embedding %d chunks...", len(rows))
    vectors: list[list[float]] = []
    batch_size = 100
    for i in range(0, len(rows), batch_size):
        batch = [r["text"] for r in rows[i : i + batch_size]]
        resp = oai.embeddings.create(model=EMBED_MODEL, input=batch, dimensions=EMBED_DIMENSIONS)
        vectors.extend([item.embedding for item in resp.data])

    # LanceDB
    lance_path = str(out_dir / "lance")
    db = lancedb.connect(lance_path)
    table_data = [{**row, "vector": vec} for row, vec in zip(rows, vectors, strict=True)]
    if "docs" in db.table_names():
        db.drop_table("docs")
    db.create_table("docs", data=table_data)
    logger.info("LanceDB written: %s", lance_path)

    # SQLite FTS5
    sqlite_path = out_dir / "fts.db"
    con = sqlite3.connect(str(sqlite_path))
    con.execute("DROP TABLE IF EXISTS docs_fts")
    con.execute("""
        CREATE VIRTUAL TABLE docs_fts USING fts5(
            id UNINDEXED,
            url UNINDEXED,
            title,
            text,
            tokenize='porter unicode61'
        )
    """)
    con.executemany(
        "INSERT INTO docs_fts(id, url, title, text) VALUES (?, ?, ?, ?)",
        [(r["id"], r["url"], r["title"], r["text"]) for r in rows],
    )
    con.commit()
    con.close()
    logger.info("SQLite FTS5 written: %s", sqlite_path)


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------

def main() -> None:
    """CLI entry point: parse arguments, crawl, and optionally build databases."""
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--out-dir", default="./docs_db_local", help="Output directory for databases")
    parser.add_argument("--no-embed", action="store_true", help="Crawl only; skip embedding and database build")
    parser.add_argument("--seed", action="append", dest="seeds", metavar="URL",
                        help="Additional seed URL (may be repeated; appended to defaults)")
    args = parser.parse_args()

    logging.basicConfig(level=logging.INFO, format="%(levelname)s %(message)s")

    seeds = list(SEED_URLS)
    if args.seeds:
        seeds.extend(args.seeds)

    logger.info("Seeds: %s", seeds)
    pages = asyncio.run(crawl(seeds))
    logger.info("Total pages crawled: %d", len(pages))

    if args.no_embed:
        for p in pages:
            print(f"  {p['url']} ({len(p['text'])} chars)")
        return

    import os
    if "OPENAI_API_KEY" not in os.environ:
        logger.error("OPENAI_API_KEY not set. Use --no-embed to skip embedding.")
        raise SystemExit(1)

    out_dir = Path(args.out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    build_databases(pages, out_dir)
    logger.info("Done. Databases written to %s", out_dir)


if __name__ == "__main__":
    main()
