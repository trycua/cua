"""
Computer tracing functionality for recording sessions.

This module provides a Computer.tracing API inspired by Playwright's tracing functionality,
allowing users to record computer interactions for debugging, training, and analysis.
"""

import asyncio
import base64
import io
import json
import time
import uuid
import zipfile
from datetime import datetime
from pathlib import Path
from typing import Any, Dict, List, Optional, Union

from PIL import Image


class ComputerTracing:
    """
    Computer tracing class that records computer interactions and saves them to disk.

    This class provides a flexible API for recording computer sessions with configurable
    options for what to record (screenshots, API calls, video, etc.).
    """

    def __init__(self, computer_instance):
        """
        Initialize the tracing instance.

        Args:
            computer_instance: The Computer instance to trace
        """
        self._computer = computer_instance
        self._is_tracing = False
        self._trace_config: Dict[str, Any] = {}
        self._trace_data: List[Dict[str, Any]] = []
        self._trace_start_time: Optional[float] = None
        self._trace_id: Optional[str] = None
        self._trace_dir: Optional[Path] = None
        self._screenshot_count = 0

    @property
    def is_tracing(self) -> bool:
        """Check if tracing is currently active."""
        return self._is_tracing

    async def start(self, config: Optional[Dict[str, Any]] = None) -> None:
        """
        Start tracing with the specified configuration.

        Args:
            config: Tracing configuration dict with options:
                - video: bool - Record video frames (default: False)
                - screenshots: bool - Record screenshots (default: True)
                - api_calls: bool - Record API calls and results (default: True)
                - accessibility_tree: bool - Record accessibility tree snapshots (default: False)
                - metadata: bool - Record custom metadata (default: True)
                - name: str - Custom trace name (default: auto-generated)
                - path: str - Custom trace directory path (default: auto-generated)
        """
        if self._is_tracing:
            raise RuntimeError("Tracing is already active. Call stop() first.")

        # Set default configuration
        default_config = {
            "video": False,
            "screenshots": True,
            "api_calls": True,
            "accessibility_tree": False,
            "metadata": True,
            "name": None,
            "path": None,
        }

        self._trace_config = {**default_config, **(config or {})}

        # Generate trace ID and directory
        timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
        self._trace_id = (
            self._trace_config.get("name") or f"trace_{timestamp}_{str(uuid.uuid4())[:8]}"
        )

        if self._trace_config.get("path"):
            self._trace_dir = Path(self._trace_config["path"])
        else:
            self._trace_dir = Path.cwd() / "traces" / self._trace_id

        # Create trace directory
        self._trace_dir.mkdir(parents=True, exist_ok=True)

        # Initialize trace data
        self._trace_data = []
        self._trace_start_time = time.time()
        self._screenshot_count = 0
        self._is_tracing = True

        # Record initial metadata
        await self._record_event(
            "trace_start",
            {
                "trace_id": self._trace_id,
                "config": self._trace_config,
                "timestamp": self._trace_start_time,
                "computer_info": {
                    "os_type": self._computer.os_type,
                    "provider_type": str(self._computer.provider_type),
                    "image": self._computer.image,
                },
            },
        )

        # Take initial screenshot if enabled
        if self._trace_config.get("screenshots"):
            await self._take_screenshot("initial_screenshot")

    async def stop(self, options: Optional[Dict[str, Any]] = None) -> str:
        """
        Stop tracing and save the trace data.

        Args:
            options: Stop options dict with:
                - path: str - Custom output path for the trace archive
                - format: str - Output format ('zip' or 'dir', default: 'zip')

        Returns:
            str: Path to the saved trace file or directory
        """
        if not self._is_tracing:
            raise RuntimeError("Tracing is not active. Call start() first.")

        if self._trace_start_time is None or self._trace_dir is None or self._trace_id is None:
            raise RuntimeError("Tracing state is invalid.")

        # Record final metadata
        await self._record_event(
            "trace_end",
            {
                "timestamp": time.time(),
                "duration": time.time() - self._trace_start_time,
                "total_events": len(self._trace_data),
                "screenshot_count": self._screenshot_count,
            },
        )

        # Take final screenshot if enabled
        if self._trace_config.get("screenshots"):
            await self._take_screenshot("final_screenshot")

        # Save trace metadata
        metadata_path = self._trace_dir / "trace_metadata.json"
        with open(metadata_path, "w") as f:
            json.dump(
                {
                    "trace_id": self._trace_id,
                    "config": self._trace_config,
                    "start_time": self._trace_start_time,
                    "end_time": time.time(),
                    "duration": time.time() - self._trace_start_time,
                    "total_events": len(self._trace_data),
                    "screenshot_count": self._screenshot_count,
                    "events": self._trace_data,
                },
                f,
                indent=2,
                default=str,
            )

        # Determine output format and path
        output_format = options.get("format", "zip") if options else "zip"
        custom_path = options.get("path") if options else None

        if output_format == "zip":
            # Create zip file
            if custom_path:
                zip_path = Path(custom_path).resolve()
                # Check if path exists as a directory
                if zip_path.exists() and zip_path.is_dir():
                    raise ValueError(
                        f"Cannot create zip file at '{custom_path}': path exists as a directory. "
                        "Please specify a file path (e.g., 'traces/my_trace.zip') or use format='dir'."
                    )
            else:
                zip_path = self._trace_dir.parent / f"{self._trace_id}.zip"

            await self._create_zip_archive(zip_path)
            output_path = str(zip_path)
        else:
            # Return directory path
            if custom_path:
                # Move directory to custom path
                custom_dir = Path(custom_path).resolve()
                trace_dir_resolved = self._trace_dir.resolve()

                # Check if custom_dir is an ancestor of the trace directory
                is_subdirectory = False
                try:
                    trace_dir_resolved.relative_to(custom_dir)
                    is_subdirectory = True
                except ValueError:
                    # relative_to raises ValueError if not a subdirectory, which is what we want
                    pass

                if is_subdirectory:
                    raise ValueError(
                        f"Cannot move trace directory to '{custom_path}': it would be moved into itself. "
                        f"Trace is currently at '{self._trace_dir}'. "
                        "Please specify a different target directory."
                    )

                if custom_dir.exists():
                    import shutil

                    shutil.rmtree(custom_dir)
                self._trace_dir.rename(custom_dir)
                output_path = str(custom_dir)
            else:
                output_path = str(self._trace_dir)

        # Reset tracing state
        self._is_tracing = False
        self._trace_config = {}
        self._trace_data = []
        self._trace_start_time = None
        self._trace_id = None
        self._screenshot_count = 0

        return output_path

    async def _record_event(self, event_type: str, data: Dict[str, Any]) -> None:
        """
        Record a trace event.

        Args:
            event_type: Type of event (e.g., 'click', 'type', 'screenshot')
            data: Event data
        """
        if not self._is_tracing or self._trace_start_time is None or self._trace_dir is None:
            return

        event = {
            "type": event_type,
            "timestamp": time.time(),
            "relative_time": time.time() - self._trace_start_time,
            "data": data,
        }

        self._trace_data.append(event)

        # Save event to individual file for large traces
        event_file = self._trace_dir / f"event_{len(self._trace_data):06d}_{event_type}.json"
        with open(event_file, "w") as f:
            json.dump(event, f, indent=2, default=str)

    async def _take_screenshot(self, name: str = "screenshot") -> Optional[str]:
        """
        Take a screenshot and save it to the trace.

        Args:
            name: Name for the screenshot

        Returns:
            Optional[str]: Path to the saved screenshot, or None if screenshots disabled
        """
        if (
            not self._trace_config.get("screenshots")
            or not self._computer.interface
            or self._trace_dir is None
        ):
            return None

        try:
            screenshot_bytes = await self._computer.interface.screenshot()
            self._screenshot_count += 1

            screenshot_filename = f"{self._screenshot_count:06d}_{name}.png"
            screenshot_path = self._trace_dir / screenshot_filename

            with open(screenshot_path, "wb") as f:
                f.write(screenshot_bytes)

            return str(screenshot_path)
        except Exception as e:
            # Log error but don't fail the trace
            if hasattr(self._computer, "logger"):
                self._computer.logger.warning(f"Failed to take screenshot: {e}")
            return None

    async def _create_zip_archive(self, zip_path: Path) -> None:
        """
        Create a zip archive of the trace directory.

        Args:
            zip_path: Path where to save the zip file
        """
        if self._trace_dir is None:
            raise RuntimeError("Trace directory is not set")

        with zipfile.ZipFile(zip_path, "w", zipfile.ZIP_DEFLATED) as zipf:
            for file_path in self._trace_dir.rglob("*"):
                if file_path.is_file():
                    arcname = file_path.relative_to(self._trace_dir)
                    zipf.write(file_path, arcname)

    async def record_api_call(
        self,
        method: str,
        args: Dict[str, Any],
        result: Any = None,
        error: Optional[Exception] = None,
    ) -> None:
        """
        Record an API call event.

        Args:
            method: The method name that was called
            args: Arguments passed to the method
            result: Result returned by the method
            error: Exception raised by the method, if any
        """
        if not self._trace_config.get("api_calls"):
            return

        # Take screenshot after certain actions if enabled
        screenshot_path = None
        screenshot_actions = [
            "left_click",
            "right_click",
            "double_click",
            "type_text",
            "press_key",
            "hotkey",
        ]
        if method in screenshot_actions and self._trace_config.get("screenshots"):
            screenshot_path = await self._take_screenshot(f"after_{method}")

        # Record accessibility tree after certain actions if enabled
        if method in screenshot_actions and self._trace_config.get("accessibility_tree"):
            await self.record_accessibility_tree()

        await self._record_event(
            "api_call",
            {
                "method": method,
                "args": args,
                "result": str(result) if result is not None else None,
                "error": str(error) if error else None,
                "screenshot": screenshot_path,
                "success": error is None,
            },
        )

    async def record_accessibility_tree(self) -> None:
        """Record the current accessibility tree if enabled."""
        if not self._trace_config.get("accessibility_tree") or not self._computer.interface:
            return

        try:
            accessibility_tree = await self._computer.interface.get_accessibility_tree()
            await self._record_event("accessibility_tree", {"tree": accessibility_tree})
        except Exception as e:
            if hasattr(self._computer, "logger"):
                self._computer.logger.warning(f"Failed to record accessibility tree: {e}")

    async def add_metadata(self, key: str, value: Any) -> None:
        """
        Add custom metadata to the trace.

        Args:
            key: Metadata key
            value: Metadata value
        """
        if not self._trace_config.get("metadata"):
            return

        await self._record_event("metadata", {"key": key, "value": value})
