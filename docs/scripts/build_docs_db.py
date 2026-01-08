#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = [
#     "chromadb>=0.4.0",
# ]
# ///
"""
Build SQLite and Chroma databases from documentation MDX files.

This script:
1. Parses all MDX files from content/docs
2. Creates a SQLite database with page content and URLs
3. Creates a Chroma vector database with sentence-level embeddings

Usage:
    uv run scripts/build_docs_db.py
"""

import hashlib
import re
import shutil
import sqlite3
from pathlib import Path

import chromadb
from chromadb.utils import embedding_functions


def extract_frontmatter(content: str) -> tuple[dict, str]:
    """Extract YAML frontmatter and return (metadata, remaining_content)."""
    frontmatter = {}
    remaining = content

    # Match frontmatter between --- delimiters
    match = re.match(r"^---\s*\n(.*?)\n---\s*\n", content, re.DOTALL)
    if match:
        fm_text = match.group(1)
        remaining = content[match.end() :]

        # Parse simple YAML key-value pairs
        for line in fm_text.split("\n"):
            if ":" in line:
                key, _, value = line.partition(":")
                key = key.strip()
                value = value.strip().strip("'\"")
                if key and value:
                    frontmatter[key] = value

    return frontmatter, remaining


def clean_mdx_content(content: str) -> str:
    """Remove JSX components and clean MDX content to plain text."""
    # Remove import statements
    content = re.sub(r"^import\s+.*$", "", content, flags=re.MULTILINE)

    # Remove JSX self-closing tags like <Component />
    content = re.sub(r"<[A-Z][^>]*/>", "", content)

    # Remove JSX block tags with content like <div className="...">...</div>
    # This is a simplified approach - handles most common cases
    content = re.sub(r"<[A-Za-z][^>]*>[\s\S]*?</[A-Za-z]+>", "", content)

    # Remove remaining JSX opening/closing tags
    content = re.sub(r"</?[A-Za-z][^>]*>", "", content)

    # Remove code blocks (keep the content readable for search)
    content = re.sub(r"```[\w]*\n", "", content)
    content = re.sub(r"```", "", content)

    # Remove inline code backticks but keep content
    content = re.sub(r"`([^`]+)`", r"\1", content)

    # Remove markdown links but keep text: [text](url) -> text
    content = re.sub(r"\[([^\]]+)\]\([^)]+\)", r"\1", content)

    # Remove markdown images: ![alt](url)
    content = re.sub(r"!\[[^\]]*\]\([^)]+\)", "", content)

    # Remove HTML comments
    content = re.sub(r"<!--[\s\S]*?-->", "", content)

    # Remove JSX expressions like {variable} or {() => ...}
    content = re.sub(r"\{[^{}]*\}", "", content)

    # Remove markdown headers markers but keep text
    content = re.sub(r"^#{1,6}\s+", "", content, flags=re.MULTILINE)

    # Remove bold/italic markers
    content = re.sub(r"\*{1,2}([^*]+)\*{1,2}", r"\1", content)
    content = re.sub(r"_{1,2}([^_]+)_{1,2}", r"\1", content)

    # Remove horizontal rules
    content = re.sub(r"^[-*_]{3,}\s*$", "", content, flags=re.MULTILINE)

    # Remove list markers
    content = re.sub(r"^[\s]*[-*+]\s+", "", content, flags=re.MULTILINE)
    content = re.sub(r"^[\s]*\d+\.\s+", "", content, flags=re.MULTILINE)

    # Collapse multiple newlines
    content = re.sub(r"\n{3,}", "\n\n", content)

    # Collapse multiple spaces
    content = re.sub(r"  +", " ", content)

    return content.strip()


def file_path_to_url(file_path: Path, base_dir: Path) -> str:
    """Convert a file path to its corresponding URL in the NextJS project."""
    # Get relative path from content/docs
    rel_path = file_path.relative_to(base_dir / "content" / "docs")

    # Remove .mdx extension
    url_path = str(rel_path).replace(".mdx", "")

    # index files become the parent directory
    if url_path.endswith("/index"):
        url_path = url_path[:-6]  # Remove '/index'
    elif url_path == "index":
        url_path = ""

    # Ensure leading slash
    return f"/{url_path}" if url_path else "/"


def split_into_sentences(text: str) -> list[str]:
    """Split text into sentences for embedding."""
    # Split on sentence boundaries
    # This regex handles common sentence endings
    sentences = re.split(r"(?<=[.!?])\s+(?=[A-Z])", text)

    # Also split on double newlines (paragraphs)
    result = []
    for sentence in sentences:
        parts = sentence.split("\n\n")
        result.extend(parts)

    # Filter out very short sentences and clean up
    cleaned = []
    for s in result:
        s = s.strip()
        if len(s) > 20:  # Skip very short fragments
            cleaned.append(s)

    return cleaned


def build_databases(docs_dir: Path):
    """Build SQLite and Chroma databases from MDX files."""
    content_dir = docs_dir / "content" / "docs"
    db_dir = docs_dir / "data"
    db_dir.mkdir(exist_ok=True)

    # Find all projects (subdirectories of content/docs)
    projects = [d.name for d in content_dir.iterdir() if d.is_dir()]
    print(f"Found projects: {projects}")

    for project in projects:
        print(f"\nProcessing project: {project}")
        project_dir = content_dir / project

        # Initialize databases for this project
        sqlite_path = db_dir / f"{project}.sqlite"
        chroma_path = db_dir / f"{project}_chroma"

        # Remove existing databases for clean rebuild
        if sqlite_path.exists():
            sqlite_path.unlink()
        if chroma_path.exists():
            shutil.rmtree(chroma_path)

        # Create SQLite database
        conn = sqlite3.connect(sqlite_path)
        cursor = conn.cursor()
        cursor.execute(
            """
            CREATE TABLE pages (
                id TEXT PRIMARY KEY,
                file_path TEXT NOT NULL,
                url TEXT NOT NULL UNIQUE,
                title TEXT,
                content TEXT NOT NULL,
                raw_content TEXT NOT NULL
            )
        """
        )
        cursor.execute("CREATE INDEX idx_url ON pages(url)")

        # Create Chroma database with default embedding function
        chroma_client = chromadb.PersistentClient(path=str(chroma_path))
        # Use default embedding function (all-MiniLM-L6-v2)
        embedding_fn = embedding_functions.DefaultEmbeddingFunction()
        collection = chroma_client.create_collection(
            name="docs", embedding_function=embedding_fn
        )

        # Find all MDX files in this project
        mdx_files = list(project_dir.rglob("*.mdx"))
        print(f"  Found {len(mdx_files)} MDX files")

        total_sentences = 0

        for mdx_file in mdx_files:
            # Read file content
            raw_content = mdx_file.read_text(encoding="utf-8")

            # Extract frontmatter and clean content
            frontmatter, body = extract_frontmatter(raw_content)
            clean_content = clean_mdx_content(body)
            title = frontmatter.get("title", mdx_file.stem)

            # Generate URL - adjust path to be relative to content/docs
            url = file_path_to_url(mdx_file, docs_dir)

            # Create deterministic ID from file path
            file_rel_path = str(mdx_file.relative_to(docs_dir))
            page_id = hashlib.sha256(file_rel_path.encode()).hexdigest()[:16]

            # Insert into SQLite
            cursor.execute(
                """
                INSERT INTO pages (id, file_path, url, title, content, raw_content)
                VALUES (?, ?, ?, ?, ?, ?)
            """,
                (page_id, file_rel_path, url, title, clean_content, raw_content),
            )

            # Split into sentences and add to Chroma
            sentences = split_into_sentences(clean_content)
            for i, sentence in enumerate(sentences):
                sentence_id = f"{page_id}_{i}"
                collection.add(
                    ids=[sentence_id],
                    documents=[sentence],
                    metadatas=[{"url": url, "title": title, "page_id": page_id}],
                )
                total_sentences += 1

        conn.commit()
        conn.close()

        print(f"  SQLite database: {sqlite_path}")
        print(f"  Chroma database: {chroma_path}")
        print(f"  Total sentences indexed: {total_sentences}")


def main():
    # Determine docs directory (script is in docs/scripts/)
    script_dir = Path(__file__).parent
    docs_dir = script_dir.parent

    print(f"Building databases from: {docs_dir}")
    build_databases(docs_dir)
    print("\nDone!")


if __name__ == "__main__":
    main()
