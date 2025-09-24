"""
Path utilities and storage information.
"""

from pathlib import Path
from typing import Dict, Any
from datetime import datetime, timedelta
import logging

logger = logging.getLogger(__name__)


class PathUtilities:
    """
    Handles path operations and storage utilities.
    """

    def __init__(self, metadata_dir: Path):
        """
        Initialize path utilities.

        Args:
            metadata_dir: Base directory for metadata storage
        """
        self.metadata_dir = metadata_dir

    def get_metadata_path(self, container_name: str) -> Path:
        """
        Get the path to the metadata file for a container.

        Args:
            container_name: Name of the container

        Returns:
            Path to the metadata file
        """
        safe_name = self._sanitize_filename(container_name)
        return self.metadata_dir / f"{safe_name}_snapshots.json"

    def list_metadata_files(self) -> list[Path]:
        """
        List all metadata files in the storage directory.

        Returns:
            List of paths to metadata files
        """
        try:
            return list(self.metadata_dir.glob("*_snapshots.json"))
        except Exception as e:
            logger.error(f"Failed to list metadata files: {e}")
            return []

    def get_storage_info(self) -> Dict[str, Any]:
        """
        Get information about the storage system.

        Returns:
            Dictionary with storage information
        """
        metadata_files = self.list_metadata_files()
        total_size = sum(f.stat().st_size for f in metadata_files if f.exists())

        return {
            "metadata_dir": str(self.metadata_dir),
            "metadata_files": len(metadata_files),
            "total_size_bytes": total_size,
            "exists": self.metadata_dir.exists(),
            "writable": self._is_writable()
        }

    def cleanup_old_files(self, days: int = 30) -> int:
        """
        Clean up metadata files older than specified days.

        Args:
            days: Age threshold in days

        Returns:
            Number of files deleted
        """
        deleted_count = 0
        cutoff_time = datetime.now() - timedelta(days=days)

        for file_path in self.list_metadata_files():
            try:
                if file_path.stat().st_mtime < cutoff_time.timestamp():
                    file_path.unlink()
                    deleted_count += 1
                    logger.info(f"Deleted old metadata file: {file_path}")
            except Exception as e:
                logger.error(f"Failed to delete old file {file_path}: {e}")

        return deleted_count

    def _sanitize_filename(self, name: str) -> str:
        """Sanitize a name for use in a filename."""
        safe_chars = "".join(c if c.isalnum() or c in "-_" else "_" for c in name)
        return safe_chars[:100]

    def _is_writable(self) -> bool:
        """Check if the metadata directory is writable."""
        try:
            test_file = self.metadata_dir / ".write_test"
            test_file.touch()
            test_file.unlink()
            return True
        except Exception:
            return False