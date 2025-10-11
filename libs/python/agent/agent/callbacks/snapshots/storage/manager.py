"""
Core storage management for snapshot metadata.
"""

from pathlib import Path
from typing import Dict, Any, Optional
import logging

from .file_ops import FileOperations
from .path_utils import PathUtilities

logger = logging.getLogger(__name__)


class StorageManager:
    """
    Manages file I/O operations for snapshot metadata.
    """

    def __init__(self, metadata_dir: str = "/tmp/cua_snapshots"):
        """
        Initialize the storage manager.

        Args:
            metadata_dir: Directory to store metadata files
        """
        self.metadata_dir = Path(metadata_dir)
        self.path_utils = PathUtilities(self.metadata_dir)
        self.file_ops = FileOperations()

        self.ensure_directory()

    def ensure_directory(self) -> None:
        """Ensure the metadata directory exists."""
        try:
            self.metadata_dir.mkdir(parents=True, exist_ok=True)
            logger.debug(f"Ensured metadata directory exists: {self.metadata_dir}")
        except Exception as e:
            logger.error(f"Failed to create metadata directory: {e}")
            self.metadata_dir = Path("/tmp/cua_snapshots_fallback")
            self.metadata_dir.mkdir(parents=True, exist_ok=True)
            logger.warning(f"Using fallback directory: {self.metadata_dir}")
            self.path_utils.metadata_dir = self.metadata_dir

    def get_metadata_path(self, container_name: str) -> Path:
        """Get the path to the metadata file for a container."""
        return self.path_utils.get_metadata_path(container_name)

    def read_json_file(self, file_path: Path) -> Optional[Dict[str, Any]]:
        """Safely read a JSON file."""
        return self.file_ops.read_json(file_path)

    def write_json_file(self, file_path: Path, data: Dict[str, Any]) -> bool:
        """Safely write data to a JSON file."""
        return self.file_ops.write_json(file_path, data)

    def delete_metadata_file(self, container_name: str) -> bool:
        """Delete the metadata file for a container."""
        file_path = self.get_metadata_path(container_name)

        if not file_path.exists():
            return True

        try:
            file_path.unlink()
            logger.info(f"Deleted metadata file: {file_path}")
            return True
        except Exception as e:
            logger.error(f"Failed to delete {file_path}: {e}")
            return False

    def list_metadata_files(self) -> list[Path]:
        """List all metadata files in the storage directory."""
        return self.path_utils.list_metadata_files()

    def get_storage_info(self) -> Dict[str, Any]:
        """Get information about the storage system."""
        return self.path_utils.get_storage_info()

    def cleanup_old_metadata(self, days: int = 30) -> int:
        """Clean up metadata files older than specified days."""
        return self.path_utils.cleanup_old_files(days)