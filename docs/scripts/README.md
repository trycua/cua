# CUA Documentation Scripts

This directory contains scripts for crawling, indexing, and serving CUA documentation through a Model Context Protocol (MCP) server.

## Scripts

### Local Scripts

- **crawl_docs.py**: Crawls cua.ai/docs using crawl4ai
- **generate_db.py**: Creates LanceDB vector database for semantic search
- **generate_sqlite.py**: Creates SQLite FTS5 database for full-text search
- **mcp_server.py**: FastMCP SSE HTTP server exposing search tools

### Modal Deployment

- **modal_app.py**: Complete Modal app with scheduled crawling and MCP server deployment

## Installation

Install dependencies using uv:

```bash
# From the repository root
uv sync --group docs-scripts
```

## Usage

### Option 1: Local Development

#### 1. Crawl Documentation

```bash
uv run docs/scripts/crawl_docs.py
```

#### 2. Generate Databases

```bash
# Generate vector database for semantic search
uv run docs/scripts/generate_db.py

# Generate SQLite FTS5 database for full-text search
uv run docs/scripts/generate_sqlite.py
```

#### 3. Run MCP Server

```bash
uv run docs/scripts/mcp_server.py
```

The MCP server will be available at `http://localhost:8000` and provides:

- Semantic search over documentation
- Full-text search capabilities
- Page content retrieval

### Option 2: Modal Deployment (Production)

The Modal app provides a production-ready deployment with:

- **Scheduled daily crawling** at 6 AM UTC
- **Persistent storage** using Modal volumes
- **Scalable MCP server** with automatic database regeneration

#### Initial Setup

1. Install Modal CLI:

```bash
pip install modal
```

2. Authenticate with Modal:

```bash
modal setup
```

#### Deploy to Modal

```bash
# Initial deployment with data generation
modal run docs/scripts/modal_app.py

# Deploy the app (includes scheduled crawling + MCP server)
modal deploy docs/scripts/modal_app.py
```

#### Access the MCP Server

After deployment, Modal will provide a public URL for the MCP server:

```
https://your-username--cua-docs-mcp-web.modal.run/mcp/
```

Use this URL with the MCP Inspector or any MCP client:

```bash
npx @modelcontextprotocol/inspector
# Enter URL: https://your-username--cua-docs-mcp-web.modal.run/mcp/
# Transport: Streamable HTTP
```

#### Monitor Scheduled Crawls

View scheduled crawl runs in the Modal dashboard:

```bash
modal app show cua-docs-mcp
```

The crawler runs daily at 6 AM UTC and automatically updates the databases.
