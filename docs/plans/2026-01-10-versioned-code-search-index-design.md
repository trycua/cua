# Versioned Code Search Index Design

## Overview

A per-component search index across all git tags, exposed via the docs MCP server to help documentation agents answer questions and vibe coding agents build implementations.

## Requirements

- **Version coverage**: All 265 tags across 13 components
- **Search capabilities**: Both text (FTS) + semantic (embeddings)
- **Query model**: Component + version required for all searches
- **Infrastructure**: Add tools to existing docs MCP server, use separate Modal volume
- **Chunking**: File-level (each file is one document)
- **File scope**: Source code only (.py, .ts, .js, .tsx)

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    Daily Modal Job                          │
├─────────────────────────────────────────────────────────────┤
│  1. Clone/pull cua repo                                     │
│  2. For each of 265 tags:                                   │
│     - git checkout <tag>                                    │
│     - Extract source files (.py, .ts, .js, .tsx)           │
│     - Parse component + version from tag name               │
│  3. Build indexes:                                          │
│     - LanceDB: file embeddings + metadata                   │
│     - SQLite FTS5: full-text search                         │
│  4. Write to Modal volume: cua-code-index                   │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│              Existing Docs MCP Server                       │
├─────────────────────────────────────────────────────────────┤
│  Existing tools:                                            │
│    - search_docs, search_docs_fts, get_page_content, etc.  │
│                                                             │
│  NEW tools:                                                 │
│    - search_code(query, component, version)        # semantic│
│    - search_code_fts(query, component, version)    # text   │
│    - get_file_content(component, version, path)             │
│    - list_components()                                      │
│    - list_versions(component)                               │
│    - list_files(component, version)                         │
└─────────────────────────────────────────────────────────────┘
```

## Data Model

### Tag Parsing

```
agent-v0.7.3       → component: "agent",          version: "0.7.3"
computer-v0.5.1    → component: "computer",       version: "0.5.1"
mcp-server-v0.1.15 → component: "mcp-server",     version: "0.1.15"
v0.1.13            → component: "cua",            version: "0.1.13"  (root package)
```

### SQLite Schema (Full-Text Search)

```sql
CREATE TABLE code_files (
    id INTEGER PRIMARY KEY,
    component TEXT NOT NULL,
    version TEXT NOT NULL,
    file_path TEXT NOT NULL,
    content TEXT NOT NULL,
    language TEXT NOT NULL,
    UNIQUE(component, version, file_path)
);

CREATE VIRTUAL TABLE code_files_fts USING fts5(
    content,
    component UNINDEXED,
    version UNINDEXED,
    file_path UNINDEXED,
    content='code_files'
);
```

### LanceDB Schema (Semantic Search)

```python
{
    "text": str,           # file content
    "vector": list[float], # embedding (384-dim, all-MiniLM-L6-v2)
    "component": str,
    "version": str,
    "file_path": str,
    "language": str,
}
```

### Estimated Size

- ~20,000-50,000 file records (265 tags × ~50-200 source files each)
- SQLite: ~500MB-1GB
- LanceDB: ~2-4GB (with embeddings)

## MCP Tool Interface

### Discovery Tools

```python
@mcp.tool()
def list_components() -> list[dict]:
    """List all indexed components with their version counts.
    Returns: [{"component": "agent", "version_count": 45}, ...]
    """

@mcp.tool()
def list_versions(component: str) -> list[str]:
    """List all indexed versions for a component, sorted semver descending.
    Returns: ["0.7.3", "0.7.2", "0.7.1", ...]
    """

@mcp.tool()
def list_files(component: str, version: str) -> list[dict]:
    """List all source files for a component@version.
    Returns: [{"path": "agent/core.py", "language": "python"}, ...]
    """
```

### Search Tools

```python
@mcp.tool()
def search_code(query: str, component: str, version: str, limit: int = 10) -> list[dict]:
    """Semantic search over source code for a specific component@version.
    Returns: [{"file_path": "...", "content": "...", "score": 0.85}, ...]
    """

@mcp.tool()
def search_code_fts(query: str, component: str, version: str, limit: int = 10) -> list[dict]:
    """Full-text search over source code for a specific component@version.
    Returns: [{"file_path": "...", "snippet": "...", "score": 12.5}, ...]
    """
```

### Retrieval Tool

```python
@mcp.tool()
def get_file_content(component: str, version: str, file_path: str) -> dict:
    """Get full content of a specific file at component@version.
    Returns: {"content": "...", "language": "python", "lines": 245}
    """
```

## Indexing Pipeline

```python
def index_all_tags():
    repo = clone_or_pull("https://github.com/anthropics/cua.git")
    tags = get_all_tags(repo)

    db = init_sqlite("code_index.sqlite")
    lance = init_lancedb("code_index.lancedb")
    embedder = SentenceTransformer("all-MiniLM-L6-v2")

    for tag in tags:
        component, version = parse_tag(tag)
        checkout(repo, tag)
        source_files = glob(repo, ["**/*.py", "**/*.ts", "**/*.js", "**/*.tsx"])

        for file_path in source_files:
            content = read_file(file_path)
            language = detect_language(file_path)

            # SQLite (full-text)
            db.insert(component, version, file_path, content, language)

            # LanceDB (semantic) - skip very large files
            if len(content) < 100_000:
                vector = embedder.encode(content)
                lance.insert(text=content, vector=vector,
                           component=component, version=version,
                           file_path=file_path, language=language)

    db.commit()
    lance.compact()
```

### Incremental Updates

- Track indexed tags in metadata table
- Only process new tags on daily runs
- Full reindex weekly

## Modal Deployment

### Volume Structure

```
cua-docs-data (existing):     cua-code-index (new):
├── crawled_data/             ├── repo/
└── docs_db/                  ├── code_index.sqlite
    ├── docs.lancedb/         ├── code_index.lancedb/
    └── docs.sqlite           └── metadata.json
```

### Updated modal_app.py

```python
code_volume = modal.Volume.from_name("cua-code-index", create_if_missing=True)

@app.function(
    volumes={
        "/data/docs": docs_volume,
        "/data/code": code_volume,
    },
    schedule=modal.Cron("0 4 * * *"),
    timeout=3600,
)
def daily_index():
    crawl_docs()
    generate_docs_db()
    generate_code_index()
    docs_volume.commit()
    code_volume.commit()
```

## Error Handling

### Tag Parsing

```python
def parse_tag(tag: str) -> tuple[str, str]:
    if tag.startswith("v") and tag[1].isdigit():
        return ("cua", tag[1:])

    match = re.match(r"^(.+)-v(\d+\.\d+\.\d+.*)$", tag)
    if match:
        return (match.group(1), match.group(2))

    raise ValueError(f"Cannot parse tag: {tag}")
```

### File Handling

- Skip binary files (detect via null bytes)
- Skip files > 1MB for SQLite, > 100KB for embeddings
- Handle encoding errors with `errors='replace'`

### Query Validation

- Validate component exists before search
- Validate version exists for component
- Return helpful error messages pointing to discovery tools

## File Structure

```
docs/scripts/
├── mcp_server.py              # MODIFY: add code search tools
├── modal_app.py               # MODIFY: add code volume + indexing job
├── generate_code_index.py     # NEW: indexing pipeline
└── code_search.py             # NEW: search functions for code index
```

## Implementation Order

1. `generate_code_index.py` - Clone repo, parse tags, build SQLite + LanceDB
2. `code_search.py` - Search functions (semantic, FTS, retrieval, discovery)
3. `mcp_server.py` - Wire up new tools using code_search functions
4. `modal_app.py` - Add volume, update scheduled job, mount both volumes

## Dependencies

```
gitpython>=3.1.0  # new
lancedb           # existing
sentence-transformers  # existing
fastmcp           # existing
```
