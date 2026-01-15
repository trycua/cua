# CUA Documentation Scripts

This directory contains scripts for crawling, indexing, and serving CUA documentation through a Model Context Protocol (MCP) server.

## Scripts

### Local Scripts

- **crawl_docs.py**: Crawls cua.ai/docs using crawl4ai
- **generate_db.py**: Creates LanceDB vector database for semantic search
- **generate_sqlite.py**: Creates SQLite FTS5 database for full-text search

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

## Code Indexing

The Modal app also indexes the CUA source code across all git tags, enabling semantic and full-text search over versioned code.

### Architecture

Code indexing uses **parallel sharded processing** for performance:

```
┌─────────────────────────────────────────────────────────────────┐
│                  generate_code_index_parallel()                  │
│                                                                 │
│  1. Clone/fetch git repository                                  │
│  2. Get all tags (e.g., agent-v0.7.3, computer-v0.5.0)         │
│  3. Group tags by component                                     │
│  4. Dispatch parallel workers via Modal starmap                 │
└─────────────────────────────────────────────────────────────────┘
                              │
           ┌──────────────────┼──────────────────┐
           ▼                  ▼                  ▼
    ┌─────────────┐    ┌─────────────┐    ┌─────────────┐
    │ index_      │    │ index_      │    │ index_      │
    │ component   │    │ component   │    │ component   │
    │ (agent)     │    │ (computer)  │    │ (lume)      │
    │             │    │             │    │             │
    │ 112 tags    │    │ 58 tags     │    │ 49 tags     │
    └─────────────┘    └─────────────┘    └─────────────┘
           │                  │                  │
           ▼                  ▼                  ▼
    ┌─────────────┐    ┌─────────────┐    ┌─────────────┐
    │ SQLite +    │    │ SQLite +    │    │ SQLite +    │
    │ LanceDB     │    │ LanceDB     │    │ LanceDB     │
    │ (agent)     │    │ (computer)  │    │ (lume)      │
    └─────────────┘    └─────────────┘    └─────────────┘
```

Each component gets its own databases:

- `code_index_{component}.sqlite` - FTS5 full-text search
- `code_index_{component}.lancedb/` - Vector embeddings for semantic search

### Running Code Indexing

```bash
# Run parallel code indexing (default)
modal run docs/scripts/modal_app.py --code-only

# Run in detached mode to monitor via dashboard
modal run --detach docs/scripts/modal_app.py --code-only

# Run sequential (legacy) mode
modal run docs/scripts/modal_app.py --code-only --no-parallel

# Skip code indexing, only crawl docs
modal run docs/scripts/modal_app.py --skip-code
```

### MCP Server: Querying Sharded Databases

The MCP server automatically discovers and queries across all component databases:

**SQLite Queries** - Uses `ATTACH DATABASE` to create a unified view:

```sql
-- This queries across ALL component databases
SELECT component, version, file_path
FROM code_files
WHERE component = 'agent' AND version = '0.7.3'

-- Full-text search across all components
SELECT * FROM code_files_fts
WHERE code_files_fts MATCH 'ComputerAgent'
```

**Vector Search** - Queries all LanceDBs and merges results by similarity:

```python
# Searches all component databases, returns top results
query_code_vectors("screenshot capture implementation", limit=10)

# Search specific component only
query_code_vectors("agent loop", component="agent", limit=10)
```

### Database Schema

**SQLite Table: code_files**
| Column | Type | Description |
|--------|------|-------------|
| id | INTEGER | Primary key |
| component | TEXT | Component name (agent, computer, etc.) |
| version | TEXT | Version string (0.7.3) |
| file_path | TEXT | Path within repository |
| content | TEXT | Full source code |
| language | TEXT | python, typescript, javascript |

**LanceDB Schema: code**
| Column | Type | Description |
|--------|------|-------------|
| text | TEXT | Source code (embedded) |
| vector | VECTOR(384) | all-MiniLM-L6-v2 embedding |
| component | TEXT | Component name |
| version | TEXT | Version string |
| file_path | TEXT | Path within repository |
| language | TEXT | Programming language |

### Size Limits

- **SQLite**: Files up to 1MB are indexed
- **LanceDB**: Files up to 100KB are embedded (larger files skip embedding)
- **File types**: `.py`, `.ts`, `.tsx`, `.js`

### Scheduled Indexing

Code indexing runs daily at 5 AM UTC (before docs crawl at 6 AM):

```bash
# View scheduled runs
modal app show cua-docs-mcp
```
