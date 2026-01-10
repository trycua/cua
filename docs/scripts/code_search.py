"""
Code search functions for the CUA code index
Provides semantic and full-text search over versioned source code
"""

import sqlite3
from pathlib import Path
from typing import Optional

import lancedb

# Default paths (can be overridden for Modal deployment)
DEFAULT_CODE_DB_PATH = Path(__file__).parent.parent / "code_db"
DEFAULT_SQLITE_PATH = DEFAULT_CODE_DB_PATH / "code_index.sqlite"
DEFAULT_LANCEDB_PATH = DEFAULT_CODE_DB_PATH / "code_index.lancedb"


class CodeSearchError(Exception):
    """Custom exception for code search errors"""

    pass


class CodeSearch:
    """
    Search interface for the versioned code index.

    Provides semantic search (via LanceDB) and full-text search (via SQLite FTS5)
    over source code files across all indexed versions.
    """

    def __init__(
        self,
        sqlite_path: Optional[Path] = None,
        lancedb_path: Optional[Path] = None,
    ):
        self.sqlite_path = sqlite_path or DEFAULT_SQLITE_PATH
        self.lancedb_path = lancedb_path or DEFAULT_LANCEDB_PATH
        self._lance_db = None
        self._components_cache = None
        self._versions_cache = {}

    def _get_sqlite_conn(self) -> sqlite3.Connection:
        """Get a SQLite connection"""
        if not self.sqlite_path.exists():
            raise CodeSearchError(
                f"SQLite database not found at {self.sqlite_path}. Run generate_code_index.py first."
            )
        return sqlite3.connect(self.sqlite_path)

    def _get_lance_table(self):
        """Get the LanceDB table"""
        if self._lance_db is None:
            if not self.lancedb_path.exists():
                raise CodeSearchError(
                    f"LanceDB not found at {self.lancedb_path}. Run generate_code_index.py first."
                )
            self._lance_db = lancedb.connect(self.lancedb_path)
        return self._lance_db.open_table("code")

    def _validate_component(self, component: str):
        """Validate that a component exists"""
        components = self.list_components()
        component_names = [c["component"] for c in components]
        if component not in component_names:
            raise CodeSearchError(
                f"Unknown component: '{component}'. "
                f"Available components: {', '.join(component_names)}. "
                "Use list_components() to see all options."
            )

    def _validate_version(self, component: str, version: str):
        """Validate that a version exists for a component"""
        self._validate_component(component)
        versions = self.list_versions(component)
        if version not in versions:
            raise CodeSearchError(
                f"Unknown version '{version}' for component '{component}'. "
                f"Available versions: {', '.join(versions[:10])}{'...' if len(versions) > 10 else ''}. "
                "Use list_versions(component) to see all options."
            )

    def list_components(self) -> list[dict]:
        """
        List all indexed components with their version counts.

        Returns:
            List of dicts with 'component' and 'version_count' keys
        """
        if self._components_cache is not None:
            return self._components_cache

        conn = self._get_sqlite_conn()
        cursor = conn.cursor()

        cursor.execute(
            """
            SELECT component, COUNT(DISTINCT version) as version_count
            FROM code_files
            GROUP BY component
            ORDER BY component
        """
        )

        results = [
            {"component": row[0], "version_count": row[1]} for row in cursor.fetchall()
        ]
        conn.close()

        self._components_cache = results
        return results

    def list_versions(self, component: str) -> list[str]:
        """
        List all indexed versions for a component, sorted by semver descending.

        Args:
            component: The component name (e.g., "agent", "computer")

        Returns:
            List of version strings, newest first
        """
        if component in self._versions_cache:
            return self._versions_cache[component]

        self._validate_component(component)

        conn = self._get_sqlite_conn()
        cursor = conn.cursor()

        cursor.execute(
            """
            SELECT DISTINCT version
            FROM code_files
            WHERE component = ?
        """,
            (component,),
        )

        versions = [row[0] for row in cursor.fetchall()]
        conn.close()

        # Sort by semver (descending)
        def semver_key(v: str) -> tuple:
            try:
                parts = v.split(".")
                # Extract numeric part before any non-numeric characters (e.g., "3-alpha" -> 3)
                result = []
                for p in parts[:3]:
                    numeric_part = p.split('-')[0]
                    result.append(int(numeric_part))
                return tuple(result)
            except (ValueError, IndexError):
                return (0, 0, 0)

        versions.sort(key=semver_key, reverse=True)

        self._versions_cache[component] = versions
        return versions

    def list_files(self, component: str, version: str) -> list[dict]:
        """
        List all source files for a component@version.

        Args:
            component: The component name
            version: The version string

        Returns:
            List of dicts with 'path' and 'language' keys
        """
        self._validate_version(component, version)

        conn = self._get_sqlite_conn()
        cursor = conn.cursor()

        cursor.execute(
            """
            SELECT file_path, language
            FROM code_files
            WHERE component = ? AND version = ?
            ORDER BY file_path
        """,
            (component, version),
        )

        results = [
            {"path": row[0], "language": row[1]} for row in cursor.fetchall()
        ]
        conn.close()

        return results

    def search_code(
        self, query: str, component: str, version: str, limit: int = 10
    ) -> list[dict]:
        """
        Semantic search over source code for a specific component@version.

        Uses LanceDB vector embeddings to find semantically similar code.

        Args:
            query: Natural language search query
            component: The component name
            version: The version string
            limit: Maximum number of results (default 10)

        Returns:
            List of dicts with 'file_path', 'content', 'language', and 'score' keys
        """
        self._validate_version(component, version)

        table = self._get_lance_table()

        # LanceDB where clause
        where_clause = f"component = '{component}' AND version = '{version}'"

        results = table.search(query).where(where_clause).limit(limit).to_list()

        return [
            {
                "file_path": r["file_path"],
                "content": r["text"],
                "language": r["language"],
                "score": round(1 - r.get("_distance", 0), 4),  # Convert distance to similarity
            }
            for r in results
        ]

    def search_code_fts(
        self,
        query: str,
        component: str,
        version: str,
        limit: int = 10,
        highlight: bool = True,
    ) -> list[dict]:
        """
        Full-text search over source code for a specific component@version.

        Uses SQLite FTS5 for fast keyword/phrase matching.

        Args:
            query: Search query (supports FTS5 syntax: AND, OR, NOT, "phrases")
            component: The component name
            version: The version string
            limit: Maximum number of results (default 10)
            highlight: Whether to include highlighted snippets (default True)

        Returns:
            List of dicts with 'file_path', 'snippet' or 'content', 'language', and 'score' keys
        """
        self._validate_version(component, version)

        conn = self._get_sqlite_conn()
        cursor = conn.cursor()

        if highlight:
            cursor.execute(
                """
                SELECT
                    f.file_path,
                    snippet(code_files_fts, 0, '>>>', '<<<', '...', 64) as snippet,
                    f.language,
                    code_files_fts.rank as score
                FROM code_files_fts
                JOIN code_files f ON code_files_fts.rowid = f.id
                WHERE code_files_fts MATCH ?
                  AND code_files_fts.component = ?
                  AND code_files_fts.version = ?
                ORDER BY rank
                LIMIT ?
            """,
                (query, component, version, limit),
            )

            results = [
                {
                    "file_path": row[0],
                    "snippet": row[1],
                    "language": row[2],
                    "score": abs(row[3]) if row[3] else 0,
                }
                for row in cursor.fetchall()
            ]
        else:
            cursor.execute(
                """
                SELECT
                    f.file_path,
                    f.content,
                    f.language,
                    code_files_fts.rank as score
                FROM code_files_fts
                JOIN code_files f ON code_files_fts.rowid = f.id
                WHERE code_files_fts MATCH ?
                  AND code_files_fts.component = ?
                  AND code_files_fts.version = ?
                ORDER BY rank
                LIMIT ?
            """,
                (query, component, version, limit),
            )

            results = [
                {
                    "file_path": row[0],
                    "content": row[1],
                    "language": row[2],
                    "score": abs(row[3]) if row[3] else 0,
                }
                for row in cursor.fetchall()
            ]

        conn.close()
        return results

    def get_file_content(self, component: str, version: str, file_path: str) -> dict:
        """
        Get full content of a specific file at component@version.

        Args:
            component: The component name
            version: The version string
            file_path: Path to the file within the repository

        Returns:
            Dict with 'content', 'language', and 'lines' keys
        """
        self._validate_version(component, version)

        conn = self._get_sqlite_conn()
        cursor = conn.cursor()

        cursor.execute(
            """
            SELECT content, language
            FROM code_files
            WHERE component = ? AND version = ? AND file_path = ?
        """,
            (component, version, file_path),
        )

        row = cursor.fetchone()
        conn.close()

        if row is None:
            raise CodeSearchError(
                f"File not found: '{file_path}' in {component}@{version}. "
                "Use list_files(component, version) to see available files."
            )

        content = row[0]
        return {
            "content": content,
            "language": row[1],
            "lines": content.count("\n") + 1,
        }

    def get_stats(self) -> dict:
        """
        Get statistics about the code index.

        Returns:
            Dict with index statistics
        """
        conn = self._get_sqlite_conn()
        cursor = conn.cursor()

        cursor.execute("SELECT COUNT(*) FROM code_files")
        total_files = cursor.fetchone()[0]

        cursor.execute("SELECT COUNT(DISTINCT component) FROM code_files")
        total_components = cursor.fetchone()[0]

        cursor.execute(
            "SELECT COUNT(DISTINCT component || '@' || version) FROM code_files"
        )
        total_versions = cursor.fetchone()[0]

        cursor.execute(
            """
            SELECT component, COUNT(DISTINCT version) as versions, COUNT(*) as files
            FROM code_files
            GROUP BY component
            ORDER BY component
        """
        )
        by_component = [
            {"component": row[0], "versions": row[1], "files": row[2]}
            for row in cursor.fetchall()
        ]

        conn.close()

        return {
            "total_files": total_files,
            "total_components": total_components,
            "total_component_versions": total_versions,
            "by_component": by_component,
        }


# Singleton instance for convenience
_search_instance: Optional[CodeSearch] = None


def get_code_search(
    sqlite_path: Optional[Path] = None,
    lancedb_path: Optional[Path] = None,
) -> CodeSearch:
    """Get or create a CodeSearch instance"""
    global _search_instance
    if _search_instance is None or sqlite_path or lancedb_path:
        _search_instance = CodeSearch(sqlite_path, lancedb_path)
    return _search_instance
