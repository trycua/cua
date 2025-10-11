"""
File I/O operations for JSON data.
"""

import json
from pathlib import Path
from typing import Optional, Dict, Any
import logging

logger = logging.getLogger(__name__)


class FileOperations:
    """
    Handles file reading and writing operations.
    """

    def read_json(self, file_path: Path) -> Optional[Dict[str, Any]]:
        """
        Safely read a JSON file.

        Args:
            file_path: Path to the JSON file

        Returns:
            Parsed JSON data or None if file doesn't exist or is invalid
        """
        if not file_path.exists():
            logger.debug(f"File does not exist: {file_path}")
            return None

        try:
            with open(file_path, 'r') as f:
                data = json.load(f)
                logger.debug(f"Successfully read JSON from {file_path}")
                return data
        except json.JSONDecodeError as e:
            logger.error(f"Invalid JSON in {file_path}: {e}")
            self._backup_corrupted_file(file_path)
            return None
        except IOError as e:
            logger.error(f"Failed to read {file_path}: {e}")
            return None

    def write_json(self, file_path: Path, data: Dict[str, Any]) -> bool:
        """
        Safely write data to a JSON file.

        Args:
            file_path: Path to the JSON file
            data: Data to write

        Returns:
            True if successful, False otherwise
        """
        try:
            temp_path = file_path.with_suffix('.tmp')

            with open(temp_path, 'w') as f:
                json.dump(data, f, indent=2)

            temp_path.replace(file_path)

            logger.debug(f"Successfully wrote JSON to {file_path}")
            return True

        except IOError as e:
            logger.error(f"Failed to write {file_path}: {e}")
            return False
        except Exception as e:
            logger.error(f"Unexpected error writing {file_path}: {e}")
            return False

    def _backup_corrupted_file(self, file_path: Path) -> None:
        """
        Backup a corrupted file.

        Args:
            file_path: Path to the corrupted file
        """
        try:
            backup_path = file_path.with_suffix('.corrupted')
            file_path.rename(backup_path)
            logger.info(f"Backed up corrupted file to {backup_path}")
        except Exception as e:
            logger.error(f"Failed to backup corrupted file: {e}")