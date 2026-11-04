"""Build a validated SQLite/vector pair from one completed docs crawl."""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent / "docs-mcp-server"))
from docs_index import activate, build_generation, load_corpus


def main():
    root = Path(__file__).parent.parent
    corpus = load_corpus(root / "crawled_data" / "_corpus.json")
    _, manifest = build_generation(corpus, root / "docs_db")
    activate(root / "docs_db", manifest)
    print(
        f"Activated docs build {manifest['build_id']}: "
        f"{len(manifest['included_urls'])} pages, {manifest['chunk_count']} chunks"
    )


if __name__ == "__main__":
    main()
