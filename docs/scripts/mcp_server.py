"""
FastMCP SSE HTTP Server for CUA Documentation and Code
Provides semantic search (LanceDB) and full-text search (SQLite FTS5) over the documentation
and versioned source code across all git tags.
"""

import sqlite3
from pathlib import Path
from typing import Optional

import lancedb
from fastmcp import FastMCP
from lancedb.embeddings import get_registry

from code_search import CodeSearch, CodeSearchError

# Configuration - Documentation
DB_PATH = Path(__file__).parent.parent / "docs_db"
SQLITE_PATH = DB_PATH / "docs.sqlite"

# Configuration - Code Index
CODE_DB_PATH = Path(__file__).parent.parent / "code_db"
CODE_SQLITE_PATH = CODE_DB_PATH / "code_index.sqlite"
CODE_LANCEDB_PATH = CODE_DB_PATH / "code_index.lancedb"

# Initialize the embedding model (same as used for indexing)
model = get_registry().get("sentence-transformers").create(name="all-MiniLM-L6-v2")

# Create FastMCP server
mcp = FastMCP(
    name="CUA Docs",
    instructions="""CUA Documentation and Code Server - provides search and retrieval for Computer Use Agent (CUA) documentation and versioned source code.

Documentation covers:
- CUA SDK: Python library for building computer-use agents with screen capture, mouse/keyboard control
- CUA Bench: Benchmarking framework for evaluating computer-use agents
- Agent Loop: Core execution loop for autonomous agent operation
- Sandboxes: Docker and cloud VM environments for safe agent execution
- Computer interfaces: Screen, mouse, keyboard, and bash interaction APIs

=== DOCUMENTATION TOOLS ===
- search_docs: Semantic/vector search - best for conceptual queries like "how does X work?"
- search_docs_fts: Full-text search - best for exact terms, code snippets, or specific keywords
- get_page_content: Get full content of a specific page by URL
- get_page_raw_markdown: Get raw markdown with original formatting
- list_pages: Browse available documentation pages
- get_doc_categories: See all documentation categories

=== CODE SEARCH TOOLS ===
Search across all versions of CUA source code. Component + version are REQUIRED for all searches.

Discovery:
- list_components: See all indexed components (agent, computer, mcp-server, etc.)
- list_versions: See all indexed versions for a component
- list_code_files: See all source files in a component@version

Search (requires component + version):
- search_code: Semantic search - "how to take a screenshot", "authentication logic"
- search_code_fts: Full-text search - exact function names, class names, keywords

Retrieval:
- get_code_file_content: Get full source file content

Workflow example:
1. list_components() -> see available components
2. list_versions("agent") -> see versions for agent
3. search_code("ComputerAgent initialization", "agent", "0.7.3") -> find relevant files
4. get_code_file_content("agent", "0.7.3", "agent/computer_agent.py") -> get full file

IMPORTANT: After performing search queries, ALWAYS retrieve full content before answering.
Search results return snippets that may lack important context.

Always cite source URLs for docs and component@version:path for code.""",
)

# Global code search instance
_code_search: Optional[CodeSearch] = None


def get_code_search() -> CodeSearch:
    """Get or create CodeSearch instance"""
    global _code_search
    if _code_search is None:
        _code_search = CodeSearch(
            sqlite_path=CODE_SQLITE_PATH,
            lancedb_path=CODE_LANCEDB_PATH,
        )
    return _code_search

# Global database connections
_lance_db: Optional[lancedb.DBConnection] = None
_lance_table = None
_sqlite_conn: Optional[sqlite3.Connection] = None


def get_lance_table():
    """Get or create LanceDB connection"""
    global _lance_db, _lance_table
    if _lance_table is None:
        if not DB_PATH.exists():
            raise RuntimeError(
                f"Database not found at {DB_PATH}. "
                "Run generate_db.py first to create the database."
            )
        _lance_db = lancedb.connect(DB_PATH)
        _lance_table = _lance_db.open_table("docs")
    return _lance_table


def get_sqlite_conn() -> sqlite3.Connection:
    """Get or create SQLite connection"""
    global _sqlite_conn
    if _sqlite_conn is None:
        if not SQLITE_PATH.exists():
            raise RuntimeError(
                f"SQLite database not found at {SQLITE_PATH}. "
                "Run generate_sqlite.py first to create the database."
            )
        _sqlite_conn = sqlite3.connect(SQLITE_PATH)
        _sqlite_conn.row_factory = sqlite3.Row
    return _sqlite_conn


# =============================================================================
# Semantic Search Tools (LanceDB)
# =============================================================================


@mcp.tool
def search_docs(query: str, limit: int = 5, category: Optional[str] = None) -> list[dict]:
    """
    Semantic search over CUA documentation using vector embeddings.
    Best for conceptual queries like "how does the agent loop work?" or "what is a sandbox?"

    Args:
        query: Natural language search query - describe what you're looking for
        limit: Maximum number of results to return (default: 5, max: 20)
        category: Optional category filter (e.g., 'cua', 'cuabench', 'llms.txt')

    Returns:
        List of relevant documentation chunks with URLs, content, and relevance scores
    """
    limit = min(max(1, limit), 20)

    table = get_lance_table()

    # Build search query
    search = table.search(query).limit(limit)

    # Apply category filter if specified
    if category:
        # Escape single quotes to prevent injection via the category parameter
        safe_category = category.replace("'", "''")
        search = search.where(f"category = '{safe_category}'")

    results = search.to_list()

    # Format results
    formatted = []
    for r in results:
        formatted.append(
            {
                "title": r.get("title", ""),
                "url": r.get("url", ""),
                "category": r.get("category", ""),
                "content": r.get("text", ""),
                "relevance_score": round(1 - r.get("_distance", 0), 4),
            }
        )

    return formatted


# =============================================================================
# Full-Text Search Tools (SQLite FTS5)
# =============================================================================


@mcp.tool
def search_docs_fts(
    query: str, limit: int = 5, category: Optional[str] = None, highlight: bool = True
) -> list[dict]:
    """
    Full-text search over CUA documentation using SQLite FTS5.
    Best for exact keyword matches, code snippets, function names, or specific terms.

    Supports FTS5 query syntax:
    - Simple words: "install" finds pages containing "install"
    - Phrases: '"computer use agent"' finds exact phrase
    - AND/OR: "install AND docker" or "install OR setup"
    - Prefix: "config*" matches configure, configuration, etc.
    - NOT: "install NOT windows"

    Args:
        query: FTS5 search query
        limit: Maximum number of results (default: 5, max: 50)
        category: Optional category filter
        highlight: Include highlighted snippets (default: True)

    Returns:
        List of matching pages with URLs, titles, and optional snippets
    """
    limit = min(max(1, limit), 50)

    conn = get_sqlite_conn()
    cursor = conn.cursor()

    try:
        if highlight:
            sql = """
                SELECT
                    p.url,
                    p.title,
                    p.category,
                    snippet(pages_fts, 1, '**', '**', '...', 64) as snippet
                FROM pages_fts
                JOIN pages p ON pages_fts.rowid = p.id
                WHERE pages_fts MATCH ?
            """
        else:
            sql = """
                SELECT p.url, p.title, p.category
                FROM pages_fts
                JOIN pages p ON pages_fts.rowid = p.id
                WHERE pages_fts MATCH ?
            """

        params = [query]
        if category:
            sql += " AND p.category = ?"
            params.append(category)

        sql += " ORDER BY rank LIMIT ?"
        params.append(limit)

        cursor.execute(sql, params)
        rows = cursor.fetchall()

        results = []
        for row in rows:
            result = {
                "url": row["url"],
                "title": row["title"],
                "category": row["category"],
            }
            if highlight and "snippet" in row.keys():
                result["snippet"] = row["snippet"]
            results.append(result)

        return results

    except sqlite3.OperationalError as e:
        raise ValueError(f"FTS query error: {str(e)}. Try simplifying your query.")


@mcp.tool
def get_page_content(url: str) -> dict:
    """
    Get the full content of a specific documentation page.

    Args:
        url: The URL of the documentation page (exact or partial match)

    Returns:
        Full page content with title, category, and raw markdown
    """
    conn = get_sqlite_conn()
    cursor = conn.cursor()

    # Try exact match first
    cursor.execute(
        "SELECT url, title, category, content, raw_markdown FROM pages WHERE url = ?", (url,)
    )
    row = cursor.fetchone()

    # If no exact match, try partial match
    if not row:
        cursor.execute(
            "SELECT url, title, category, content, raw_markdown FROM pages WHERE url LIKE ?",
            (f"%{url}%",),
        )
        row = cursor.fetchone()

    if not row:
        raise ValueError(f"No page found matching URL: {url}")

    return {
        "url": row["url"],
        "title": row["title"],
        "category": row["category"],
        "content": row["content"],
        "has_raw_markdown": bool(row["raw_markdown"]),
    }


@mcp.tool
def get_page_raw_markdown(url: str) -> dict:
    """
    Get the raw markdown content of a documentation page.
    Useful when you need the original formatting, code blocks, etc.

    Args:
        url: The URL of the documentation page

    Returns:
        Raw markdown content of the page
    """
    conn = get_sqlite_conn()
    cursor = conn.cursor()

    cursor.execute("SELECT url, title, raw_markdown FROM pages WHERE url LIKE ?", (f"%{url}%",))
    row = cursor.fetchone()

    if not row:
        raise ValueError(f"No page found matching URL: {url}")

    return {
        "url": row["url"],
        "title": row["title"],
        "raw_markdown": row["raw_markdown"],
    }


# =============================================================================
# Discovery Tools
# =============================================================================


@mcp.tool
def get_doc_categories() -> list[dict]:
    """
    Get all available documentation categories with page counts.

    Returns:
        List of categories with their page counts
    """
    conn = get_sqlite_conn()
    cursor = conn.cursor()

    cursor.execute(
        """
        SELECT category, COUNT(*) as page_count
        FROM pages
        GROUP BY category
        ORDER BY page_count DESC
    """
    )
    rows = cursor.fetchall()

    return [{"category": row["category"], "page_count": row["page_count"]} for row in rows]


@mcp.tool
def list_pages(category: Optional[str] = None, limit: int = 50) -> list[dict]:
    """
    List documentation pages, optionally filtered by category.

    Args:
        category: Optional category to filter by
        limit: Maximum number of pages to return (default: 50)

    Returns:
        List of pages with URLs and titles
    """
    limit = min(max(1, limit), 200)

    conn = get_sqlite_conn()
    cursor = conn.cursor()

    if category:
        cursor.execute(
            "SELECT url, title, category FROM pages WHERE category = ? LIMIT ?", (category, limit)
        )
    else:
        cursor.execute("SELECT url, title, category FROM pages LIMIT ?", (limit,))

    rows = cursor.fetchall()
    return [{"url": row["url"], "title": row["title"], "category": row["category"]} for row in rows]


@mcp.tool
def search_by_url(url_pattern: str, limit: int = 10) -> list[dict]:
    """
    Search for documentation pages by URL pattern.

    Args:
        url_pattern: Partial URL to search for (e.g., 'get-started', 'reference/agent-sdk')
        limit: Maximum number of results to return

    Returns:
        List of matching documentation pages
    """
    limit = min(max(1, limit), 50)

    conn = get_sqlite_conn()
    cursor = conn.cursor()

    cursor.execute(
        "SELECT url, title, category FROM pages WHERE url LIKE ? LIMIT ?",
        (f"%{url_pattern}%", limit),
    )
    rows = cursor.fetchall()

    return [{"url": row["url"], "title": row["title"], "category": row["category"]} for row in rows]


# =============================================================================
# Code Search Tools
# =============================================================================


@mcp.tool
def list_components() -> list[dict]:
    """
    List all indexed code components with their version counts.

    Returns:
        List of components with version counts, e.g.:
        [{"component": "agent", "version_count": 45}, {"component": "computer", "version_count": 30}]
    """
    try:
        return get_code_search().list_components()
    except CodeSearchError as e:
        raise ValueError(str(e))


@mcp.tool
def list_versions(component: str) -> list[str]:
    """
    List all indexed versions for a component, sorted by semver descending (newest first).

    Args:
        component: The component name (e.g., "agent", "computer", "mcp-server")

    Returns:
        List of version strings, e.g.: ["0.7.3", "0.7.2", "0.7.1", ...]
    """
    try:
        return get_code_search().list_versions(component)
    except CodeSearchError as e:
        raise ValueError(str(e))


@mcp.tool
def list_code_files(component: str, version: str) -> list[dict]:
    """
    List all source files for a specific component@version.

    Args:
        component: The component name (e.g., "agent")
        version: The version string (e.g., "0.7.3")

    Returns:
        List of files with paths and languages, e.g.:
        [{"path": "agent/core.py", "language": "python"}, ...]
    """
    try:
        return get_code_search().list_files(component, version)
    except CodeSearchError as e:
        raise ValueError(str(e))


@mcp.tool
def search_code(query: str, component: str, version: str, limit: int = 10) -> list[dict]:
    """
    Semantic search over source code for a specific component@version.
    Uses vector embeddings to find semantically similar code.

    Best for natural language queries like:
    - "how to take a screenshot"
    - "authentication and login logic"
    - "error handling patterns"

    Args:
        query: Natural language search query
        component: The component name (REQUIRED, e.g., "agent")
        version: The version string (REQUIRED, e.g., "0.7.3")
        limit: Maximum results (default: 10, max: 20)

    Returns:
        List of matching files with content and relevance scores
    """
    limit = min(max(1, limit), 20)
    try:
        return get_code_search().search_code(query, component, version, limit)
    except CodeSearchError as e:
        raise ValueError(str(e))


@mcp.tool
def search_code_fts(
    query: str, component: str, version: str, limit: int = 10, highlight: bool = True
) -> list[dict]:
    """
    Full-text search over source code for a specific component@version.
    Uses SQLite FTS5 for fast keyword matching.

    Best for exact matches like:
    - Function names: "ComputerAgent"
    - Class names: "ScreenCapture"
    - Specific keywords: "async def screenshot"

    Supports FTS5 syntax:
    - Phrases: '"async def"'
    - AND/OR: "screenshot AND save"
    - Prefix: "Computer*"

    Args:
        query: FTS5 search query
        component: The component name (REQUIRED, e.g., "agent")
        version: The version string (REQUIRED, e.g., "0.7.3")
        limit: Maximum results (default: 10, max: 50)
        highlight: Include highlighted snippets (default: True)

    Returns:
        List of matching files with snippets and relevance scores
    """
    limit = min(max(1, limit), 50)
    try:
        return get_code_search().search_code_fts(query, component, version, limit, highlight)
    except CodeSearchError as e:
        raise ValueError(str(e))


@mcp.tool
def get_code_file_content(component: str, version: str, file_path: str) -> dict:
    """
    Get the full content of a specific source file at component@version.

    Args:
        component: The component name (e.g., "agent")
        version: The version string (e.g., "0.7.3")
        file_path: Path to the file (e.g., "agent/computer_agent.py")

    Returns:
        Full file content with language and line count
    """
    try:
        return get_code_search().get_file_content(component, version, file_path)
    except CodeSearchError as e:
        raise ValueError(str(e))


@mcp.tool
def get_code_index_stats() -> dict:
    """
    Get statistics about the code search index.

    Returns:
        Index statistics including total files, components, and versions
    """
    try:
        return get_code_search().get_stats()
    except CodeSearchError as e:
        raise ValueError(str(e))


# =============================================================================
# Resources
# =============================================================================


@mcp.resource("docs://categories")
def list_categories_resource() -> str:
    """List all documentation categories"""
    categories = get_doc_categories()
    lines = ["Documentation Categories:", ""]
    for cat in categories:
        lines.append(f"- {cat['category']}: {cat['page_count']} pages")
    return "\n".join(lines)


@mcp.resource("docs://stats")
def get_stats_resource() -> str:
    """Get documentation database statistics"""
    conn = get_sqlite_conn()
    cursor = conn.cursor()

    cursor.execute("SELECT COUNT(*) FROM pages")
    total_pages = cursor.fetchone()[0]

    cursor.execute("SELECT category, COUNT(*) FROM pages GROUP BY category")
    categories = cursor.fetchall()

    # Get LanceDB stats
    table = get_lance_table()
    total_chunks = table.count_rows()

    stats = [
        "CUA Documentation Database Statistics",
        "=" * 40,
        f"Total pages: {total_pages}",
        f"Total chunks (for semantic search): {total_chunks}",
        "",
        "Pages by category:",
    ]
    for cat, count in categories:
        stats.append(f"  - {cat}: {count}")

    return "\n".join(stats)


# =============================================================================
# Main
# =============================================================================


def main():
    """Run the MCP server"""
    import argparse

    parser = argparse.ArgumentParser(description="CUA Docs MCP Server")
    parser.add_argument("--host", default="127.0.0.1", help="Host to bind to")
    parser.add_argument("--port", type=int, default=8000, help="Port to bind to")
    parser.add_argument(
        "--transport", default="sse", choices=["sse", "http", "stdio"], help="Transport type"
    )
    args = parser.parse_args()

    print("Starting CUA Docs MCP Server...")
    print(f"Transport: {args.transport}")
    if args.transport in ["sse", "http"]:
        print(f"URL: http://{args.host}:{args.port}")

    # Verify documentation databases exist
    try:
        get_lance_table()
        print(f"Docs LanceDB loaded from: {DB_PATH}")
    except RuntimeError as e:
        print(f"Warning: {e}")

    try:
        get_sqlite_conn()
        print(f"Docs SQLite loaded from: {SQLITE_PATH}")
    except RuntimeError as e:
        print(f"Warning: {e}")

    # Verify code index databases exist
    try:
        code_search = get_code_search()
        stats = code_search.get_stats()
        print(f"Code index loaded: {stats['total_files']} files across {stats['total_components']} components")
    except (CodeSearchError, Exception) as e:
        print(f"Warning: Code index not available: {e}")

    mcp.run(transport=args.transport, host=args.host, port=args.port)


if __name__ == "__main__":
    main()
