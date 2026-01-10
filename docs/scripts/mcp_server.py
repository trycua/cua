"""
FastMCP SSE HTTP Server for CUA Documentation and Code
Provides direct read-only query access to documentation and versioned source code databases.

Available databases:
- Documentation SQLite (docs.sqlite): Full-text search over documentation pages
- Documentation LanceDB (docs.lance): Semantic vector search over documentation
- Code SQLite (code_index.sqlite): Full-text search over versioned source code
- Code LanceDB (code_index.lancedb): Semantic vector search over versioned source code
"""

import sqlite3
from pathlib import Path
from typing import Optional

import lancedb
from fastmcp import FastMCP
from lancedb.embeddings import get_registry

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
    instructions="""CUA Documentation and Code Server - provides direct read-only query access to Computer Use Agent (CUA) documentation and versioned source code.

=== AVAILABLE TOOLS ===

Documentation:
- query_docs_db: Execute SQL queries against the documentation SQLite database
- query_docs_vectors: Execute vector similarity searches against the documentation LanceDB

Code:
- query_code_db: Execute SQL queries against the code search SQLite database
- query_code_vectors: Execute vector similarity searches against the code LanceDB

All tools are READ-ONLY. Only SELECT queries are allowed for SQL databases.

=== DOCUMENTATION DATABASE ===

The documentation database contains crawled pages from cua.ai/docs covering:
- CUA SDK: Python library for building computer-use agents
- CUA Bench: Benchmarking framework for evaluating computer-use agents
- Agent Loop: Core execution loop for autonomous agent operation
- Sandboxes: Docker and cloud VM environments for safe agent execution
- Computer interfaces: Screen, mouse, keyboard, and bash interaction APIs

=== CODE DATABASE ===

The code database contains versioned source code indexed across all git tags.
Components include: agent, computer, mcp-server, som, etc.

=== WORKFLOW EXAMPLES ===

1. Find documentation about a topic:
   - Use query_docs_vectors with a natural language query for semantic search
   - Use query_docs_db with FTS5 MATCH for keyword search

2. Explore code across versions:
   - List components: SELECT component, COUNT(DISTINCT version) FROM code_files GROUP BY component
   - Search code: Use query_code_db with FTS5 on code_files_fts
   - Get file content: SELECT content FROM code_files WHERE component='agent' AND version='0.7.3' AND file_path='...'

3. Semantic code search:
   - Use query_code_vectors with natural language queries like "screenshot capture implementation"

IMPORTANT: Always cite sources - URLs for docs, component@version:path for code.""",
)

# Global database connections
_lance_db: Optional[lancedb.DBConnection] = None
_lance_table = None
_sqlite_conn: Optional[sqlite3.Connection] = None
_code_lance_db: Optional[lancedb.DBConnection] = None
_code_lance_table = None


def get_lance_table():
    """Get or create documentation LanceDB connection"""
    global _lance_db, _lance_table
    if _lance_table is None:
        if not DB_PATH.exists():
            raise RuntimeError(
                f"Database not found at {DB_PATH}. Run generate_db.py first to create the database."
            )
        _lance_db = lancedb.connect(DB_PATH)
        _lance_table = _lance_db.open_table("docs")
    return _lance_table


def get_code_lance_table():
    """Get or create code LanceDB connection"""
    global _code_lance_db, _code_lance_table
    if _code_lance_table is None:
        if not CODE_LANCEDB_PATH.exists():
            raise RuntimeError(
                f"Code LanceDB not found at {CODE_LANCEDB_PATH}. "
                "Run 'modal run docs/scripts/modal_app.py::generate_code_index' first to create the database."
            )
        _code_lance_db = lancedb.connect(CODE_LANCEDB_PATH)
        _code_lance_table = _code_lance_db.open_table("code")
    return _code_lance_table


def get_sqlite_conn() -> sqlite3.Connection:
    """Get or create documentation SQLite connection (read-only)"""
    global _sqlite_conn
    if _sqlite_conn is None:
        if not SQLITE_PATH.exists():
            raise RuntimeError(
                f"SQLite database not found at {SQLITE_PATH}. "
                "Run generate_sqlite.py first to create the database."
            )
        _sqlite_conn = sqlite3.connect(f"file:{SQLITE_PATH}?mode=ro", uri=True)
        _sqlite_conn.row_factory = sqlite3.Row
    return _sqlite_conn


# =============================================================================
# Documentation Query Tools (Read-Only)
# =============================================================================


@mcp.tool
def query_docs_db(sql: str) -> list[dict]:
    """
    Execute a SQL query against the documentation database.
    The database is READ-ONLY.

    Database Schema:

    Table: pages
    - id INTEGER PRIMARY KEY AUTOINCREMENT
    - url TEXT NOT NULL UNIQUE         -- Full URL of the documentation page
    - title TEXT NOT NULL              -- Page title
    - description TEXT                 -- Page description/summary
    - category TEXT NOT NULL           -- Category (e.g., 'cua', 'cuabench', 'llms.txt')
    - subcategory TEXT                 -- Subcategory within the main category
    - page_name TEXT                   -- Short page name
    - content TEXT NOT NULL            -- Plain text content (markdown stripped)
    - raw_markdown TEXT                -- Original markdown content

    Virtual Table: pages_fts (FTS5 full-text search)
    - title TEXT                       -- Full-text indexed title
    - content TEXT                     -- Full-text indexed content

    Example queries:

    1. List all pages with categories:
       SELECT url, title, category FROM pages ORDER BY category, title

    2. Get page count by category:
       SELECT category, COUNT(*) as count FROM pages GROUP BY category

    3. Full-text search with snippets:
       SELECT p.url, p.title, snippet(pages_fts, 1, '>>>', '<<<', '...', 64) as snippet
       FROM pages_fts
       JOIN pages p ON pages_fts.rowid = p.id
       WHERE pages_fts MATCH 'agent loop'
       ORDER BY rank
       LIMIT 10

    4. Get full page content:
       SELECT url, title, content, raw_markdown FROM pages WHERE url LIKE '%quickstart%'

    5. Search in specific category:
       SELECT p.url, p.title
       FROM pages_fts
       JOIN pages p ON pages_fts.rowid = p.id
       WHERE pages_fts MATCH 'sandbox'
         AND p.category = 'cua'

    6. FTS5 query syntax examples:
       - Simple words: 'install' finds pages containing 'install'
       - Phrases: '"computer use agent"' finds exact phrase
       - AND/OR: 'install AND docker' or 'install OR setup'
       - Prefix: 'config*' matches configure, configuration, etc.
       - NOT: 'install NOT windows'

    Args:
        sql: SQL query to execute

    Returns:
        List of dictionaries, one per row, with column names as keys
    """
    conn = get_sqlite_conn()
    cursor = conn.cursor()
    cursor.execute(sql)
    return [dict(row) for row in cursor.fetchall()]


@mcp.tool
def query_docs_vectors(
    query: str,
    limit: int = 10,
    where: Optional[str] = None,
    select: Optional[list[str]] = None,
) -> list[dict]:
    """
    Execute a vector similarity search against the documentation LanceDB (read-only).

    This provides direct access to the vector database for semantic search.
    The query is embedded using all-MiniLM-L6-v2 and compared against document embeddings.

    Schema:
    - text TEXT           -- The document chunk text
    - vector VECTOR       -- Embedding vector (all-MiniLM-L6-v2, 384 dimensions)
    - url TEXT            -- Source URL
    - title TEXT          -- Document title
    - category TEXT       -- Category (e.g., 'cua', 'cuabench')
    - chunk_index INT     -- Index of chunk within document

    Args:
        query: Natural language query to embed and search for
        limit: Maximum number of results (default: 10, max: 100)
        where: Optional SQL-like filter (e.g., "category = 'cua'", "title LIKE '%agent%'")
        select: Optional list of columns to return (default: all columns except vector)

    Returns:
        List of matching documents with similarity scores (_distance field,
        lower is more similar)

    Example usage:
    - query="how to capture screenshots", limit=5
    - query="agent loop architecture", where="category = 'cua'"
    - query="benchmarking evaluation", select=["url", "title", "text"]
    """
    limit = min(max(1, limit), 100)

    table = get_lance_table()

    # Build search query
    search = table.search(query).limit(limit)

    # Apply where filter if specified
    if where:
        search = search.where(where)

    # Apply column selection if specified
    if select:
        search = search.select(select)

    results = search.to_list()

    # Format results - include all fields except vector
    formatted = []
    for r in results:
        result = {}
        for key, value in r.items():
            # Skip the vector field as it's large and not useful to return
            if key == "vector":
                continue
            result[key] = value
        formatted.append(result)

    return formatted


# =============================================================================
# Code Query Tools (Read-Only)
# =============================================================================


@mcp.tool
def query_code_db(sql: str) -> list[dict]:
    """
    Execute a SQL query against the code search database.
    The database is READ-ONLY.

    Database Schema:

    Table: code_files
    - id INTEGER PRIMARY KEY AUTOINCREMENT
    - component TEXT NOT NULL          -- Component name (e.g., "agent", "computer", "mcp-server")
    - version TEXT NOT NULL            -- Version string (e.g., "0.7.3", "0.7.2")
    - file_path TEXT NOT NULL          -- Path to file (e.g., "agent/computer_agent.py")
    - content TEXT NOT NULL            -- Full source code content
    - language TEXT NOT NULL           -- Programming language (e.g., "python", "typescript")
    - UNIQUE(component, version, file_path)

    Indexes:
    - idx_component ON code_files(component)
    - idx_version ON code_files(component, version)

    Virtual Table: code_files_fts (FTS5 full-text search)
    - content TEXT                     -- Full-text indexed content
    - component TEXT UNINDEXED         -- Component name for filtering
    - version TEXT UNINDEXED           -- Version for filtering
    - file_path TEXT UNINDEXED         -- File path for reference

    Example queries:

    1. List all components with version counts:
       SELECT component, COUNT(DISTINCT version) as version_count
       FROM code_files
       GROUP BY component
       ORDER BY component

    2. List versions for a component:
       SELECT DISTINCT version
       FROM code_files
       WHERE component = 'agent'
       ORDER BY version DESC

    3. List files in a component@version:
       SELECT file_path, language
       FROM code_files
       WHERE component = 'agent' AND version = '0.7.3'

    4. Get file content:
       SELECT content, language
       FROM code_files
       WHERE component = 'agent' AND version = '0.7.3' AND file_path = 'agent/core.py'

    5. Full-text search (FTS5):
       SELECT f.component, f.version, f.file_path, f.language,
              snippet(code_files_fts, 0, '>>>', '<<<', '...', 64) as snippet
       FROM code_files_fts
       JOIN code_files f ON code_files_fts.rowid = f.id
       WHERE code_files_fts MATCH 'ComputerAgent'
         AND code_files_fts.component = 'agent'
       ORDER BY rank
       LIMIT 10

    6. Search for function definitions:
       SELECT f.component, f.version, f.file_path,
              snippet(code_files_fts, 0, '>>>', '<<<', '...', 100) as snippet
       FROM code_files_fts
       JOIN code_files f ON code_files_fts.rowid = f.id
       WHERE code_files_fts MATCH '"def screenshot" OR "async def screenshot"'
       ORDER BY rank
       LIMIT 10

    7. Search across all versions of a component:
       SELECT f.version, f.file_path,
              snippet(code_files_fts, 0, '>>>', '<<<', '...', 64) as snippet
       FROM code_files_fts
       JOIN code_files f ON code_files_fts.rowid = f.id
       WHERE code_files_fts MATCH 'error handling'
         AND code_files_fts.component = 'agent'
       ORDER BY f.version DESC, rank
       LIMIT 20

    Args:
        sql: SQL query to execute

    Returns:
        List of dictionaries, one per row, with column names as keys
    """
    conn = sqlite3.connect(f"file:{CODE_SQLITE_PATH}?mode=ro", uri=True)
    conn.row_factory = sqlite3.Row
    cursor = conn.cursor()
    cursor.execute(sql)
    results = [dict(row) for row in cursor.fetchall()]
    conn.close()
    return results


@mcp.tool
def query_code_vectors(
    query: str,
    limit: int = 10,
    where: Optional[str] = None,
    select: Optional[list[str]] = None,
) -> list[dict]:
    """
    Execute a vector similarity search against the code LanceDB (read-only).

    This provides direct access to the vector database for semantic code search.
    The query is embedded using all-MiniLM-L6-v2 and compared against code embeddings.

    Schema:
    - text TEXT           -- The source code content
    - vector VECTOR       -- Embedding vector (all-MiniLM-L6-v2, 384 dimensions)
    - component TEXT      -- Component name (e.g., "agent", "computer")
    - version TEXT        -- Version string (e.g., "0.7.3")
    - file_path TEXT      -- Path to file within the component
    - language TEXT       -- Programming language (e.g., "python", "typescript")

    Args:
        query: Natural language query to embed and search for
               (e.g., "screenshot capture implementation", "error handling in agent loop")
        limit: Maximum number of results (default: 10, max: 100)
        where: Optional SQL-like filter (e.g., "component = 'agent'", "version = '0.7.3'",
               "component = 'agent' AND version = '0.7.3'")
        select: Optional list of columns to return (default: all columns except vector)

    Returns:
        List of matching code files with similarity scores (_distance field,
        lower is more similar)

    Example usage:
    - query="mouse click implementation", limit=5
    - query="screenshot capture", where="component = 'computer'"
    - query="agent loop error handling", where="component = 'agent' AND version = '0.7.3'"
    - query="async task execution", select=["component", "version", "file_path", "text"]
    """
    limit = min(max(1, limit), 100)

    table = get_code_lance_table()

    # Build search query
    search = table.search(query).limit(limit)

    # Apply where filter if specified
    if where:
        search = search.where(where)

    # Apply column selection if specified
    if select:
        search = search.select(select)

    results = search.to_list()

    # Format results - include all fields except vector
    formatted = []
    for r in results:
        result = {}
        for key, value in r.items():
            # Skip the vector field as it's large and not useful to return
            if key == "vector":
                continue
            result[key] = value
        formatted.append(result)

    return formatted


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
        if CODE_SQLITE_PATH.exists():
            conn = sqlite3.connect(CODE_SQLITE_PATH)
            cursor = conn.cursor()
            cursor.execute("SELECT COUNT(*) FROM code_files")
            total_files = cursor.fetchone()[0]
            cursor.execute("SELECT COUNT(DISTINCT component) FROM code_files")
            total_components = cursor.fetchone()[0]
            conn.close()
            print(f"Code SQLite loaded: {total_files} files across {total_components} components")
        else:
            print(f"Warning: Code SQLite not found at {CODE_SQLITE_PATH}")
    except Exception as e:
        print(f"Warning: Code SQLite not available: {e}")

    try:
        get_code_lance_table()
        print(f"Code LanceDB loaded from: {CODE_LANCEDB_PATH}")
    except RuntimeError as e:
        print(f"Warning: {e}")

    mcp.run(transport=args.transport, host=args.host, port=args.port)


if __name__ == "__main__":
    main()
