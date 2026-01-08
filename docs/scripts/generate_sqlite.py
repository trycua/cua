"""
SQLite database generator for CUA documentation
Creates a full-text search enabled SQLite database from crawled data
"""

import json
import re
import sqlite3
from pathlib import Path

# Configuration
CRAWLED_DATA_DIR = Path(__file__).parent.parent / "crawled_data"
SQLITE_PATH = Path(__file__).parent.parent / "docs_db" / "docs.sqlite"


def clean_markdown(markdown: str) -> str:
    """Clean markdown content"""
    text = re.sub(r'\n{3,}', '\n\n', markdown)
    text = re.sub(r'!\[.*?\]\(.*?\)', '', text)
    text = re.sub(r'\[([^\]]+)\]\([^)]+\)', r'\1', text)
    text = re.sub(r'<[^>]+>', '', text)
    text = re.sub(r' {2,}', ' ', text)
    return text.strip()


def load_crawled_data() -> list[dict]:
    """Load all crawled page data"""
    all_pages_file = CRAWLED_DATA_DIR / "_all_pages.json"

    if all_pages_file.exists():
        with open(all_pages_file, 'r', encoding='utf-8') as f:
            return json.load(f)

    pages = []
    for json_file in CRAWLED_DATA_DIR.glob("*.json"):
        if json_file.name.startswith('_'):
            continue
        with open(json_file, 'r', encoding='utf-8') as f:
            pages.append(json.load(f))

    return pages


def create_database(pages: list[dict]):
    """Create SQLite database with FTS5 full-text search"""
    # Ensure parent directory exists
    SQLITE_PATH.parent.mkdir(parents=True, exist_ok=True)

    # Remove existing database
    if SQLITE_PATH.exists():
        SQLITE_PATH.unlink()

    conn = sqlite3.connect(SQLITE_PATH)
    cursor = conn.cursor()

    # Create main pages table
    cursor.execute("""
        CREATE TABLE pages (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            url TEXT UNIQUE NOT NULL,
            title TEXT,
            description TEXT,
            category TEXT,
            subcategory TEXT,
            page_name TEXT,
            content TEXT,
            raw_markdown TEXT
        )
    """)

    # Create FTS5 virtual table for full-text search
    cursor.execute("""
        CREATE VIRTUAL TABLE pages_fts USING fts5(
            title,
            content,
            url,
            content='pages',
            content_rowid='id'
        )
    """)

    # Create triggers to keep FTS in sync
    cursor.execute("""
        CREATE TRIGGER pages_ai AFTER INSERT ON pages BEGIN
            INSERT INTO pages_fts(rowid, title, content, url)
            VALUES (new.id, new.title, new.content, new.url);
        END
    """)

    cursor.execute("""
        CREATE TRIGGER pages_ad AFTER DELETE ON pages BEGIN
            INSERT INTO pages_fts(pages_fts, rowid, title, content, url)
            VALUES('delete', old.id, old.title, old.content, old.url);
        END
    """)

    cursor.execute("""
        CREATE TRIGGER pages_au AFTER UPDATE ON pages BEGIN
            INSERT INTO pages_fts(pages_fts, rowid, title, content, url)
            VALUES('delete', old.id, old.title, old.content, old.url);
            INSERT INTO pages_fts(rowid, title, content, url)
            VALUES (new.id, new.title, new.content, new.url);
        END
    """)

    # Insert pages
    for page in pages:
        markdown = page.get("markdown", "")
        if not markdown:
            continue

        content = clean_markdown(markdown)
        if not content or len(content) < 50:
            continue

        path_info = page.get("path_info", {})

        cursor.execute("""
            INSERT OR REPLACE INTO pages
            (url, title, description, category, subcategory, page_name, content, raw_markdown)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)
        """, (
            page.get("url", ""),
            page.get("title") or path_info.get("page", "") or "Untitled",
            page.get("description", ""),
            path_info.get("category", "unknown"),
            path_info.get("subcategory"),
            path_info.get("page", ""),
            content,
            markdown,
        ))

    conn.commit()

    # Get stats
    cursor.execute("SELECT COUNT(*) FROM pages")
    page_count = cursor.fetchone()[0]

    cursor.execute("SELECT category, COUNT(*) FROM pages GROUP BY category")
    categories = cursor.fetchall()

    conn.close()

    print(f"SQLite database created at: {SQLITE_PATH}")
    print(f"Total pages: {page_count}")
    print("Pages by category:")
    for cat, count in categories:
        print(f"  - {cat}: {count}")


def test_search(query: str):
    """Test full-text search"""
    conn = sqlite3.connect(SQLITE_PATH)
    cursor = conn.cursor()

    print(f"\nFTS5 search for: '{query}'")
    print("-" * 50)

    cursor.execute("""
        SELECT url, title, snippet(pages_fts, 1, '>>>', '<<<', '...', 50) as snippet
        FROM pages_fts
        WHERE pages_fts MATCH ?
        ORDER BY rank
        LIMIT 5
    """, (query,))

    results = cursor.fetchall()
    for url, title, snippet in results:
        print(f"\n{title}")
        print(f"  URL: {url}")
        print(f"  Snippet: {snippet}")

    conn.close()


def main():
    print("Loading crawled data...")
    pages = load_crawled_data()
    print(f"Loaded {len(pages)} pages")

    if not pages:
        print("No crawled data found. Run crawl_docs.py first.")
        return

    print("\nCreating SQLite database...")
    create_database(pages)

    # Test searches
    print("\n" + "=" * 50)
    print("Testing FTS5 search")
    print("=" * 50)

    test_search("install")
    test_search("computer use agent")
    test_search("benchmark")


if __name__ == "__main__":
    main()
