"""
Modal app for CUA documentation crawling and MCP server

This app provides:
1. Scheduled daily crawling of cua.ai/docs stored in a Modal volume
2. MCP server that serves documentation search over the crawled data

Usage:
    modal deploy docs/scripts/modal_app.py
"""

import asyncio
import json
import re
import sqlite3
from pathlib import Path
from typing import Optional

import modal
from markdown_it import MarkdownIt

# Define the Modal app
app = modal.App("cua-docs-mcp")

# Create persistent volumes for storing data
docs_volume = modal.Volume.from_name("cua-docs-data", create_if_missing=True)
code_volume = modal.Volume.from_name("cua-code-index", create_if_missing=True)

# Define the container image with all dependencies
image = (
    modal.Image.debian_slim(python_version="3.12")
    .apt_install("git")
    .pip_install(
        "crawl4ai>=0.4.0",
        "playwright>=1.40.0",
        "lancedb>=0.4.0",
        "sentence-transformers>=2.2.0",
        "pyarrow>=14.0.1",
        "fastapi>=0.100.0",
        "fastmcp>=2.14.0",
        "pydantic>=2.0.0",
        "pandas>=2.0.0",
        "markdown-it-py>=3.0.0",
        "markitdown>=0.0.1",
    )
    .run_commands("playwright install --with-deps chromium")
)

# Volume mount paths
VOLUME_PATH = "/data"
CRAWLED_DATA_PATH = f"{VOLUME_PATH}/crawled_data"
DB_PATH = f"{VOLUME_PATH}/docs_db"

# Code index volume mount path
CODE_VOLUME_PATH = "/code_data"
CODE_REPO_PATH = f"{CODE_VOLUME_PATH}/repo"
CODE_DB_PATH = f"{CODE_VOLUME_PATH}/code_db"


# =============================================================================
# Helper Functions
# =============================================================================


def clean_markdown(markdown: str) -> str:
    """Extract plain text content from markdown using markdown-it-py parser"""
    md_parser = MarkdownIt()
    tokens = md_parser.parse(markdown)

    text_parts = []

    def extract_text(token_list):
        for token in token_list:
            if token.type == "inline" and token.children:
                for child in token.children:
                    if child.type == "text":
                        text_parts.append(child.content)
                    elif child.type == "code_inline":
                        text_parts.append(child.content)
                    elif child.type == "softbreak":
                        text_parts.append(" ")
                    elif child.type == "hardbreak":
                        text_parts.append("\n")
            elif token.type == "fence" or token.type == "code_block":
                text_parts.append(token.content)
                text_parts.append("\n")

            if token.children:
                extract_text(token.children)

            if token.type in [
                "heading_close",
                "paragraph_close",
                "list_item_close",
                "blockquote_close",
            ]:
                text_parts.append("\n")

    extract_text(tokens)
    text = "".join(text_parts)
    text = re.sub(r"\n{3,}", "\n\n", text)
    text = re.sub(r" {2,}", " ", text)
    return text.strip()


# =============================================================================
# Crawling Functions
# =============================================================================


@app.function(
    image=image,
    volumes={VOLUME_PATH: docs_volume},
    timeout=3600,  # 1 hour timeout
    cpu=2.0,
    memory=4096,
)
async def crawl_docs():
    """Crawl CUA documentation and save to volume"""
    import re
    import shutil
    from urllib.parse import urljoin, urlparse

    from crawl4ai import AsyncWebCrawler, BrowserConfig, CrawlerRunConfig

    print("Starting documentation crawl...")

    BASE_URL = "https://cua.ai"
    DOCS_URL = f"{BASE_URL}/docs"
    OUTPUT_DIR = Path(CRAWLED_DATA_PATH)

    # Clear existing crawled data to ensure fresh results
    if OUTPUT_DIR.exists():
        shutil.rmtree(OUTPUT_DIR)
        print("Cleared existing crawled data")

    OUTPUT_DIR.mkdir(parents=True, exist_ok=True)

    visited_urls = set()
    to_visit = set()
    failed_urls = set()
    all_data = []

    def normalize_url(url: str) -> str:
        """Normalize URL to avoid duplicates"""
        parsed = urlparse(url)
        path = parsed.path.rstrip("/")
        if not path:
            path = ""
        return f"{parsed.scheme}://{parsed.netloc}{path}"

    def is_valid_url(url: str) -> bool:
        """Check if URL should be crawled (only /docs pages)"""
        parsed = urlparse(url)
        if parsed.netloc and parsed.netloc not in ["cua.ai", "www.cua.ai"]:
            return False
        if not parsed.path.startswith("/docs"):
            return False
        # Skip non-page resources
        excluded_extensions = [
            ".pdf",
            ".zip",
            ".png",
            ".jpg",
            ".jpeg",
            ".gif",
            ".svg",
            ".ico",
            ".css",
            ".js",
        ]
        if any(parsed.path.lower().endswith(ext) for ext in excluded_extensions):
            return False
        return True

    def extract_links(html: str, base_url: str) -> set[str]:
        """Extract all valid links from HTML"""
        links = set()
        # Find all href attributes
        href_pattern = r'href=["\']([^"\']+)["\']'
        matches = re.findall(href_pattern, html)

        for match in matches:
            full_url = urljoin(base_url, match)
            normalized = normalize_url(full_url)
            if is_valid_url(normalized):
                links.add(normalized)

        return links

    def extract_path_info(url: str) -> dict:
        """Extract meaningful path information from URL"""
        parsed = urlparse(url)
        path = parsed.path.replace("/docs/", "").strip("/")
        parts = path.split("/") if path else []

        return {
            "path": path,
            "category": parts[0] if parts else "root",
            "subcategory": parts[1] if len(parts) > 1 else None,
            "page": parts[-1] if parts else "index",
            "depth": len(parts),
        }

    def save_page(url: str, data: dict):
        """Save page data to a JSON file"""
        parsed = urlparse(url)
        path = parsed.path.strip("/") or "index"
        filename = path.replace("/", "_") + ".json"

        filepath = OUTPUT_DIR / filename
        with open(filepath, "w", encoding="utf-8") as f:
            json.dump(data, f, indent=2, ensure_ascii=False)

    # Seed URLs
    seed_urls = [
        DOCS_URL,
        f"{DOCS_URL}/cua",
        f"{DOCS_URL}/cua/guide",
        f"{DOCS_URL}/cua/guide/get-started",
        f"{DOCS_URL}/cua/reference",
        f"{DOCS_URL}/cua/reference/computer-sdk",
        f"{DOCS_URL}/cuabench",
        f"{DOCS_URL}/cuabench/guide",
        f"{DOCS_URL}/cuabench/reference",
    ]

    for url in seed_urls:
        normalized = normalize_url(url)
        if is_valid_url(normalized):
            to_visit.add(normalized)

    browser_config = BrowserConfig(
        headless=True,
        verbose=False,
    )

    async with AsyncWebCrawler(config=browser_config) as crawler:
        while to_visit:
            # Get batch of URLs to crawl
            batch = []
            MAX_CONCURRENT = 5
            while to_visit and len(batch) < MAX_CONCURRENT:
                url = to_visit.pop()
                if url not in visited_urls:
                    batch.append(url)
                    visited_urls.add(url)

            if not batch:
                break

            # Crawl each URL in batch
            for url in batch:
                try:
                    print(f"Crawling: {url}")

                    config = CrawlerRunConfig(
                        word_count_threshold=10,
                        exclude_external_links=True,
                    )

                    result = await crawler.arun(url=url, config=config)

                    if result.success:
                        # Extract new links from the page
                        new_links = extract_links(result.html, url)
                        for link in new_links:
                            if link not in visited_urls and link not in to_visit:
                                to_visit.add(link)

                        path_info = extract_path_info(url)

                        page_data = {
                            "url": url,
                            "title": result.metadata.get("title", "") if result.metadata else "",
                            "description": (
                                result.metadata.get("description", "") if result.metadata else ""
                            ),
                            "markdown": result.markdown,
                            "path_info": path_info,
                            "links_found": list(new_links),
                        }

                        # Save individual page
                        save_page(url, page_data)
                        all_data.append(page_data)

                        await asyncio.sleep(0.5)
                    else:
                        print(f"Failed to crawl {url}: {result.error_message}")
                        failed_urls.add(url)

                except Exception as e:
                    print(f"Error crawling {url}: {e}")
                    failed_urls.add(url)

            print(f"Progress: {len(visited_urls)} crawled, {len(to_visit)} remaining")

    # Save summary
    summary = {
        "total_pages": len(all_data),
        "failed_urls": list(failed_urls),
        "all_urls": list(visited_urls),
    }

    with open(OUTPUT_DIR / "_summary.json", "w", encoding="utf-8") as f:
        json.dump(summary, f, indent=2)

    # Save all data in one file too
    with open(OUTPUT_DIR / "_all_pages.json", "w", encoding="utf-8") as f:
        json.dump(all_data, f, indent=2, ensure_ascii=False)

    # Commit changes to volume
    docs_volume.commit()

    print(f"Crawl complete! Crawled {len(all_data)} pages")
    return summary


@app.function(
    image=image,
    volumes={VOLUME_PATH: docs_volume},
    schedule=modal.Cron("0 6 * * *"),  # Daily at 6 AM UTC
    timeout=3600,
)
async def scheduled_crawl():
    """Scheduled daily crawl of documentation"""
    print("Running scheduled crawl...")
    summary = await crawl_docs.remote.aio()

    # Regenerate databases after crawl
    print("Generating databases...")
    await generate_vector_db.remote.aio()
    await generate_sqlite_db.remote.aio()

    print(f"Scheduled crawl complete: {summary}")
    return summary


# =============================================================================
# Database Generation Functions
# =============================================================================


@app.function(
    image=image,
    volumes={VOLUME_PATH: docs_volume},
    timeout=1800,  # 30 minutes
    cpu=2.0,
    memory=8192,
)
async def generate_vector_db():
    """Generate LanceDB vector database from crawled data"""
    import shutil
    import tempfile

    import lancedb
    from lancedb.embeddings import get_registry
    from lancedb.pydantic import LanceModel, Vector

    print("Generating LanceDB vector database...")

    CRAWLED_DIR = Path(CRAWLED_DATA_PATH)
    DB_DIR = Path(DB_PATH)
    DB_DIR.mkdir(parents=True, exist_ok=True)

    # Use /tmp for LanceDB operations (Modal volumes don't support atomic rename)
    TMP_LANCE_DIR = Path(tempfile.mkdtemp(prefix="lancedb_"))
    print(f"Using temp directory: {TMP_LANCE_DIR}")

    # Initialize embedding model
    model = get_registry().get("sentence-transformers").create(name="all-MiniLM-L6-v2")

    # Define schema with embedding configuration
    class DocsChunk(LanceModel):
        text: str = model.SourceField()
        vector: Vector(model.ndims()) = model.VectorField()
        url: str
        title: str
        category: str
        chunk_index: int

    # Load all crawled pages
    json_files = list(CRAWLED_DIR.glob("*.json"))
    json_files = [f for f in json_files if not f.name.startswith("_")]

    if not json_files:
        print("No crawled data found!")
        return

    all_chunks = []

    for json_file in json_files:
        with open(json_file, "r", encoding="utf-8") as f:
            page_data = json.load(f)

        url = page_data.get("url", "")
        title = page_data.get("title", "")
        markdown = page_data.get("markdown", "")
        category = page_data.get("path_info", {}).get("category", "unknown")

        if not markdown:
            continue

        # Convert markdown to plain text
        try:
            text = clean_markdown(markdown)
        except Exception:
            text = markdown

        # Simple chunking by paragraphs
        paragraphs = [p.strip() for p in text.split("\n\n") if p.strip()]

        for i, para in enumerate(paragraphs):
            if len(para) < 50:  # Skip very short paragraphs
                continue

            chunk = {
                "text": para,
                "url": url,
                "title": title,
                "category": category,
                "chunk_index": i,
            }
            all_chunks.append(chunk)

    if not all_chunks:
        print("No chunks generated!")
        return

    # Create LanceDB in temp directory (supports atomic operations)
    db = lancedb.connect(TMP_LANCE_DIR)

    # Create table with schema - embeddings are generated automatically
    table = db.create_table(
        "docs",
        schema=DocsChunk,
        mode="overwrite",
    )

    # Add data in batches for better performance
    batch_size = 100
    for i in range(0, len(all_chunks), batch_size):
        batch = all_chunks[i : i + batch_size]
        table.add(batch)
        print(
            f"Added batch {i // batch_size + 1}/{(len(all_chunks) + batch_size - 1) // batch_size}"
        )

    # Close the connection before copying
    del table
    del db

    # Copy completed database to Modal volume
    lance_db_dest = DB_DIR / "docs.lance"
    if lance_db_dest.exists():
        shutil.rmtree(lance_db_dest)
        print("Cleared existing vector database on volume")

    # Copy from temp to volume
    shutil.copytree(TMP_LANCE_DIR / "docs.lance", lance_db_dest)
    print(f"Copied LanceDB to volume: {lance_db_dest}")

    # Clean up temp directory
    shutil.rmtree(TMP_LANCE_DIR)

    # Commit changes to volume
    docs_volume.commit()

    print(f"Vector database created with {len(all_chunks)} chunks")
    return {"chunks": len(all_chunks)}


@app.function(
    image=image,
    volumes={VOLUME_PATH: docs_volume},
    timeout=1800,
    cpu=2.0,
    memory=4096,
)
async def generate_sqlite_db():
    """Generate SQLite FTS5 database from crawled data"""

    print("Generating SQLite FTS5 database...")

    CRAWLED_DIR = Path(CRAWLED_DATA_PATH)
    DB_DIR = Path(DB_PATH)
    DB_DIR.mkdir(parents=True, exist_ok=True)

    SQLITE_PATH = DB_DIR / "docs.sqlite"

    # Delete existing database to ensure fresh data
    if SQLITE_PATH.exists():
        SQLITE_PATH.unlink()
        print("Cleared existing SQLite database")

    # Create database
    conn = sqlite3.connect(SQLITE_PATH)
    cursor = conn.cursor()

    # Create tables
    cursor.execute(
        """
        CREATE TABLE pages (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            url TEXT UNIQUE NOT NULL,
            title TEXT,
            category TEXT,
            content TEXT
        )
    """
    )

    cursor.execute(
        """
        CREATE VIRTUAL TABLE pages_fts USING fts5(
            content,
            url UNINDEXED,
            title UNINDEXED,
            category UNINDEXED,
            content='pages',
            content_rowid='id'
        )
    """
    )

    # Load and insert data
    json_files = list(CRAWLED_DIR.glob("*.json"))
    json_files = [f for f in json_files if not f.name.startswith("_")]

    inserted = 0

    for json_file in json_files:
        with open(json_file, "r", encoding="utf-8") as f:
            page_data = json.load(f)

        url = page_data.get("url", "")
        title = page_data.get("title", "")
        markdown = page_data.get("markdown", "")
        category = page_data.get("path_info", {}).get("category", "unknown")

        if not markdown:
            continue

        # Convert markdown to plain text
        try:
            text = clean_markdown(markdown)
        except Exception:
            text = markdown

        try:
            cursor.execute(
                "INSERT OR REPLACE INTO pages (url, title, category, content) VALUES (?, ?, ?, ?)",
                (url, title, category, text),
            )
            inserted += 1
        except Exception as e:
            print(f"Error inserting {url}: {e}")

    # Create FTS triggers
    cursor.execute(
        """
        CREATE TRIGGER pages_ai AFTER INSERT ON pages BEGIN
            INSERT INTO pages_fts(rowid, content, url, title, category)
            VALUES (new.id, new.content, new.url, new.title, new.category);
        END;
    """
    )

    cursor.execute(
        """
        CREATE TRIGGER pages_ad AFTER DELETE ON pages BEGIN
            DELETE FROM pages_fts WHERE rowid = old.id;
        END;
    """
    )

    cursor.execute(
        """
        CREATE TRIGGER pages_au AFTER UPDATE ON pages BEGIN
            DELETE FROM pages_fts WHERE rowid = old.id;
            INSERT INTO pages_fts(rowid, content, url, title, category)
            VALUES (new.id, new.content, new.url, new.title, new.category);
        END;
    """
    )

    conn.commit()
    conn.close()

    # Commit changes to volume
    docs_volume.commit()

    print(f"SQLite database created with {inserted} pages")
    return {"pages": inserted}


# =============================================================================
# Code Index Generation Functions
# =============================================================================

# Source file extensions to index
SOURCE_EXTENSIONS = {".py", ".ts", ".js", ".tsx"}
MAX_FILE_SIZE_SQLITE = 1_000_000  # 1MB for SQLite
MAX_FILE_SIZE_EMBEDDINGS = 100_000  # 100KB for embeddings


def parse_tag(tag: str) -> tuple[str, str]:
    """Parse a git tag into component and version."""
    if tag.startswith("v") and len(tag) > 1 and tag[1].isdigit():
        return ("cua", tag[1:])
    match = re.match(r"^(.+)-v(\d+\.\d+\.\d+.*)$", tag)
    if match:
        return (match.group(1), match.group(2))
    raise ValueError(f"Cannot parse tag: {tag}")


def detect_language(file_path: str) -> str:
    """Detect programming language from file extension."""
    ext = Path(file_path).suffix.lower()
    return {".py": "python", ".ts": "typescript", ".tsx": "typescript", ".js": "javascript"}.get(
        ext, "unknown"
    )


@app.function(
    image=image,
    volumes={CODE_VOLUME_PATH: code_volume},
    timeout=3600,  # 1 hour
    cpu=2.0,
    memory=8192,
)
async def generate_code_index():
    """Generate code search index from all git tags"""
    import shutil
    import subprocess
    import tempfile

    import lancedb
    from lancedb.embeddings import get_registry
    from lancedb.pydantic import LanceModel, Vector

    print("Generating code search index...")

    REPO_URL = "https://github.com/anthropics/cua.git"
    REPO_PATH = Path(CODE_REPO_PATH)
    DB_DIR = Path(CODE_DB_PATH)
    SQLITE_PATH = DB_DIR / "code_index.sqlite"

    DB_DIR.mkdir(parents=True, exist_ok=True)

    # Clone or update repo (bare clone for efficiency)
    if REPO_PATH.exists():
        print("Fetching latest tags...")
        subprocess.run(["git", "fetch", "--all", "--tags"], cwd=REPO_PATH, check=True)
    else:
        print(f"Cloning {REPO_URL}...")
        REPO_PATH.parent.mkdir(parents=True, exist_ok=True)
        subprocess.run(["git", "clone", "--bare", REPO_URL, str(REPO_PATH)], check=True)

    # Get all tags
    result = subprocess.run(
        ["git", "tag"], cwd=REPO_PATH, check=True, capture_output=True, text=True
    )
    all_tags = [t.strip() for t in result.stdout.strip().split("\n") if t.strip()]
    print(f"Found {len(all_tags)} tags")

    # Initialize SQLite
    if SQLITE_PATH.exists():
        SQLITE_PATH.unlink()

    conn = sqlite3.connect(SQLITE_PATH)
    cursor = conn.cursor()

    cursor.execute(
        """
        CREATE TABLE code_files (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            component TEXT NOT NULL,
            version TEXT NOT NULL,
            file_path TEXT NOT NULL,
            content TEXT NOT NULL,
            language TEXT NOT NULL,
            UNIQUE(component, version, file_path)
        )
    """
    )
    cursor.execute("CREATE INDEX idx_component ON code_files(component)")
    cursor.execute("CREATE INDEX idx_version ON code_files(component, version)")

    cursor.execute(
        """
        CREATE VIRTUAL TABLE code_files_fts USING fts5(
            content,
            component UNINDEXED,
            version UNINDEXED,
            file_path UNINDEXED,
            content='code_files',
            content_rowid='id'
        )
    """
    )

    # FTS triggers
    cursor.execute(
        """
        CREATE TRIGGER code_files_ai AFTER INSERT ON code_files BEGIN
            INSERT INTO code_files_fts(rowid, content, component, version, file_path)
            VALUES (new.id, new.content, new.component, new.version, new.file_path);
        END;
    """
    )
    conn.commit()

    # Initialize LanceDB in temp directory
    TMP_LANCE_DIR = Path(tempfile.mkdtemp(prefix="code_lancedb_"))
    model = get_registry().get("sentence-transformers").create(name="all-MiniLM-L6-v2")

    class CodeFile(LanceModel):
        text: str = model.SourceField()
        vector: Vector(model.ndims()) = model.VectorField()
        component: str
        version: str
        file_path: str
        language: str

    lance_db = lancedb.connect(TMP_LANCE_DIR)
    lance_table = lance_db.create_table("code", schema=CodeFile, mode="overwrite")

    # Process each tag
    total_files = 0
    total_embedded = 0
    failed_tags = []

    for i, tag in enumerate(all_tags):
        print(f"[{i + 1}/{len(all_tags)}] Processing {tag}")

        try:
            component, version = parse_tag(tag)
        except ValueError as e:
            print(f"  Skipping: {e}")
            failed_tags.append(tag)
            continue

        # Get files at this tag
        try:
            result = subprocess.run(
                ["git", "ls-tree", "-r", "--name-only", tag],
                cwd=REPO_PATH,
                check=True,
                capture_output=True,
                text=True,
            )
            files = [
                f.strip()
                for f in result.stdout.strip().split("\n")
                if f.strip() and Path(f.strip()).suffix.lower() in SOURCE_EXTENSIONS
            ]
        except subprocess.CalledProcessError:
            failed_tags.append(tag)
            continue

        lance_batch = []

        for file_path in files:
            try:
                result = subprocess.run(
                    ["git", "show", f"{tag}:{file_path}"],
                    cwd=REPO_PATH,
                    check=True,
                    capture_output=True,
                )
                content = result.stdout.decode("utf-8", errors="replace")
                if "\x00" in content[:1024]:
                    continue  # Skip binary
            except (subprocess.CalledProcessError, UnicodeDecodeError):
                continue

            language = detect_language(file_path)
            content_size = len(content)

            # Add to SQLite
            if content_size <= MAX_FILE_SIZE_SQLITE:
                try:
                    cursor.execute(
                        "INSERT OR REPLACE INTO code_files (component, version, file_path, content, language) VALUES (?, ?, ?, ?, ?)",
                        (component, version, file_path, content, language),
                    )
                    total_files += 1
                except Exception:
                    pass

            # Queue for LanceDB
            if content_size <= MAX_FILE_SIZE_EMBEDDINGS:
                lance_batch.append(
                    {
                        "text": content,
                        "component": component,
                        "version": version,
                        "file_path": file_path,
                        "language": language,
                    }
                )

        conn.commit()

        # Add to LanceDB
        if lance_batch:
            try:
                lance_table.add(lance_batch)
                total_embedded += len(lance_batch)
            except Exception as e:
                print(f"  LanceDB error: {e}")

    conn.close()

    # Copy LanceDB to volume
    del lance_table
    del lance_db

    lance_dest = DB_DIR / "code_index.lancedb"
    if lance_dest.exists():
        shutil.rmtree(lance_dest)
    shutil.copytree(TMP_LANCE_DIR, lance_dest)
    shutil.rmtree(TMP_LANCE_DIR)

    code_volume.commit()

    print(f"Code index complete: {total_files} files in SQLite, {total_embedded} embedded")
    return {"files": total_files, "embedded": total_embedded, "failed_tags": len(failed_tags)}


@app.function(
    image=image,
    volumes={CODE_VOLUME_PATH: code_volume},
    schedule=modal.Cron("0 5 * * *"),  # Daily at 5 AM UTC (before docs crawl)
    timeout=3600,
)
async def scheduled_code_index():
    """Scheduled daily code index generation"""
    print("Running scheduled code indexing...")
    result = await generate_code_index.remote.aio()
    print(f"Code indexing complete: {result}")
    return result


# =============================================================================
# MCP Server
# =============================================================================


@app.function(
    image=image,
    volumes={VOLUME_PATH: docs_volume, CODE_VOLUME_PATH: code_volume},
    cpu=1.0,
    memory=2048,
)
@modal.concurrent(max_inputs=10)
@modal.asgi_app()
def web():
    """ASGI web endpoint for the MCP server"""
    import lancedb
    from fastmcp import FastMCP
    from lancedb.embeddings import get_registry
    from starlette.middleware.cors import CORSMiddleware

    # Initialize the MCP server
    mcp = FastMCP(
        name="CUA Docs & Code",
        instructions="""CUA Documentation and Code Server - provides search and retrieval for Computer Use Agent (CUA) documentation and versioned source code.

Documentation covers:
- CUA SDK: Python library for building computer-use agents with screen capture, mouse/keyboard control
- CUA Bench: Benchmarking framework for evaluating computer-use agents
- Agent Loop: Core execution loop for autonomous agent operation
- Sandboxes: Docker and cloud VM environments for safe agent execution
- Computer interfaces: Screen, mouse, keyboard, and bash interaction APIs

=== DOCUMENTATION TOOLS ===
- search_docs: Semantic/vector search - best for conceptual queries
- sql_query: Direct SQL/FTS5 queries for exact keyword matches

=== CODE SEARCH TOOLS ===
Search across all versions of CUA source code. Component + version are REQUIRED.

Discovery:
- list_components: See all indexed components (agent, computer, mcp-server, etc.)
- list_versions: See all indexed versions for a component

Search (requires component + version):
- search_code: Semantic search - "how to take a screenshot"
- search_code_fts: Full-text search - exact function/class names

Retrieval:
- get_code_file_content: Get full source file content

Workflow:
1. list_components() -> see available components
2. list_versions("agent") -> see versions
3. search_code("ComputerAgent init", "agent", "0.7.3") -> find files
4. get_code_file_content("agent", "0.7.3", "agent/computer_agent.py") -> full file

Always cite sources: URLs for docs, component@version:path for code.""",
    )

    # Initialize embedding model
    model = get_registry().get("sentence-transformers").create(name="all-MiniLM-L6-v2")

    # Global database connections
    _lance_db = None
    _lance_table = None
    _sqlite_conn = None
    _code_lance_db = None
    _code_lance_table = None
    _code_sqlite_conn = None

    def get_lance_table():
        """Get or create LanceDB connection for docs"""
        nonlocal _lance_db, _lance_table
        if _lance_table is None:
            db_path = Path(DB_PATH)
            if not db_path.exists():
                raise RuntimeError("Database not found. Run crawl and generation functions first.")
            _lance_db = lancedb.connect(db_path)
            _lance_table = _lance_db.open_table("docs")
        return _lance_table

    def get_sqlite_conn():
        """Get or create read-only SQLite connection for docs"""
        nonlocal _sqlite_conn
        if _sqlite_conn is None:
            sqlite_path = Path(DB_PATH) / "docs.sqlite"
            if not sqlite_path.exists():
                raise RuntimeError("SQLite database not found.")
            _sqlite_conn = sqlite3.connect(f"file:{sqlite_path}?mode=ro", uri=True)
            _sqlite_conn.row_factory = sqlite3.Row
        return _sqlite_conn

    def get_code_lance_table():
        """Get or create LanceDB connection for code"""
        nonlocal _code_lance_db, _code_lance_table
        if _code_lance_table is None:
            db_path = Path(CODE_DB_PATH) / "code_index.lancedb"
            if not db_path.exists():
                raise RuntimeError("Code index not found. Run generate_code_index first.")
            _code_lance_db = lancedb.connect(db_path)
            _code_lance_table = _code_lance_db.open_table("code")
        return _code_lance_table

    def get_code_sqlite_conn():
        """Get or create read-only SQLite connection for code"""
        nonlocal _code_sqlite_conn
        if _code_sqlite_conn is None:
            sqlite_path = Path(CODE_DB_PATH) / "code_index.sqlite"
            if not sqlite_path.exists():
                raise RuntimeError("Code SQLite database not found.")
            _code_sqlite_conn = sqlite3.connect(f"file:{sqlite_path}?mode=ro", uri=True)
            _code_sqlite_conn.row_factory = sqlite3.Row
        return _code_sqlite_conn

    # =================== DOCUMENTATION TOOLS ===================

    @mcp.tool()
    def search_docs(query: str, limit: int = 5, category: Optional[str] = None) -> list[dict]:
        """
        Semantic search over CUA documentation using vector embeddings.
        Best for conceptual queries like "how does the agent loop work?"
        """
        limit = min(max(1, limit), 20)
        table = get_lance_table()
        search = table.search(query).limit(limit)
        if category:
            safe_category = category.replace("'", "''")
            search = search.where(f"category = '{safe_category}'")
        results = search.to_list()
        return [
            {
                "text": r.get("text", ""),
                "url": r.get("url", ""),
                "title": r.get("title", ""),
                "category": r.get("category", ""),
                "score": float(r.get("_distance", 0.0)),
            }
            for r in results
        ]

    @mcp.tool()
    def sql_query(query: str) -> list[dict]:
        """
        Execute a read-only SQL query on the documentation database.
        Tables: pages (id, url, title, category, content), pages_fts (FTS5)
        """
        conn = get_sqlite_conn()
        cursor = conn.cursor()
        try:
            cursor.execute(query)
            results = cursor.fetchall()
            return [dict(row) for row in results]
        except sqlite3.Error as e:
            return [{"error": str(e)}]

    # =================== CODE SEARCH TOOLS ===================

    @mcp.tool()
    def list_components() -> list[dict]:
        """List all indexed code components with version counts."""
        try:
            conn = get_code_sqlite_conn()
            cursor = conn.cursor()
            cursor.execute(
                "SELECT component, COUNT(DISTINCT version) as version_count FROM code_files GROUP BY component ORDER BY component"
            )
            return [{"component": r["component"], "version_count": r["version_count"]} for r in cursor.fetchall()]
        except Exception as e:
            return [{"error": str(e)}]

    @mcp.tool()
    def list_versions(component: str) -> list[str]:
        """List all indexed versions for a component, sorted by semver descending."""
        try:
            conn = get_code_sqlite_conn()
            cursor = conn.cursor()
            cursor.execute(
                "SELECT DISTINCT version FROM code_files WHERE component = ?", (component,)
            )
            versions = [r["version"] for r in cursor.fetchall()]

            def semver_key(v):
                try:
                    parts = v.split(".")
                    return tuple(int(p) for p in parts[:3])
                except:
                    return (0, 0, 0)

            versions.sort(key=semver_key, reverse=True)
            return versions
        except Exception as e:
            return [f"error: {e}"]

    @mcp.tool()
    def search_code(query: str, component: str, version: str, limit: int = 10) -> list[dict]:
        """
        Semantic search over source code for a specific component@version.
        Best for natural language queries like "how to take a screenshot".
        """
        limit = min(max(1, limit), 20)
        try:
            table = get_code_lance_table()
            where_clause = f"component = '{component}' AND version = '{version}'"
            results = table.search(query).where(where_clause).limit(limit).to_list()
            return [
                {
                    "file_path": r["file_path"],
                    "content": r["text"][:2000] + "..." if len(r["text"]) > 2000 else r["text"],
                    "language": r["language"],
                    "score": round(1 - r.get("_distance", 0), 4),
                }
                for r in results
            ]
        except Exception as e:
            return [{"error": str(e)}]

    @mcp.tool()
    def search_code_fts(query: str, component: str, version: str, limit: int = 10) -> list[dict]:
        """
        Full-text search over source code for a specific component@version.
        Best for exact matches like function names, class names, keywords.
        """
        limit = min(max(1, limit), 50)
        try:
            conn = get_code_sqlite_conn()
            cursor = conn.cursor()
            cursor.execute(
                """
                SELECT file_path, snippet(code_files_fts, 0, '>>>', '<<<', '...', 64) as snippet, language
                FROM code_files_fts
                JOIN code_files f ON code_files_fts.rowid = f.id
                WHERE code_files_fts MATCH ? AND code_files_fts.component = ? AND code_files_fts.version = ?
                ORDER BY rank LIMIT ?
                """,
                (query, component, version, limit),
            )
            return [
                {"file_path": r["file_path"], "snippet": r["snippet"], "language": r["language"]}
                for r in cursor.fetchall()
            ]
        except Exception as e:
            return [{"error": str(e)}]

    @mcp.tool()
    def get_code_file_content(component: str, version: str, file_path: str) -> dict:
        """Get the full content of a specific source file at component@version."""
        try:
            conn = get_code_sqlite_conn()
            cursor = conn.cursor()
            cursor.execute(
                "SELECT content, language FROM code_files WHERE component = ? AND version = ? AND file_path = ?",
                (component, version, file_path),
            )
            row = cursor.fetchone()
            if not row:
                return {"error": f"File not found: {file_path} in {component}@{version}"}
            return {
                "content": row["content"],
                "language": row["language"],
                "lines": row["content"].count("\n") + 1,
            }
        except Exception as e:
            return {"error": str(e)}

    # Create SSE app directly - endpoints at /sse (GET) and /messages (POST)
    from starlette.middleware import Middleware

    mcp_app = mcp.http_app(
        transport="sse",
        middleware=[
            Middleware(
                CORSMiddleware,
                allow_origins=["*"],
                allow_credentials=True,
                allow_methods=["*"],
                allow_headers=["*"],
            )
        ],
    )

    return mcp_app


# =============================================================================
# Local testing functions
# =============================================================================


@app.local_entrypoint()
def main(skip_docs: bool = False, skip_code: bool = False):
    """Run initial crawl and database generation

    Args:
        skip_docs: Skip documentation crawl and indexing
        skip_code: Skip code indexing
    """
    if not skip_docs:
        print("Running initial crawl...")
        summary = crawl_docs.remote()
        print(f"Crawl summary: {summary}")

        print("Generating vector database...")
        vector_result = generate_vector_db.remote()
        print(f"Vector DB: {vector_result}")

        print("Generating SQLite database...")
        sqlite_result = generate_sqlite_db.remote()
        print(f"SQLite DB: {sqlite_result}")

    if not skip_code:
        print("Generating code index...")
        code_result = generate_code_index.remote()
        print(f"Code index: {code_result}")

    print("Done! Deploy with: modal deploy docs/scripts/modal_app.py")
