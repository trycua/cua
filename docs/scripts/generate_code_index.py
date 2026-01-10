"""
Code index generator for CUA repository
Indexes source code across all git tags for semantic and full-text search
"""

import re
import shutil
import sqlite3
from pathlib import Path
from typing import Optional

import git
import lancedb
from lancedb.embeddings import get_registry
from lancedb.pydantic import LanceModel, Vector

# Configuration
REPO_URL = "https://github.com/trycua/cua.git"
CODE_DB_PATH = Path(__file__).parent.parent / "code_db"
REPO_PATH = CODE_DB_PATH / "repo"
SQLITE_PATH = CODE_DB_PATH / "code_index.sqlite"
LANCEDB_PATH = CODE_DB_PATH / "code_index.lancedb"

# File extensions to index
SOURCE_EXTENSIONS = {".py", ".ts", ".js", ".tsx"}

# Size limits
MAX_FILE_SIZE_SQLITE = 1_000_000  # 1MB for SQLite
MAX_FILE_SIZE_EMBEDDINGS = 100_000  # 100KB for embeddings

# Embedding model
model = get_registry().get("sentence-transformers").create(name="all-MiniLM-L6-v2")


class CodeFile(LanceModel):
    """Schema for code files in LanceDB"""

    text: str = model.SourceField()
    vector: Vector(model.ndims()) = model.VectorField()
    component: str
    version: str
    file_path: str
    language: str


def parse_tag(tag: str) -> tuple[str, str]:
    """
    Parse a git tag into component and version.

    Examples:
        agent-v0.7.3 → ("agent", "0.7.3")
        computer-server-v0.3.0 → ("computer-server", "0.3.0")
        v0.1.13 → ("cua", "0.1.13")
    """
    # Root package tags: v0.1.13 → ("cua", "0.1.13")
    if tag.startswith("v") and len(tag) > 1 and tag[1].isdigit():
        return ("cua", tag[1:])

    # Component tags: component-v1.2.3 → ("component", "1.2.3")
    match = re.match(r"^(.+)-v(\d+\.\d+\.\d+.*)$", tag)
    if match:
        return (match.group(1), match.group(2))

    raise ValueError(f"Cannot parse tag: {tag}")


def detect_language(file_path: str) -> str:
    """Detect programming language from file extension"""
    ext = Path(file_path).suffix.lower()
    language_map = {
        ".py": "python",
        ".ts": "typescript",
        ".tsx": "typescript",
        ".js": "javascript",
    }
    return language_map.get(ext, "unknown")


def clone_or_pull_repo() -> git.Repo:
    """Clone the repo if it doesn't exist, otherwise fetch latest"""
    CODE_DB_PATH.mkdir(parents=True, exist_ok=True)

    if REPO_PATH.exists():
        print("Fetching latest changes...")
        repo = git.Repo(REPO_PATH)
        repo.git.fetch("--all", "--tags")
    else:
        print(f"Cloning {REPO_URL}...")
        repo = git.Repo.clone_from(REPO_URL, REPO_PATH, bare=True)

    return repo


def get_all_tags(repo: git.Repo) -> list[str]:
    """Get all git tags from the repository"""
    tags = [tag.name for tag in repo.tags]
    return tags


def get_files_at_tag(repo: git.Repo, tag: str) -> list[str]:
    """Get list of source files at a specific tag"""
    try:
        tag_ref = repo.tag(tag)
        tree = tag_ref.commit.tree
    except (IndexError, KeyError, AttributeError, ValueError):
        return []

    files = []

    def collect_files(tree_obj, prefix=""):
        """Recursively collect files from tree"""
        for item in tree_obj:
            path = f"{prefix}{item.name}" if prefix else item.name
            if item.type == "tree":
                collect_files(item, f"{path}/")
            elif item.type == "blob":
                ext = Path(path).suffix.lower()
                if ext in SOURCE_EXTENSIONS:
                    files.append(path)

    collect_files(tree)
    return files


def get_file_content_at_tag(repo: git.Repo, tag: str, file_path: str) -> Optional[str]:
    """Get content of a file at a specific tag"""
    try:
        tag_ref = repo.tag(tag)
        tree = tag_ref.commit.tree

        # Navigate to the file in the tree
        parts = file_path.split("/")
        current = tree
        for part in parts[:-1]:
            current = current / part

        # Get the blob for the file
        blob = current / parts[-1]

        # Read content and check for binary
        content_bytes = blob.data_stream.read()
        # Try to decode as UTF-8, skip binary files
        try:
            content = content_bytes.decode("utf-8", errors="replace")
            # Check for binary content
            if "\x00" in content[:1024]:
                return None
            return content
        except Exception:
            return None
    except (KeyError, AttributeError, TypeError, ValueError):
        return None


def init_sqlite() -> sqlite3.Connection:
    """Initialize SQLite database with FTS5"""
    SQLITE_PATH.parent.mkdir(parents=True, exist_ok=True)

    # Remove existing database for full reindex
    if SQLITE_PATH.exists():
        SQLITE_PATH.unlink()

    conn = sqlite3.connect(SQLITE_PATH)
    cursor = conn.cursor()

    # Create main code_files table
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

    # Create indexes for filtering
    cursor.execute("CREATE INDEX idx_component ON code_files(component)")
    cursor.execute("CREATE INDEX idx_version ON code_files(component, version)")

    # Create FTS5 virtual table
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

    # Create triggers to keep FTS in sync
    cursor.execute(
        """
        CREATE TRIGGER code_files_ai AFTER INSERT ON code_files BEGIN
            INSERT INTO code_files_fts(rowid, content, component, version, file_path)
            VALUES (new.id, new.content, new.component, new.version, new.file_path);
        END;
    """
    )

    cursor.execute(
        """
        CREATE TRIGGER code_files_ad AFTER DELETE ON code_files BEGIN
            DELETE FROM code_files_fts WHERE rowid = old.id;
        END;
    """
    )

    cursor.execute(
        """
        CREATE TRIGGER code_files_au AFTER UPDATE ON code_files BEGIN
            DELETE FROM code_files_fts WHERE rowid = old.id;
            INSERT INTO code_files_fts(rowid, content, component, version, file_path)
            VALUES (new.id, new.content, new.component, new.version, new.file_path);
        END;
    """
    )

    conn.commit()
    return conn


def init_lancedb() -> lancedb.DBConnection:
    """Initialize LanceDB database"""
    if LANCEDB_PATH.exists():
        shutil.rmtree(LANCEDB_PATH)

    db = lancedb.connect(LANCEDB_PATH)
    db.create_table("code", schema=CodeFile, mode="overwrite")
    return db


def index_all_tags():
    """Index all git tags into SQLite and LanceDB"""
    print("Setting up repository...")
    repo = clone_or_pull_repo()

    print("Getting all tags...")
    all_tags = get_all_tags(repo)
    print(f"Found {len(all_tags)} tags")

    # Initialize databases
    print("Initializing databases...")
    sqlite_conn = init_sqlite()
    sqlite_cursor = sqlite_conn.cursor()
    lance_db = init_lancedb()
    lance_table = lance_db.open_table("code")

    # Track stats
    total_files = 0
    total_embedded = 0
    failed_tags = []

    # Process each tag
    for i, tag in enumerate(all_tags):
        print(f"\n[{i + 1}/{len(all_tags)}] Processing tag: {tag}")

        try:
            component, version = parse_tag(tag)
        except ValueError as e:
            print(f"  Skipping: {e}")
            failed_tags.append(tag)
            continue

        print(f"  Component: {component}, Version: {version}")

        # Get files at this tag
        try:
            files = get_files_at_tag(repo, tag)
        except Exception as e:
            print(f"  Failed to list files: {e}")
            failed_tags.append(tag)
            continue

        print(f"  Found {len(files)} source files")

        lance_batch = []
        files_indexed = 0

        for file_path in files:
            content = get_file_content_at_tag(repo, tag, file_path)
            if content is None:
                continue

            content_size = len(content)
            language = detect_language(file_path)

            # Add to SQLite (up to 1MB)
            if content_size <= MAX_FILE_SIZE_SQLITE:
                sqlite_cursor.execute(
                    """
                    INSERT OR REPLACE INTO code_files
                    (component, version, file_path, content, language)
                    VALUES (?, ?, ?, ?, ?)
                """,
                    (component, version, file_path, content, language),
                )
                files_indexed += 1

            # Queue for LanceDB (up to 100KB for embeddings)
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

        # Commit SQLite batch
        sqlite_conn.commit()
        total_files += files_indexed
        print(f"  Indexed {files_indexed} files to SQLite")

        # Add to LanceDB in batches
        if lance_batch:
            try:
                lance_table.add(lance_batch)
                total_embedded += len(lance_batch)
                print(f"  Embedded {len(lance_batch)} files to LanceDB")
            except Exception as e:
                print(f"  LanceDB error: {e}")

    # Final cleanup
    sqlite_conn.close()

    # Print summary
    print("\n" + "=" * 50)
    print("Indexing complete!")
    print(f"  Total tags processed: {len(all_tags)}")
    print(f"  Failed tags: {len(failed_tags)}")
    print(f"  Total files in SQLite: {total_files}")
    print(f"  Total files embedded: {total_embedded}")
    print(f"  SQLite database: {SQLITE_PATH}")
    print(f"  LanceDB database: {LANCEDB_PATH}")


def test_sqlite_search(query: str, component: str, version: str):
    """Test SQLite FTS search"""
    with sqlite3.connect(SQLITE_PATH) as conn:
        cursor = conn.cursor()

        print(f"\nFTS search for '{query}' in {component}@{version}")
        print("-" * 50)

        cursor.execute(
            """
            SELECT file_path, snippet(code_files_fts, 0, '>>>', '<<<', '...', 50) as snippet
            FROM code_files_fts
            WHERE code_files_fts MATCH ?
              AND component = ?
              AND version = ?
            ORDER BY rank
            LIMIT 5
        """,
            (query, component, version),
        )

        results = cursor.fetchall()
        for file_path, snippet in results:
            print(f"\n  {file_path}")
            print(f"  Snippet: {snippet[:200]}...")


def test_lance_search(query: str, component: str, version: str):
    """Test LanceDB semantic search"""
    db = lancedb.connect(LANCEDB_PATH)
    table = db.open_table("code")

    print(f"\nSemantic search for '{query}' in {component}@{version}")
    print("-" * 50)

    results = (
        table.search(query)
        .where(f"component = '{component}' AND version = '{version}'")
        .limit(5)
        .to_list()
    )

    for result in results:
        print(f"\n  {result['file_path']}")
        print(f"  Score: {result.get('_distance', 'N/A'):.4f}")
        print(f"  Preview: {result['text'][:150]}...")


def main():
    import argparse

    parser = argparse.ArgumentParser(description="Index CUA repository code")
    parser.add_argument("--test", action="store_true", help="Run test searches")
    args = parser.parse_args()

    if args.test:
        # Run test searches
        test_sqlite_search("ComputerAgent", "agent", "0.7.3")
        test_lance_search("how to take a screenshot", "agent", "0.7.3")
    else:
        index_all_tags()


if __name__ == "__main__":
    main()
