#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11"
# dependencies = [
#     "modal>=0.73",
#     "aiohttp>=3.9",
#     "beautifulsoup4>=4.12",
#     "lancedb>=0.6",
#     "openai>=1.0",
#     "boto3>=1.34",
# ]
# ///
"""
docs-mcp-server Modal crawl job
================================

Crawls cua.ai/docs/* and top-level product pages (e.g. /cua-driver),
embeds them with OpenAI text-embedding-3-small, and writes the result to
LanceDB + SQLite databases in S3.

Scheduled daily at 06:00 UTC (see `scheduled_crawl`).

Usage (manual trigger):
    modal run docs/scripts/modal_app.py::scheduled_crawl

Architecture:
    crawl → chunk → embed → LanceDB vector table
                          → SQLite FTS5 table
    Both databases are synced to s3://trycua-docs-mcp-data/docs_db/ after
    each run and pulled to the K3s cluster by docs-mcp-s3-sync CronJob.
"""

from __future__ import annotations

import asyncio
import hashlib
import json
import logging
import re
import sqlite3
import tempfile
import time
from pathlib import Path
from typing import AsyncGenerator
from urllib.parse import urljoin, urlparse

import aiohttp
import boto3
import lancedb
import modal
import openai
from bs4 import BeautifulSoup

logger = logging.getLogger(__name__)

# ---------------------------------------------------------------------------
# Modal app + image
# ---------------------------------------------------------------------------

app = modal.App("docs-mcp-crawl")

crawler_image = (
    modal.Image.debian_slim(python_version="3.12")
    .pip_install(
        "aiohttp>=3.9",
        "beautifulsoup4>=4.12",
        "lancedb>=0.6",
        "openai>=1.0",
        "boto3>=1.34",
    )
)

# ---------------------------------------------------------------------------
# Config
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

S3_BUCKET = "trycua-docs-mcp-data"
S3_DOCS_DB_PREFIX = "docs_db"

EMBED_MODEL = "text-embedding-3-small"
EMBED_DIMENSIONS = 1536
CHUNK_SIZE = 800   # tokens (approximate chars / 4)
CHUNK_OVERLAP = 100

MAX_CONCURRENT_REQUESTS = 5
REQUEST_TIMEOUT = 30
CRAWL_DELAY = 0.25  # seconds between requests per domain


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
    """Fetch *url* and return (url, html) or None on failure."""
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
    """Extract absolute links from *html* that pass is_valid_url()."""
    soup = BeautifulSoup(html, "html.parser")
    links: list[str] = []
    for tag in soup.find_all("a", href=True):
        href = tag["href"].strip()
        if not href or href.startswith("#") or href.startswith("mailto:"):
            continue
        absolute = urljoin(base_url, href)
        # Strip fragment + query for dedup
        parsed = urlparse(absolute)
        clean = parsed._replace(fragment="", query="").geturl()
        if is_valid_url(clean):
            links.append(clean)
    return links


def extract_text(url: str, html: str) -> dict:
    """Extract clean text and metadata from an HTML page."""
    soup = BeautifulSoup(html, "html.parser")

    # Remove script/style/nav noise
    for tag in soup(["script", "style", "nav", "footer", "header"]):
        tag.decompose()

    title_tag = soup.find("title")
    title = title_tag.get_text(strip=True) if title_tag else url

    # Prefer main content area
    main = soup.find("main") or soup.find("article") or soup.body or soup
    text = re.sub(r"\n{3,}", "\n\n", main.get_text(separator="\n", strip=True))

    return {"url": url, "title": title, "text": text}


async def crawl(seed_urls: list[str]) -> list[dict]:
    """BFS crawl starting from *seed_urls*. Returns list of page dicts."""
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

                new_links = extract_links(final_url, html)
                for link in new_links:
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
        # Try to break at a paragraph boundary
        boundary = text.rfind("\n\n", start, end)
        if boundary > start + overlap:
            end = boundary
        chunks.append(text[start:end].strip())
        if end >= len(text):
            break
        start = end - overlap
    return [c for c in chunks if c]


# ---------------------------------------------------------------------------
# Embedding
# ---------------------------------------------------------------------------

def embed_chunks(client: openai.OpenAI, texts: list[str]) -> list[list[float]]:
    """Embed *texts* in batches and return vectors."""
    vectors: list[list[float]] = []
    batch_size = 100
    for i in range(0, len(texts), batch_size):
        batch = texts[i : i + batch_size]
        response = client.embeddings.create(model=EMBED_MODEL, input=batch, dimensions=EMBED_DIMENSIONS)
        vectors.extend([item.embedding for item in response.data])
    return vectors


# ---------------------------------------------------------------------------
# Database
# ---------------------------------------------------------------------------

def build_databases(pages: list[dict], out_dir: Path) -> None:
    """Write LanceDB + SQLite FTS databases to *out_dir*."""
    import os
    openai_key = os.environ["OPENAI_API_KEY"]
    oai = openai.OpenAI(api_key=openai_key)

    # Build chunks
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
    texts = [r["text"] for r in rows]
    vectors = embed_chunks(oai, texts)

    # LanceDB
    lance_path = str(out_dir / "lance")
    db = lancedb.connect(lance_path)
    table_data = [
        {**row, "vector": vec}
        for row, vec in zip(rows, vectors, strict=True)
    ]
    if "docs" in db.table_names():
        db.drop_table("docs")
    db.create_table("docs", data=table_data)
    logger.info("LanceDB table written: %s", lance_path)

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
# S3 upload
# ---------------------------------------------------------------------------

def upload_to_s3(local_dir: Path, bucket: str, prefix: str) -> None:
    """Recursively upload *local_dir* to s3://*bucket*/*prefix*/."""
    s3 = boto3.client("s3")
    for path in local_dir.rglob("*"):
        if path.is_file():
            key = f"{prefix}/{path.relative_to(local_dir)}"
            logger.info("Uploading s3://%s/%s", bucket, key)
            s3.upload_file(str(path), bucket, key)


# ---------------------------------------------------------------------------
# Modal entrypoints
# ---------------------------------------------------------------------------

@app.function(
    image=crawler_image,
    schedule=modal.Cron("0 6 * * *"),
    timeout=3600,
    secrets=[
        modal.Secret.from_name("openai-api-key"),
        modal.Secret.from_name("modal-aws-oidc"),
    ],
)
def scheduled_crawl():
    """Daily scheduled crawl — runs at 06:00 UTC every day."""
    logging.basicConfig(level=logging.INFO, format="%(levelname)s %(message)s")
    logger.info("Starting scheduled docs crawl. Seeds: %s", SEED_URLS)

    pages = asyncio.run(crawl(SEED_URLS))

    with tempfile.TemporaryDirectory() as tmp:
        out_dir = Path(tmp) / "docs_db"
        out_dir.mkdir()
        build_databases(pages, out_dir)
        upload_to_s3(out_dir, S3_BUCKET, S3_DOCS_DB_PREFIX)

    logger.info("Crawl and upload complete.")


@app.local_entrypoint()
def main():
    """Local test: run the crawl without uploading to S3."""
    logging.basicConfig(level=logging.INFO, format="%(levelname)s %(message)s")
    import os

    pages = asyncio.run(crawl(SEED_URLS))
    out_dir = Path("./docs_db_local")
    out_dir.mkdir(exist_ok=True)

    if "OPENAI_API_KEY" in os.environ:
        build_databases(pages, out_dir)
        logger.info("Local databases written to %s", out_dir)
    else:
        logger.info("OPENAI_API_KEY not set — skipping embedding. %d pages crawled.", len(pages))
        for p in pages:
            print(f"  {p['url']} ({len(p['text'])} chars)")
