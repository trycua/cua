"""
Git repository SQLite database generator for trycua/cua and trycua/cloud

Creates a lightweight SQLite database containing:
1. Latest file contents at HEAD (read-only snapshot of both repos)
2. Last N commits with metadata and per-commit file change lists

This database is designed to be consumed by the docs-mcp-server for
repo-aware queries without needing a full git clone at query time.

Usage:
    python generate_repo_db.py [--commits 50] [--output /path/to/repo.sqlite]
    python generate_repo_db.py --local  # Use local repo checkouts instead of cloning
"""

import argparse
import os
import sqlite3
import subprocess
import tempfile
from pathlib import Path

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------

REPOS = {
    "cua": "https://github.com/trycua/cua.git",
    "cloud": "https://github.com/trycua/cloud.git",
}

# Local checkouts (used when --local flag is passed or when available)
LOCAL_REPOS = {
    "cua": Path(__file__).parent.parent.parent,  # /cua root
    "cloud": Path(__file__).parent.parent.parent.parent / "cloud",  # ../cloud
}

DEFAULT_COMMITS = 50
DEFAULT_DB_PATH = Path(__file__).parent.parent / "repo_db" / "repo.sqlite"

# File extensions to index (source code + config + docs)
SOURCE_EXTENSIONS = {
    ".py", ".ts", ".tsx", ".js", ".jsx",
    ".md", ".mdx", ".rst", ".txt",
    ".yaml", ".yml", ".toml", ".json", ".jsonc",
    ".sh", ".bash", ".zsh",
    ".nix", ".nixos",
    ".go", ".rs", ".c", ".cpp", ".h", ".hpp",
    ".dockerfile", ".containerfile",
    ".env.example", ".env.sample",
    ".sql",
    ".css", ".scss",
    ".html", ".xml",
}

# Files to always skip regardless of extension
SKIP_PATTERNS = {
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "poetry.lock",
    "Cargo.lock",
    ".gitignore",
    ".DS_Store",
}

# Skip directories
SKIP_DIRS = {
    "node_modules", ".git", "__pycache__", ".venv", "venv",
    "dist", "build", ".next", ".nuxt", "target",
    ".mypy_cache", ".ruff_cache", ".pytest_cache",
}

# Max file size to index (bytes) — keeps the DB lightweight
MAX_FILE_SIZE = 150_000  # 150 KB


# ---------------------------------------------------------------------------
# Language detection
# ---------------------------------------------------------------------------

EXTENSION_TO_LANGUAGE = {
    ".py": "python",
    ".ts": "typescript",
    ".tsx": "typescript",
    ".js": "javascript",
    ".jsx": "javascript",
    ".md": "markdown",
    ".mdx": "markdown",
    ".rst": "rst",
    ".txt": "text",
    ".yaml": "yaml",
    ".yml": "yaml",
    ".toml": "toml",
    ".json": "json",
    ".jsonc": "json",
    ".sh": "shell",
    ".bash": "shell",
    ".zsh": "shell",
    ".nix": "nix",
    ".go": "go",
    ".rs": "rust",
    ".c": "c",
    ".cpp": "cpp",
    ".h": "c",
    ".hpp": "cpp",
    ".sql": "sql",
    ".css": "css",
    ".scss": "scss",
    ".html": "html",
    ".xml": "xml",
}


def detect_language(file_path: str) -> str:
    path = Path(file_path)
    name_lower = path.name.lower()
    if name_lower in ("dockerfile", "containerfile"):
        return "dockerfile"
    return EXTENSION_TO_LANGUAGE.get(path.suffix.lower(), "text")


# ---------------------------------------------------------------------------
# Database schema
# ---------------------------------------------------------------------------


def create_schema(cursor: sqlite3.Cursor) -> None:
    """Create all tables, indexes, FTS virtual tables and triggers."""

    # -- files: latest HEAD snapshot ----------------------------------------
    cursor.execute("""
        CREATE TABLE files (
            id       INTEGER PRIMARY KEY AUTOINCREMENT,
            repo     TEXT    NOT NULL,
            path     TEXT    NOT NULL,
            content  TEXT    NOT NULL,
            language TEXT    NOT NULL,
            size     INTEGER NOT NULL,
            UNIQUE(repo, path)
        )
    """)

    cursor.execute("""
        CREATE VIRTUAL TABLE files_fts USING fts5(
            content,
            repo     UNINDEXED,
            path     UNINDEXED,
            language UNINDEXED,
            content='files',
            content_rowid='id'
        )
    """)

    cursor.execute("""
        CREATE TRIGGER files_ai AFTER INSERT ON files BEGIN
            INSERT INTO files_fts(rowid, content, repo, path, language)
            VALUES (new.id, new.content, new.repo, new.path, new.language);
        END
    """)
    cursor.execute("""
        CREATE TRIGGER files_ad AFTER DELETE ON files BEGIN
            DELETE FROM files_fts WHERE rowid = old.id;
        END
    """)
    cursor.execute("""
        CREATE TRIGGER files_au AFTER UPDATE ON files BEGIN
            DELETE FROM files_fts WHERE rowid = old.id;
            INSERT INTO files_fts(rowid, content, repo, path, language)
            VALUES (new.id, new.content, new.repo, new.path, new.language);
        END
    """)

    # -- commits ------------------------------------------------------------
    cursor.execute("""
        CREATE TABLE commits (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            repo         TEXT    NOT NULL,
            hash         TEXT    NOT NULL,
            short_hash   TEXT    NOT NULL,
            author_name  TEXT,
            author_email TEXT,
            date         TEXT    NOT NULL,
            subject      TEXT    NOT NULL,
            body         TEXT,
            files_changed INTEGER DEFAULT 0,
            insertions    INTEGER DEFAULT 0,
            deletions     INTEGER DEFAULT 0,
            UNIQUE(repo, hash)
        )
    """)

    cursor.execute("""
        CREATE VIRTUAL TABLE commits_fts USING fts5(
            subject,
            body,
            repo         UNINDEXED,
            hash         UNINDEXED,
            author_name  UNINDEXED,
            date         UNINDEXED,
            content='commits',
            content_rowid='id'
        )
    """)

    cursor.execute("""
        CREATE TRIGGER commits_ai AFTER INSERT ON commits BEGIN
            INSERT INTO commits_fts(rowid, subject, body, repo, hash, author_name, date)
            VALUES (new.id, new.subject, new.body, new.repo, new.hash, new.author_name, new.date);
        END
    """)
    cursor.execute("""
        CREATE TRIGGER commits_ad AFTER DELETE ON commits BEGIN
            DELETE FROM commits_fts WHERE rowid = old.id;
        END
    """)

    # -- commit_files -------------------------------------------------------
    cursor.execute("""
        CREATE TABLE commit_files (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            commit_id   INTEGER NOT NULL REFERENCES commits(id),
            repo        TEXT    NOT NULL,
            path        TEXT    NOT NULL,
            change_type TEXT    NOT NULL,
            additions   INTEGER DEFAULT 0,
            deletions   INTEGER DEFAULT 0
        )
    """)
    cursor.execute("CREATE INDEX idx_cf_commit   ON commit_files(commit_id)")
    cursor.execute("CREATE INDEX idx_cf_path     ON commit_files(repo, path)")

    # -- metadata -----------------------------------------------------------
    cursor.execute("""
        CREATE TABLE meta (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        )
    """)


# ---------------------------------------------------------------------------
# Git helpers
# ---------------------------------------------------------------------------


def run_git(args: list[str], cwd: str, check: bool = True) -> subprocess.CompletedProcess:
    return subprocess.run(
        ["git"] + args,
        cwd=cwd,
        check=check,
        capture_output=True,
        text=True,
    )


def should_index_file(path: str) -> bool:
    p = Path(path)
    # Skip hidden files and known skip patterns
    if p.name in SKIP_PATTERNS:
        return False
    # Skip files inside skipped directories
    parts = set(p.parts)
    if parts & SKIP_DIRS:
        return False
    # Allow files without extension that match known names
    name_lower = p.name.lower()
    if name_lower in ("dockerfile", "containerfile", "makefile", "procfile"):
        return True
    # Require a known extension
    return p.suffix.lower() in SOURCE_EXTENSIONS


def get_head_files(repo_path: str) -> list[str]:
    """Return list of all file paths tracked at HEAD."""
    result = run_git(["ls-tree", "-r", "--name-only", "HEAD"], cwd=repo_path)
    return [f for f in result.stdout.strip().split("\n") if f and should_index_file(f)]


def read_file_at_head(repo_path: str, file_path: str) -> bytes | None:
    """Read file content at HEAD via git show."""
    try:
        result = subprocess.run(
            ["git", "show", f"HEAD:{file_path}"],
            cwd=repo_path,
            check=True,
            capture_output=True,
        )
        return result.stdout
    except subprocess.CalledProcessError:
        return None


def get_commits(repo_path: str, n: int) -> list[dict]:
    """Return the last N commits with metadata."""
    # Format: hash|short_hash|author_name|author_email|date|subject|||body
    sep = "\x1f"
    fmt = sep.join(["%H", "%h", "%an", "%ae", "%aI", "%s", "%b"])
    result = run_git(
        ["log", f"-{n}", f"--format={fmt}"],
        cwd=repo_path,
    )
    commits = []
    for line in result.stdout.split("\n"):
        if not line.strip():
            continue
        parts = line.split(sep)
        if len(parts) < 6:
            continue
        commits.append({
            "hash":         parts[0].strip(),
            "short_hash":   parts[1].strip(),
            "author_name":  parts[2].strip(),
            "author_email": parts[3].strip(),
            "date":         parts[4].strip(),
            "subject":      parts[5].strip(),
            "body":         parts[6].strip() if len(parts) > 6 else "",
        })
    return commits


def get_commit_stats(repo_path: str, commit_hash: str) -> dict:
    """Return stats (files_changed, insertions, deletions) for a commit."""
    result = run_git(
        ["show", "--stat", "--format=", commit_hash],
        cwd=repo_path,
        check=False,
    )
    files_changed = insertions = deletions = 0
    # Last non-empty line: " 3 files changed, 42 insertions(+), 7 deletions(-)"
    for line in reversed(result.stdout.strip().split("\n")):
        line = line.strip()
        if "file" in line and "changed" in line:
            import re
            m = re.search(r"(\d+) file", line)
            if m:
                files_changed = int(m.group(1))
            m = re.search(r"(\d+) insertion", line)
            if m:
                insertions = int(m.group(1))
            m = re.search(r"(\d+) deletion", line)
            if m:
                deletions = int(m.group(1))
            break
    return {"files_changed": files_changed, "insertions": insertions, "deletions": deletions}


def get_commit_files(repo_path: str, commit_hash: str) -> list[dict]:
    """Return list of files changed in a commit with change type and line counts."""
    result = run_git(
        ["show", "--numstat", "--format=", commit_hash],
        cwd=repo_path,
        check=False,
    )
    changed = []
    for line in result.stdout.strip().split("\n"):
        if not line.strip():
            continue
        parts = line.split("\t")
        if len(parts) < 3:
            continue
        additions_str, deletions_str, path = parts[0], parts[1], parts[2]
        # Binary files show "-" for counts
        additions = int(additions_str) if additions_str.isdigit() else 0
        deletions = int(deletions_str) if deletions_str.isdigit() else 0
        # Handle renames: "old_path => new_path" or just new path
        if " => " in path:
            path = path.split(" => ")[-1].strip("}")
        changed.append({
            "path":      path.strip(),
            "additions": additions,
            "deletions": deletions,
        })
    # Determine change type per file via --diff-filter
    result2 = run_git(
        ["show", "--name-status", "--format=", commit_hash],
        cwd=repo_path,
        check=False,
    )
    type_map: dict[str, str] = {}
    for line in result2.stdout.strip().split("\n"):
        if not line.strip():
            continue
        parts = line.split("\t")
        if len(parts) >= 2:
            change_type = parts[0][0]  # A/M/D/R/C
            path = parts[-1].strip()
            type_map[path] = change_type

    for entry in changed:
        entry["change_type"] = type_map.get(entry["path"], "M")

    return changed


# ---------------------------------------------------------------------------
# Indexing
# ---------------------------------------------------------------------------


def index_repo_files(
    cursor: sqlite3.Cursor,
    repo_name: str,
    repo_path: str,
) -> int:
    """Index all files at HEAD for a repo. Returns count of indexed files."""
    print(f"  [{repo_name}] Indexing files at HEAD...")
    files = get_head_files(repo_path)
    print(f"  [{repo_name}] Found {len(files)} eligible files")

    indexed = 0
    skipped_binary = 0
    skipped_size = 0

    for file_path in files:
        raw = read_file_at_head(repo_path, file_path)
        if raw is None:
            continue

        # Skip binary
        if b"\x00" in raw[:8192]:
            skipped_binary += 1
            continue

        # Skip oversized
        if len(raw) > MAX_FILE_SIZE:
            skipped_size += 1
            continue

        try:
            content = raw.decode("utf-8", errors="replace")
        except Exception:
            skipped_binary += 1
            continue

        language = detect_language(file_path)

        cursor.execute(
            "INSERT OR REPLACE INTO files (repo, path, content, language, size) VALUES (?,?,?,?,?)",
            (repo_name, file_path, content, language, len(raw)),
        )
        indexed += 1

    print(
        f"  [{repo_name}] Files indexed: {indexed}  "
        f"(skipped binary: {skipped_binary}, oversized: {skipped_size})"
    )
    return indexed


def index_repo_commits(
    cursor: sqlite3.Cursor,
    repo_name: str,
    repo_path: str,
    num_commits: int,
) -> int:
    """Index recent commits. Returns count of commits indexed."""
    print(f"  [{repo_name}] Indexing last {num_commits} commits...")
    commits = get_commits(repo_path, num_commits)
    print(f"  [{repo_name}] Retrieved {len(commits)} commits")

    for commit in commits:
        stats = get_commit_stats(repo_path, commit["hash"])
        cursor.execute(
            """INSERT OR REPLACE INTO commits
               (repo, hash, short_hash, author_name, author_email, date,
                subject, body, files_changed, insertions, deletions)
               VALUES (?,?,?,?,?,?,?,?,?,?,?)""",
            (
                repo_name,
                commit["hash"],
                commit["short_hash"],
                commit["author_name"],
                commit["author_email"],
                commit["date"],
                commit["subject"],
                commit["body"],
                stats["files_changed"],
                stats["insertions"],
                stats["deletions"],
            ),
        )
        commit_id = cursor.lastrowid

        changed_files = get_commit_files(repo_path, commit["hash"])
        for cf in changed_files:
            cursor.execute(
                """INSERT INTO commit_files
                   (commit_id, repo, path, change_type, additions, deletions)
                   VALUES (?,?,?,?,?,?)""",
                (
                    commit_id,
                    repo_name,
                    cf["path"],
                    cf["change_type"],
                    cf["additions"],
                    cf["deletions"],
                ),
            )

    print(f"  [{repo_name}] Commits indexed: {len(commits)}")
    return len(commits)


# ---------------------------------------------------------------------------
# Clone / local-repo helpers
# ---------------------------------------------------------------------------


def ensure_repo(
    repo_name: str,
    repo_url: str,
    work_dir: Path,
    github_token: str = "",
) -> str:
    """Clone (or fetch-update) a bare repo and return its path."""
    dest = work_dir / f"{repo_name}.git"
    if github_token:
        # Inject token into URL for private repos
        url_with_token = repo_url.replace("https://", f"https://{github_token}@")
    else:
        url_with_token = repo_url

    if dest.exists():
        print(f"  [{repo_name}] Fetching updates in {dest}...")
        subprocess.run(
            ["git", "fetch", "--all", "--tags", "--prune"],
            cwd=str(dest),
            check=True,
            capture_output=True,
        )
    else:
        print(f"  [{repo_name}] Cloning {repo_url} -> {dest}...")
        dest.parent.mkdir(parents=True, exist_ok=True)
        subprocess.run(
            ["git", "clone", "--bare", url_with_token, str(dest)],
            check=True,
            capture_output=True,
        )
    return str(dest)


def get_local_repo(repo_name: str) -> str | None:
    """Return path to local checkout if it exists and is a git repo."""
    local = LOCAL_REPOS.get(repo_name)
    if local and local.exists() and (local / ".git").exists():
        return str(local)
    return None


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------


def generate(
    db_path: Path,
    num_commits: int = DEFAULT_COMMITS,
    use_local: bool = False,
    clone_dir: Path | None = None,
    github_token: str = "",
) -> None:
    """Generate the repo SQLite database."""
    db_path.parent.mkdir(parents=True, exist_ok=True)

    # Remove existing DB for a clean build
    if db_path.exists():
        db_path.unlink()
        print(f"Removed existing database at {db_path}")

    conn = sqlite3.connect(db_path)
    cursor = conn.cursor()
    cursor.execute("PRAGMA journal_mode=WAL")
    cursor.execute("PRAGMA synchronous=NORMAL")

    print("Creating schema...")
    create_schema(cursor)
    conn.commit()

    total_files = 0
    total_commits = 0

    with tempfile.TemporaryDirectory() as tmp:
        work_dir = clone_dir or Path(tmp)

        for repo_name, repo_url in REPOS.items():
            print(f"\n=== Processing repo: {repo_name} ===")

            # Resolve which repo path to use
            if use_local:
                repo_path = get_local_repo(repo_name)
                if repo_path:
                    print(f"  [{repo_name}] Using local checkout at {repo_path}")
                else:
                    print(f"  [{repo_name}] Local checkout not found, cloning instead...")
                    repo_path = ensure_repo(repo_name, repo_url, work_dir, github_token)
            else:
                repo_path = ensure_repo(repo_name, repo_url, work_dir, github_token)

            total_files += index_repo_files(cursor, repo_name, repo_path)
            conn.commit()

            total_commits += index_repo_commits(cursor, repo_name, repo_path, num_commits)
            conn.commit()

    # Write metadata
    import datetime

    cursor.execute(
        "INSERT INTO meta (key, value) VALUES (?,?)",
        ("generated_at", datetime.datetime.utcnow().isoformat() + "Z"),
    )
    cursor.execute("INSERT INTO meta (key, value) VALUES (?,?)", ("num_commits", str(num_commits)))
    cursor.execute(
        "INSERT INTO meta (key, value) VALUES (?,?)",
        ("repos", ",".join(REPOS.keys())),
    )
    conn.commit()

    # Report
    cursor.execute("SELECT COUNT(*) FROM files")
    file_count = cursor.fetchone()[0]
    cursor.execute("SELECT COUNT(*) FROM commits")
    commit_count = cursor.fetchone()[0]
    cursor.execute("SELECT COUNT(*) FROM commit_files")
    cf_count = cursor.fetchone()[0]

    size_mb = db_path.stat().st_size / 1_048_576

    conn.close()

    print(f"\n{'=' * 50}")
    print(f"Database written to: {db_path}")
    print(f"  files:        {file_count:>6}")
    print(f"  commits:      {commit_count:>6}")
    print(f"  commit_files: {cf_count:>6}")
    print(f"  size:         {size_mb:.1f} MB")


def main() -> None:
    parser = argparse.ArgumentParser(description="Generate repo SQLite database")
    parser.add_argument(
        "--commits", type=int, default=DEFAULT_COMMITS,
        help=f"Number of recent commits to index per repo (default: {DEFAULT_COMMITS})",
    )
    parser.add_argument(
        "--output", type=Path, default=DEFAULT_DB_PATH,
        help=f"Output SQLite path (default: {DEFAULT_DB_PATH})",
    )
    parser.add_argument(
        "--local", action="store_true",
        help="Use local repo checkouts when available instead of cloning",
    )
    parser.add_argument(
        "--clone-dir", type=Path, default=None,
        help="Directory to cache bare clones in (default: temp dir, deleted after run)",
    )
    parser.add_argument(
        "--token", type=str, default=os.environ.get("GITHUB_TOKEN", ""),
        help="GitHub token for private repos (or set GITHUB_TOKEN env var)",
    )
    args = parser.parse_args()

    print(f"Generating repo database")
    print(f"  output:   {args.output}")
    print(f"  commits:  {args.commits} per repo")
    print(f"  mode:     {'local+fallback' if args.local else 'clone'}")
    print()

    generate(
        db_path=args.output,
        num_commits=args.commits,
        use_local=args.local,
        clone_dir=args.clone_dir,
        github_token=args.token,
    )


if __name__ == "__main__":
    main()
