# CUA Documentation Scripts

This directory contains scripts for crawling, indexing, and serving CUA documentation through a Model Context Protocol (MCP) server.

## Scripts

- **crawl_docs.py**: Crawls cua.ai/docs using crawl4ai
- **generate_db.py**: Creates LanceDB vector database for semantic search
- **generate_sqlite.py**: Creates SQLite FTS5 database for full-text search
- **mcp_server.py**: FastMCP SSE HTTP server exposing search tools

## Installation

Install dependencies using uv:

```bash
# From the repository root
uv sync --group docs-scripts
```

## Usage

### 1. Crawl Documentation

```bash
uv run docs/scripts/crawl_docs.py
```

### 2. Generate Databases

```bash
# Generate vector database for semantic search
uv run docs/scripts/generate_db.py

# Generate SQLite FTS5 database for full-text search
uv run docs/scripts/generate_sqlite.py
```

### 3. Run MCP Server

```bash
uv run docs/scripts/mcp_server.py
```

The MCP server will be available at `http://localhost:8000` and provides:
- Semantic search over documentation
- Full-text search capabilities
- Page content retrieval
