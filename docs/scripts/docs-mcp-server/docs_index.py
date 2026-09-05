"""Shared docs corpus, immutable generation, publication, and reader contract."""

import hashlib
import json
import logging
import re
import shutil
import sqlite3
import threading
import uuid
from datetime import datetime, timezone
from pathlib import Path

FORMAT_VERSION = 1
EMBEDDING = {"model": "all-MiniLM-L6-v2", "dimensions": 384}
WRITER_VERSIONS = {"lancedb": "0.37.1", "sentence-transformers": "5.7.0", "pyarrow": "25.0.1"}


def write_json(path, value):
    Path(path).write_text(json.dumps(value, sort_keys=True, indent=2), encoding="utf-8")


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True).encode()).hexdigest()


def capture_corpus(pages, expected_urls, failed_urls=()):
    """Identify one completed crawl; a missing discovered page fails the build."""
    urls = [p["url"] for p in pages]
    if failed_urls or not urls or any(not u for u in urls) or len(urls) != len(set(urls)):
        raise ValueError("Incomplete crawl: failed, empty, or duplicate pages")
    if set(urls) != set(expected_urls):
        raise ValueError("Incomplete crawl: discovered URLs differ from captured pages")
    pages = sorted(pages, key=lambda p: p["url"])
    return {
        "format_version": FORMAT_VERSION,
        "source": "https://cua.ai/docs (links discovered from /docs)",
        "captured_at": datetime.now(timezone.utc).isoformat(),
        "corpus_id": digest(pages),
        "expected_urls": sorted(urls),
        "pages": pages,
    }


def load_corpus(path):
    corpus = json.loads(Path(path).read_text())
    checked = capture_corpus(corpus["pages"], corpus["expected_urls"])
    if (
        corpus.get("format_version") != FORMAT_VERSION
        or checked["corpus_id"] != corpus["corpus_id"]
    ):
        raise ValueError("Corpus identity mismatch")
    return corpus


def prepare_pages(corpus):
    from markdown_it import MarkdownIt

    parser = MarkdownIt()
    included, exclusions = [], []
    for page in corpus["pages"]:
        parts = []
        for token in parser.parse(page.get("markdown", "")):
            if token.type in ("fence", "code_block"):
                parts.append(token.content)
            elif token.type == "inline":
                parts.append(
                    " ".join(
                        c.content for c in token.children or [] if c.type in ("text", "code_inline")
                    )
                )
        text = "\n\n".join(parts).strip()
        if not text:
            exclusions.append({"url": page["url"], "reason": "no searchable text"})
            continue
        included.append({**page, "text": text})
    if not included:
        raise ValueError("Corpus has no searchable pages")
    return included, exclusions


def make_chunks(pages, build_id):
    """Keep short pages and short paragraphs; both indexes cover identical URLs."""
    return [
        {
            "text": page["text"][offset : offset + 1000],
            "url": page["url"],
            "title": page.get("title") or "Untitled",
            "category": page.get("path_info", {}).get("category") or "unknown",
            "chunk_index": index,
            "build_id": build_id,
        }
        for page in pages
        for index, offset in enumerate(range(0, len(page["text"]), 800))
    ]


def write_sqlite(path, pages, build_id, corpus):
    with sqlite3.connect(path) as conn:
        conn.executescript("""
            CREATE TABLE pages (id INTEGER PRIMARY KEY, url TEXT UNIQUE NOT NULL,
                title TEXT, category TEXT, content TEXT, build_id TEXT NOT NULL,
                description TEXT, subcategory TEXT, page_name TEXT, raw_markdown TEXT);
            CREATE VIRTUAL TABLE pages_fts USING fts5(content, url UNINDEXED,
                title UNINDEXED, category UNINDEXED, content='pages', content_rowid='id');
            CREATE TABLE index_metadata (build_id TEXT NOT NULL, corpus_id TEXT NOT NULL,
                captured_at TEXT NOT NULL, source TEXT NOT NULL);
        """)
        conn.execute(
            "INSERT INTO index_metadata VALUES (?,?,?,?)",
            (build_id, corpus["corpus_id"], corpus["captured_at"], corpus["source"]),
        )
        conn.executemany(
            "INSERT INTO pages (url,title,category,content,build_id,description,subcategory,page_name,raw_markdown) "
            "VALUES (?,?,?,?,?,?,?,?,?)",
            [
                (
                    p["url"],
                    p.get("title") or "Untitled",
                    p.get("path_info", {}).get("category") or "unknown",
                    p["text"],
                    build_id,
                    p.get("description", ""),
                    p.get("path_info", {}).get("subcategory"),
                    p.get("path_info", {}).get("page", ""),
                    p.get("markdown", ""),
                )
                for p in pages
            ],
        )
        conn.execute("INSERT INTO pages_fts(pages_fts) VALUES ('rebuild')")


def write_vectors(path, chunks):
    from importlib.metadata import version

    for package, expected in WRITER_VERSIONS.items():
        if version(package) != expected:
            raise RuntimeError(
                f"Use the docs-mcp-server locked environment: {package}=={expected} required"
            )
    import lancedb
    from lancedb.embeddings import get_registry
    from lancedb.pydantic import LanceModel, Vector

    model = get_registry().get("sentence-transformers").create(name=EMBEDDING["model"])

    class DocChunk(LanceModel):
        text: str = model.SourceField()
        vector: Vector(model.ndims()) = model.VectorField()
        url: str
        title: str
        category: str
        chunk_index: int
        build_id: str

    table = lancedb.connect(path).create_table("docs", schema=DocChunk)
    for offset in range(0, len(chunks), 100):
        table.add(chunks[offset : offset + 100])


def open_vectors(path):
    import lancedb

    return lancedb.connect(path).open_table("docs")


def validate_pair(path, manifest, table):
    conn = sqlite3.connect(
        f"{(Path(path) / 'docs.sqlite').as_uri()}?mode=ro", uri=True, check_same_thread=False
    )
    try:
        expected = set(manifest["included_urls"])
        if expected | {e["url"] for e in manifest["exclusions"]} != set(manifest["expected_urls"]):
            raise ValueError("Manifest corpus coverage mismatch")
        metadata = conn.execute(
            "SELECT build_id,corpus_id,captured_at,source FROM index_metadata"
        ).fetchall()
        if metadata != [
            (
                manifest["build_id"],
                manifest["corpus_id"],
                manifest["captured_at"],
                manifest["source"],
            )
        ]:
            raise ValueError("SQLite build identity mismatch")
        rows = conn.execute("SELECT url, build_id FROM pages").fetchall()
        vectors = (
            table.search().select(["url", "build_id"]).limit(manifest["chunk_count"] + 1).to_list()
        )
        if (
            {r[0] for r in rows} != expected
            or {r["url"] for r in vectors} != expected
            or len(rows) != len(expected)
            or len(vectors) != manifest["chunk_count"]
            or any(r[1] != manifest["build_id"] for r in rows)
            or any(r["build_id"] != manifest["build_id"] for r in vectors)
        ):
            raise ValueError("SQLite/vector corpus or build identity mismatch")
        conn.row_factory = sqlite3.Row
        return conn
    except Exception:
        conn.close()
        raise


def file_hash(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def build_generation(corpus, root, vector_writer=write_vectors, vector_opener=open_vectors):
    """Build without touching the active generation. Return only a verified pair."""
    checked = capture_corpus(corpus["pages"], corpus["expected_urls"])
    if checked["corpus_id"] != corpus["corpus_id"]:
        raise ValueError("Corpus identity mismatch")
    pages, exclusions = prepare_pages(corpus)
    build_id = uuid.uuid4().hex
    path = Path(root).resolve() / "generations" / build_id
    path.mkdir(parents=True)
    chunks = make_chunks(pages, build_id)
    write_sqlite(path / "docs.sqlite", pages, build_id, corpus)
    vector_writer(path, chunks)
    manifest = {
        "format_version": FORMAT_VERSION,
        "build_id": build_id,
        "corpus_id": corpus["corpus_id"],
        "source": corpus["source"],
        "captured_at": corpus["captured_at"],
        "embedding": EMBEDDING,
        "writer_versions": WRITER_VERSIONS,
        "expected_urls": corpus["expected_urls"],
        "included_urls": [p["url"] for p in pages],
        "exclusions": exclusions,
        "chunk_count": len(chunks),
    }
    validate_pair(path, manifest, vector_opener(path)).close()
    manifest["files"] = {
        p.relative_to(path).as_posix(): file_hash(p) for p in sorted(path.rglob("*")) if p.is_file()
    }
    write_json(path / "manifest.json", manifest)
    return path, manifest


def verify_files(path, manifest):
    if manifest.get("format_version") != FORMAT_VERSION or manifest.get("embedding") != EMBEDDING:
        raise ValueError("Unsupported docs generation")
    if not manifest.get("files") or "docs.sqlite" not in manifest["files"]:
        raise ValueError("Incomplete docs generation manifest")
    actual = {
        p.relative_to(path).as_posix()
        for p in Path(path).rglob("*")
        if p.is_file() and p != Path(path) / "manifest.json"
    }
    if actual != set(manifest["files"]):
        raise ValueError("Incomplete docs generation: file inventory mismatch")
    for name, sha in manifest["files"].items():
        file = Path(path) / name
        if (
            file.resolve().parent != Path(path).resolve()
            and Path(path).resolve() not in file.resolve().parents
        ):
            raise ValueError("Invalid generation file path")
        if not file.is_file() or file_hash(file) != sha:
            raise ValueError(f"Incomplete docs generation: {name}")


def pointer_for(manifest):
    return {"build_id": manifest["build_id"], "manifest_sha256": digest(manifest)}


def activate(root, manifest):
    """Local filesystems: replace the small pointer only after validation/copy."""
    root = Path(root)
    verify_files(root / "generations" / manifest["build_id"], manifest)
    staging = root / f".current-{uuid.uuid4().hex}.json"
    write_json(staging, pointer_for(manifest))
    staging.replace(root / "current.json")


def publish_generation(s3, bucket, path, manifest):
    """Immutable objects first, then one commit marker. Never delete old builds."""
    verify_files(path, manifest)
    if json.loads((Path(path) / "manifest.json").read_text()) != manifest:
        raise ValueError("Publication manifest mismatch")
    prefix = f"docs_db/generations/{manifest['build_id']}/"
    for name in sorted(manifest["files"]):
        s3.upload_file(str(Path(path) / name), bucket, prefix + name)
    s3.upload_file(str(Path(path) / "manifest.json"), bucket, prefix + "manifest.json")
    s3.put_object(
        Bucket=bucket,
        Key="docs_db/current.json",
        Body=json.dumps(pointer_for(manifest)).encode(),
        ContentType="application/json",
    )
    return len(manifest["files"]) + 2


def copy_generation(source, destination):
    """Copy a committed generation to local disk, then activate it atomically."""
    source, destination = Path(source), Path(destination)
    pointer = json.loads((source / "current.json").read_text())
    build_id = pointer["build_id"]
    if not re.fullmatch(r"[a-f0-9]{32}", build_id):
        raise ValueError("Invalid docs build id")
    if (destination / "current.json").exists():
        if json.loads((destination / "current.json").read_text()) == pointer:
            return
    origin = source / "generations" / build_id
    manifest = json.loads((origin / "manifest.json").read_text())
    if pointer != pointer_for(manifest):
        raise ValueError("Copy manifest identity mismatch")
    verify_files(origin, manifest)
    target = destination / "generations" / build_id
    shutil.copytree(origin, target, dirs_exist_ok=True)
    activate(destination, manifest)


class GenerationReader:
    """Refresh both handles together; reject bad candidates and keep last good."""

    def __init__(self, root, vector_opener=open_vectors, refresh=None):
        self.root = Path(root).resolve()
        self.vector_opener = vector_opener
        self.current = None
        self.error = None
        self.lock = threading.RLock()
        self.refresh = refresh

    def get(self):
        with self.lock:
            try:
                if self.refresh:
                    self.refresh()
                pointer = json.loads((self.root / "current.json").read_text())
                build_id = pointer["build_id"]
                if not re.fullmatch(r"[a-f0-9]{32}", build_id):
                    raise ValueError("Invalid docs build id")
                if self.current and pointer == pointer_for(self.current[2]):
                    self.error = None
                    return self.current
                path = self.root / "generations" / build_id
                manifest = json.loads((path / "manifest.json").read_text())
                if (
                    manifest["build_id"] != build_id
                    or digest(manifest) != pointer["manifest_sha256"]
                ):
                    raise ValueError("Manifest identity mismatch")
                verify_files(path, manifest)
                table = self.vector_opener(path)
                conn = validate_pair(path, manifest, table)
                # Requests retain their returned handle while a new generation opens.
                self.current = (conn, table, manifest)
                self.error = None
            except Exception as error:
                self.error = str(error)
                logging.warning("Docs generation refresh rejected: %s", error)
                if self.current is None:
                    raise RuntimeError("No verified docs generation available") from error
            return self.current
