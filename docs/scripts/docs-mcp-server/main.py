"""
CUA Documentation and Code MCP Server

A standalone MCP server that provides read-only query access to:
1. CUA documentation (crawled from cua.ai/docs)
2. Versioned source code indexed across git tags

This server is designed to run as a containerized service, with databases
mounted from external volumes or cloud storage.

Usage:
    # Run the server
    python main.py

    # Or with uvicorn
    uvicorn main:app --host 0.0.0.0 --port 8000
"""

import os
import sqlite3
import time
from pathlib import Path
from typing import Optional

import lancedb
from fastmcp import FastMCP
from lancedb.embeddings import get_registry
from starlette.middleware import Middleware
from starlette.middleware.cors import CORSMiddleware

# Configuration from environment variables
OTEL_ENDPOINT = os.environ.get("OTEL_ENDPOINT", "https://otel.cua.ai")
OTEL_SERVICE_NAME = os.environ.get("OTEL_SERVICE_NAME", "cua-docs-mcp")

# Database paths (configurable via environment)
DOCS_DB_PATH = os.environ.get("DOCS_DB_PATH", "/data/docs_db")
CODE_DB_PATH = os.environ.get("CODE_DB_PATH", "/data/code_db")

# Initialize OpenTelemetry for metrics and tracing
_tracer = None
_meter = None
_request_counter = None
_request_duration = None


def init_telemetry():
    """Initialize OpenTelemetry for metrics and tracing."""
    global _tracer, _meter, _request_counter, _request_duration

    try:
        from opentelemetry import metrics, trace
        from opentelemetry.exporter.otlp.proto.http.metric_exporter import (
            OTLPMetricExporter,
        )
        from opentelemetry.exporter.otlp.proto.http.trace_exporter import (
            OTLPSpanExporter,
        )
        from opentelemetry.sdk.metrics import MeterProvider
        from opentelemetry.sdk.metrics.export import PeriodicExportingMetricReader
        from opentelemetry.sdk.resources import Resource
        from opentelemetry.sdk.trace import TracerProvider
        from opentelemetry.sdk.trace.export import BatchSpanProcessor

        resource = Resource.create(
            {
                "service.name": OTEL_SERVICE_NAME,
                "service.version": "1.0.0",
            }
        )

        # Set up tracing
        trace_exporter = OTLPSpanExporter(endpoint=f"{OTEL_ENDPOINT}/v1/traces")
        tracer_provider = TracerProvider(resource=resource)
        tracer_provider.add_span_processor(BatchSpanProcessor(trace_exporter))
        trace.set_tracer_provider(tracer_provider)
        _tracer = trace.get_tracer(OTEL_SERVICE_NAME)

        # Set up metrics
        metric_exporter = OTLPMetricExporter(endpoint=f"{OTEL_ENDPOINT}/v1/metrics")
        metric_reader = PeriodicExportingMetricReader(metric_exporter, export_interval_millis=60000)
        meter_provider = MeterProvider(resource=resource, metric_readers=[metric_reader])
        metrics.set_meter_provider(meter_provider)
        _meter = metrics.get_meter(OTEL_SERVICE_NAME)

        # Create metrics instruments
        _request_counter = _meter.create_counter(
            name="mcp_requests_total",
            description="Total number of MCP tool requests",
            unit="1",
        )
        _request_duration = _meter.create_histogram(
            name="mcp_request_duration_seconds",
            description="Duration of MCP tool requests in seconds",
            unit="s",
        )

        print(f"OpenTelemetry initialized with endpoint: {OTEL_ENDPOINT}")
    except ImportError as e:
        print(f"OpenTelemetry packages not available: {e}")
    except Exception as e:
        print(f"Failed to initialize OpenTelemetry: {e}")


def record_request(tool_name: str, duration: float, status: str = "success"):
    """Record metrics for a tool request."""
    if _request_counter is not None:
        _request_counter.add(1, {"tool": tool_name, "status": status})
    if _request_duration is not None:
        _request_duration.record(duration, {"tool": tool_name, "status": status})


# Initialize telemetry
init_telemetry()

# Initialize the MCP server
mcp = FastMCP(
    name="CUA Docs & Code",
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

# Initialize embedding model - load eagerly to avoid cold start on first search
print("Initializing embedding model...")
model = get_registry().get("sentence-transformers").create(name="all-MiniLM-L6-v2")

# Eagerly initialize database connections at startup to reduce first-request latency
print("Initializing database connections...")

# Docs LanceDB
_docs_lance_db = None
_docs_lance_table = None
docs_db_path = Path(DOCS_DB_PATH)
if docs_db_path.exists():
    try:
        _docs_lance_db = lancedb.connect(docs_db_path)
        _docs_lance_table = _docs_lance_db.open_table("docs")
        print(f"  Docs LanceDB loaded from {docs_db_path}")
    except Exception as e:
        print(f"  Warning: Could not load docs LanceDB: {e}")

# Docs SQLite
_docs_sqlite_conn = None
sqlite_path = Path(DOCS_DB_PATH) / "docs.sqlite"
if sqlite_path.exists():
    try:
        _docs_sqlite_conn = sqlite3.connect(f"file:{sqlite_path}?mode=ro", uri=True)
        _docs_sqlite_conn.row_factory = sqlite3.Row
        print(f"  Docs SQLite loaded from {sqlite_path}")
    except Exception as e:
        print(f"  Warning: Could not load docs SQLite: {e}")

# Code LanceDB
_code_lance_db = None
_code_lance_table = None
code_lance_path = Path(CODE_DB_PATH) / "code_index.lancedb"
if code_lance_path.exists():
    try:
        _code_lance_db = lancedb.connect(code_lance_path)
        _code_lance_table = _code_lance_db.open_table("code")
        print(f"  Code LanceDB loaded from {code_lance_path}")
    except Exception as e:
        print(f"  Warning: Could not load code LanceDB: {e}")

# Code SQLite
_code_sqlite_conn = None
code_sqlite_path = Path(CODE_DB_PATH) / "code_index.sqlite"
if code_sqlite_path.exists():
    try:
        _code_sqlite_conn = sqlite3.connect(f"file:{code_sqlite_path}?mode=ro", uri=True)
        _code_sqlite_conn.row_factory = sqlite3.Row
        print(f"  Code SQLite loaded from {code_sqlite_path}")
    except Exception as e:
        print(f"  Warning: Could not load code SQLite: {e}")

print("Database initialization complete.")


def get_lance_table():
    """Get LanceDB connection for docs (eagerly loaded)"""
    if _docs_lance_table is None:
        raise RuntimeError(
            "Database not found. Ensure the docs database is mounted at DOCS_DB_PATH."
        )
    return _docs_lance_table


def get_sqlite_conn():
    """Get read-only SQLite connection for docs (eagerly loaded)"""
    if _docs_sqlite_conn is None:
        raise RuntimeError(
            "SQLite database not found. Ensure docs.sqlite is present in DOCS_DB_PATH."
        )
    return _docs_sqlite_conn


def get_code_lance_table():
    """Get LanceDB connection for the aggregated code database (eagerly loaded)."""
    if _code_lance_table is None:
        raise RuntimeError(
            "Code LanceDB not found. Ensure code_index.lancedb is present in CODE_DB_PATH."
        )
    return _code_lance_table


def get_code_sqlite_conn():
    """Get read-only SQLite connection for the aggregated code database (eagerly loaded)."""
    if _code_sqlite_conn is None:
        raise RuntimeError(
            "Code SQLite database not found. Ensure code_index.sqlite is present in CODE_DB_PATH."
        )
    return _code_sqlite_conn


# =================== DOCUMENTATION QUERY TOOLS (READ-ONLY) ===================


@mcp.tool()
def query_docs_db(sql: str) -> list[dict]:
    """
    Execute a SQL query against the documentation database.
    The database is READ-ONLY.

    Database Schema:

    Table: pages
    - id INTEGER PRIMARY KEY AUTOINCREMENT
    - url TEXT NOT NULL UNIQUE         -- Full URL of the documentation page
    - title TEXT NOT NULL              -- Page title
    - category TEXT NOT NULL           -- Category (e.g., 'cua', 'cuabench', 'llms.txt')
    - content TEXT NOT NULL            -- Plain text content (markdown stripped)

    Virtual Table: pages_fts (FTS5 full-text search)
    - content TEXT                     -- Full-text indexed content
    - url TEXT UNINDEXED
    - title TEXT UNINDEXED
    - category TEXT UNINDEXED

    Example queries:

    1. List all pages: SELECT url, title, category FROM pages ORDER BY category, title

    2. Full-text search with snippets:
       SELECT p.url, p.title, snippet(pages_fts, 0, '>>>', '<<<', '...', 64) as snippet
       FROM pages_fts JOIN pages p ON pages_fts.rowid = p.id
       WHERE pages_fts MATCH 'agent loop' ORDER BY rank LIMIT 10

    3. Get page content: SELECT url, title, content FROM pages WHERE url LIKE '%quickstart%'

    Args:
        sql: SQL query to execute

    Returns:
        List of dictionaries, one per row, with column names as keys
    """
    start_time = time.perf_counter()
    status = "success"
    try:
        conn = get_sqlite_conn()
        cursor = conn.cursor()
        cursor.execute(sql)
        return [dict(row) for row in cursor.fetchall()]
    except Exception:
        status = "error"
        raise
    finally:
        record_request("query_docs_db", time.perf_counter() - start_time, status)


@mcp.tool()
def query_docs_vectors(
    query: str,
    limit: int = 10,
    where: Optional[str] = None,
    select: Optional[list[str]] = None,
) -> list[dict]:
    """
    Execute a vector similarity search against the documentation LanceDB (read-only).

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
        where: Optional SQL-like filter (e.g., "category = 'cua'")
        select: Optional list of columns to return (default: all except vector)

    Returns:
        List of matching documents with similarity scores (_distance field)
    """
    start_time = time.perf_counter()
    status = "success"
    try:
        limit = min(max(1, limit), 100)
        table = get_lance_table()

        search = table.search(query).limit(limit)

        if where:
            search = search.where(where)
        if select:
            search = search.select(select)

        results = search.to_list()

        formatted = []
        for r in results:
            result = {}
            for key, value in r.items():
                if key == "vector":
                    continue
                result[key] = value
            formatted.append(result)

        return formatted
    except Exception:
        status = "error"
        raise
    finally:
        record_request("query_docs_vectors", time.perf_counter() - start_time, status)


# =================== CODE QUERY TOOLS (READ-ONLY) ===================


@mcp.tool()
def query_code_db(sql: str) -> list[dict]:
    """
    Execute a SQL query against the code search database.
    The database is READ-ONLY.

    Database Schema:

    Table: code_files
    - id INTEGER PRIMARY KEY AUTOINCREMENT
    - component TEXT NOT NULL          -- Component name (e.g., "agent", "computer")
    - version TEXT NOT NULL            -- Version string (e.g., "0.7.3")
    - file_path TEXT NOT NULL          -- Path to file
    - content TEXT NOT NULL            -- Full source code content
    - language TEXT NOT NULL           -- Programming language
    - UNIQUE(component, version, file_path)

    Virtual Table: code_files_fts (FTS5 full-text search)
    - content TEXT                     -- Full-text indexed content
    - component TEXT UNINDEXED
    - version TEXT UNINDEXED
    - file_path TEXT UNINDEXED

    Example queries:

    1. List components: SELECT component, COUNT(DISTINCT version) as version_count
       FROM code_files GROUP BY component ORDER BY component

    2. List versions: SELECT DISTINCT version FROM code_files
       WHERE component = 'agent' ORDER BY version DESC

    3. Full-text search:
       SELECT f.component, f.version, f.file_path,
              snippet(code_files_fts, 0, '>>>', '<<<', '...', 64) as snippet
       FROM code_files_fts JOIN code_files f ON code_files_fts.rowid = f.id
       WHERE code_files_fts MATCH 'ComputerAgent' ORDER BY rank LIMIT 10

    4. Get file content: SELECT content, language FROM code_files
       WHERE component = 'agent' AND version = '0.7.3' AND file_path = 'agent/core.py'

    Args:
        sql: SQL query to execute

    Returns:
        List of dictionaries, one per row, with column names as keys
    """
    start_time = time.perf_counter()
    status = "success"
    try:
        conn = get_code_sqlite_conn()
        cursor = conn.cursor()
        cursor.execute(sql)
        return [dict(row) for row in cursor.fetchall()]
    except Exception:
        status = "error"
        raise
    finally:
        record_request("query_code_db", time.perf_counter() - start_time, status)


@mcp.tool()
def query_code_vectors(
    query: str,
    limit: int = 10,
    where: Optional[str] = None,
    select: Optional[list[str]] = None,
    component: Optional[str] = None,
) -> list[dict]:
    """
    Execute a vector similarity search against the code LanceDB (read-only).

    Schema:
    - text TEXT           -- The source code content
    - vector VECTOR       -- Embedding vector (all-MiniLM-L6-v2, 384 dimensions)
    - component TEXT      -- Component name (e.g., "agent", "computer")
    - version TEXT        -- Version string (e.g., "0.7.3")
    - file_path TEXT      -- Path to file within the component
    - language TEXT       -- Programming language

    Args:
        query: Natural language query to embed and search for
        limit: Maximum number of results (default: 10, max: 100)
        where: Optional SQL-like filter (e.g., "version = '0.7.3'")
        select: Optional list of columns to return (default: all except vector)
        component: Optional component to filter by (if not specified, searches all)

    Returns:
        List of matching code files with similarity scores (_distance field)
    """
    start_time = time.perf_counter()
    status = "success"
    try:
        limit = min(max(1, limit), 100)
        table = get_code_lance_table()

        search = table.search(query).limit(limit)

        # Build where clause, adding component filter if specified
        where_clauses = []
        if component:
            where_clauses.append(f"component = '{component}'")
        if where:
            where_clauses.append(where)

        if where_clauses:
            search = search.where(" AND ".join(where_clauses))
        if select:
            search = search.select(select)

        results = search.to_list()

        formatted = []
        for r in results:
            result = {}
            for key, value in r.items():
                if key == "vector":
                    continue
                result[key] = value
            formatted.append(result)

        return formatted
    except Exception:
        status = "error"
        raise
    finally:
        record_request("query_code_vectors", time.perf_counter() - start_time, status)


# Create the ASGI app
app = mcp.http_app(
    transport="streamable-http",
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

if __name__ == "__main__":
    import uvicorn

    port = int(os.environ.get("PORT", "8000"))
    host = os.environ.get("HOST", "0.0.0.0")

    print(f"Starting MCP server on {host}:{port}")
    uvicorn.run(app, host=host, port=port)
