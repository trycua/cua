"""
Database generator for CUA documentation
Parses crawled JSON data and creates a LanceDB vector database for RAG
"""

import json
import re
from pathlib import Path
from typing import Optional

import lancedb
from lancedb.embeddings import get_registry
from lancedb.pydantic import LanceModel, Vector

# Configuration
CRAWLED_DATA_DIR = Path(__file__).parent.parent / "crawled_data"
DB_PATH = Path(__file__).parent.parent / "docs_db"
CHUNK_SIZE = 1000  # Characters per chunk
CHUNK_OVERLAP = 200  # Overlap between chunks

# Use sentence-transformers for embeddings
model = get_registry().get("sentence-transformers").create(name="all-MiniLM-L6-v2")


class DocChunk(LanceModel):
    """Schema for document chunks in the database"""
    text: str = model.SourceField()
    vector: Vector(model.ndims()) = model.VectorField()
    url: str
    title: str
    category: str
    subcategory: Optional[str]
    page: str
    chunk_index: int


def clean_markdown(markdown: str) -> str:
    """Clean markdown content for better chunking"""
    # Remove excessive whitespace
    text = re.sub(r'\n{3,}', '\n\n', markdown)
    # Remove image markdown
    text = re.sub(r'!\[.*?\]\(.*?\)', '', text)
    # Remove link URLs but keep text
    text = re.sub(r'\[([^\]]+)\]\([^)]+\)', r'\1', text)
    # Remove HTML tags
    text = re.sub(r'<[^>]+>', '', text)
    # Clean up whitespace
    text = re.sub(r' {2,}', ' ', text)
    return text.strip()


def chunk_text(text: str, chunk_size: int = CHUNK_SIZE, overlap: int = CHUNK_OVERLAP) -> list[str]:
    """Split text into overlapping chunks, respecting sentence boundaries"""
    if not text:
        return []

    # Split by paragraphs first
    paragraphs = text.split('\n\n')
    chunks = []
    current_chunk = ""

    for para in paragraphs:
        para = para.strip()
        if not para:
            continue

        # If adding this paragraph exceeds chunk size, save current and start new
        if len(current_chunk) + len(para) + 2 > chunk_size:
            if current_chunk:
                chunks.append(current_chunk.strip())
                # Start new chunk with overlap from previous
                if overlap > 0 and len(current_chunk) > overlap:
                    # Try to find a sentence boundary for overlap
                    overlap_text = current_chunk[-overlap:]
                    sentence_end = overlap_text.rfind('. ')
                    if sentence_end > 0:
                        overlap_text = overlap_text[sentence_end + 2:]
                    current_chunk = overlap_text + "\n\n" + para
                else:
                    current_chunk = para
            else:
                # Single paragraph exceeds chunk size, split by sentences
                sentences = re.split(r'(?<=[.!?])\s+', para)
                for sentence in sentences:
                    if len(current_chunk) + len(sentence) + 1 > chunk_size:
                        if current_chunk:
                            chunks.append(current_chunk.strip())
                            # Start new chunk with overlap from previous, similar to paragraph logic
                            if overlap > 0 and len(current_chunk) > overlap:
                                overlap_text = current_chunk[-overlap:]
                                sentence_end = overlap_text.rfind('. ')
                                if sentence_end > 0:
                                    overlap_text = overlap_text[sentence_end + 2:]
                                current_chunk = (overlap_text + " " + sentence).strip()
                            else:
                                current_chunk = sentence.strip()
                        else:
                            # No existing chunk; start with this sentence
                            current_chunk = sentence.strip()
                    else:
                        current_chunk = (current_chunk + " " + sentence).strip()
        else:
            current_chunk = (current_chunk + "\n\n" + para).strip()

    # Don't forget the last chunk
    if current_chunk:
        chunks.append(current_chunk.strip())

    return chunks


def load_crawled_data() -> list[dict]:
    """Load all crawled page data"""
    all_pages_file = CRAWLED_DATA_DIR / "_all_pages.json"

    if all_pages_file.exists():
        with open(all_pages_file, 'r', encoding='utf-8') as f:
            return json.load(f)

    # Fallback: load individual files
    pages = []
    for json_file in CRAWLED_DATA_DIR.glob("*.json"):
        if json_file.name.startswith('_'):
            continue
        with open(json_file, 'r', encoding='utf-8') as f:
            pages.append(json.load(f))

    return pages


def process_pages(pages: list[dict]) -> list[dict]:
    """Process pages into document chunks"""
    all_chunks = []

    for page in pages:
        markdown = page.get("markdown", "")
        if not markdown:
            continue

        # Clean the markdown
        cleaned_text = clean_markdown(markdown)
        if not cleaned_text or len(cleaned_text) < 50:
            continue

        # Get path info
        path_info = page.get("path_info", {})

        # Chunk the text
        text_chunks = chunk_text(cleaned_text)

        # Ensure non-null values for required fields
        url = page.get("url", "")
        title = page.get("title") or path_info.get("page", "") or "Untitled"
        category = path_info.get("category") or "unknown"
        page_name = path_info.get("page") or ""

        for i, chunk_text_content in enumerate(text_chunks):
            chunk = {
                "text": chunk_text_content,
                "url": url,
                "title": title,
                "category": category,
                "subcategory": path_info.get("subcategory"),
                "page": page_name,
                "chunk_index": i,
            }
            all_chunks.append(chunk)

    return all_chunks


def create_database(chunks: list[dict]):
    """Create LanceDB database from chunks"""
    # Remove existing database
    if DB_PATH.exists():
        import shutil
        shutil.rmtree(DB_PATH)

    # Create database
    db = lancedb.connect(DB_PATH)

    # Create table with schema
    table = db.create_table(
        "docs",
        schema=DocChunk,
        mode="overwrite",
    )

    # Add data in batches
    batch_size = 100
    for i in range(0, len(chunks), batch_size):
        batch = chunks[i:i + batch_size]
        print(f"Adding batch {i // batch_size + 1}/{(len(chunks) + batch_size - 1) // batch_size}")
        table.add(batch)

    print(f"Database created at: {DB_PATH}")
    print(f"Total chunks: {len(chunks)}")

    return db


def test_search(db: lancedb.DBConnection, query: str, limit: int = 5):
    """Test search functionality"""
    table = db.open_table("docs")

    print(f"\nSearching for: '{query}'")
    print("-" * 50)

    results = table.search(query).limit(limit).to_list()

    for i, result in enumerate(results):
        print(f"\n{i + 1}. [{result['category']}] {result['title']}")
        print(f"   URL: {result['url']}")
        print(f"   Score: {result.get('_distance', 'N/A'):.4f}")
        print(f"   Preview: {result['text'][:150]}...")


def main():
    print("Loading crawled data...")
    pages = load_crawled_data()
    print(f"Loaded {len(pages)} pages")

    if not pages:
        print("No crawled data found. Run crawl_docs.py first.")
        return

    print("\nProcessing pages into chunks...")
    chunks = process_pages(pages)
    print(f"Created {len(chunks)} chunks")

    if not chunks:
        print("No chunks created. Check your crawled data.")
        return

    print("\nCreating database...")
    db = create_database(chunks)

    # Test with sample queries
    print("\n" + "=" * 50)
    print("Testing search functionality")
    print("=" * 50)

    test_queries = [
        "how to install CUA",
        "computer use agent",
        "benchmark evaluation",
        "API reference",
    ]

    for query in test_queries:
        test_search(db, query)

    print("\n" + "=" * 50)
    print("Database generation complete!")
    print(f"Database location: {DB_PATH}")


if __name__ == "__main__":
    main()
