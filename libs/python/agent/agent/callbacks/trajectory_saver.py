"""
Trajectory saving callback handler for ComputerAgent.
"""

import os
import json
import uuid
from datetime import datetime
import base64
from pathlib import Path
from typing import List, Dict, Any, Optional, Union, override
from PIL import Image, ImageDraw
import io
from .base import AsyncCallbackHandler

def sanitize_image_urls(data: Any) -> Any:
    """
    Recursively search for 'image_url' keys and set their values to '[omitted]'.
    
    Args:
        data: Any data structure (dict, list, or primitive type)
        
    Returns:
        A deep copy of the data with all 'image_url' values replaced with '[omitted]'
    """
    if isinstance(data, dict):
        # Create a copy of the dictionary
        sanitized = {}
        for key, value in data.items():
            if key == "image_url":
                sanitized[key] = "[omitted]"
            else:
                # Recursively sanitize the value
                sanitized[key] = sanitize_image_urls(value)
        return sanitized
    
    elif isinstance(data, list):
        # Recursively sanitize each item in the list
        return [sanitize_image_urls(item) for item in data]
    
    else:
        # For primitive types (str, int, bool, None, etc.), return as-is
        return data


class TrajectorySaverCallback(AsyncCallbackHandler):
    """
    Callback handler that saves agent trajectories to disk.
    
    Saves each run as a separate trajectory with unique ID, and each turn
    within the trajectory gets its own folder with screenshots and responses.
    """
    
    def __init__(self, trajectory_dir: str, reset_on_run: bool = True):
        """
        Initialize trajectory saver.
        
        Args:
            trajectory_dir: Base directory to save trajectories
            reset_on_run: If True, reset trajectory_id/turn/artifact on each run.
                         If False, continue using existing trajectory_id if set.
        """
        self.trajectory_dir = Path(trajectory_dir)
        self.trajectory_id: Optional[str] = None
        self.current_turn: int = 0
        self.current_artifact: int = 0
        self.model: Optional[str] = None
        self.total_usage: Dict[str, Any] = {}
        self.reset_on_run = reset_on_run
        
        # Ensure trajectory directory exists
        self.trajectory_dir.mkdir(parents=True, exist_ok=True)
    
    def _get_turn_dir(self) -> Path:
        """Get the directory for the current turn."""
        if not self.trajectory_id:
            raise ValueError("Trajectory not initialized - call _on_run_start first")
        
        # format: trajectory_id/turn_000
        turn_dir = self.trajectory_dir / self.trajectory_id / f"turn_{self.current_turn:03d}"
        turn_dir.mkdir(parents=True, exist_ok=True)
        return turn_dir

    def _save_artifact(self, name: str, artifact: Union[str, bytes, Dict[str, Any]]) -> None:
        """Save an artifact to the current turn directory."""
        turn_dir = self._get_turn_dir()
        if isinstance(artifact, bytes):
            # format: turn_000/0000_name.png
            artifact_filename = f"{self.current_artifact:04d}_{name}"
            artifact_path = turn_dir / f"{artifact_filename}.png"
            with open(artifact_path, "wb") as f:
                f.write(artifact)
        else:
            # format: turn_000/0000_name.json
            artifact_filename = f"{self.current_artifact:04d}_{name}"
            artifact_path = turn_dir / f"{artifact_filename}.json"
            with open(artifact_path, "w") as f:
                json.dump(sanitize_image_urls(artifact), f, indent=2)
        self.current_artifact += 1

    def _update_usage(self, usage: Dict[str, Any]) -> None:
        """Update total usage statistics."""
        def add_dicts(target: Dict[str, Any], source: Dict[str, Any]) -> None:
            for key, value in source.items():
                if isinstance(value, dict):
                    if key not in target:
                        target[key] = {}
                    add_dicts(target[key], value)
                else:
                    if key not in target:
                        target[key] = 0
                    target[key] += value
        add_dicts(self.total_usage, usage)
    
    @override
    async def on_run_start(self, kwargs: Dict[str, Any], old_items: List[Dict[str, Any]]) -> None:
        """Initialize trajectory tracking for a new run."""
        model = kwargs.get("model", "unknown")
        
        # Only reset trajectory state if reset_on_run is True or no trajectory exists
        if self.reset_on_run or not self.trajectory_id:
            model_name_short = model.split("+")[-1].split("/")[-1].lower()[:16]
            if "+" in model:
                model_name_short = model.split("+")[0].lower()[:4] + "_" + model_name_short

            # id format: yyyy-mm-dd_model_hhmmss_uuid[:4]
            now = datetime.now()
            self.trajectory_id = f"{now.strftime('%Y-%m-%d')}_{model_name_short}_{now.strftime('%H%M%S')}_{str(uuid.uuid4())[:4]}"
            self.current_turn = 0
            self.current_artifact = 0
            self.model = model
            self.total_usage = {}
            
            # Create trajectory directory
            trajectory_path = self.trajectory_dir / self.trajectory_id
            trajectory_path.mkdir(parents=True, exist_ok=True)
            
            # Save trajectory metadata
            metadata = {
                "trajectory_id": self.trajectory_id,
                "created_at": str(uuid.uuid1().time),
                "status": "running",
                "kwargs": kwargs,
            }
            
            with open(trajectory_path / "metadata.json", "w") as f:
                json.dump(metadata, f, indent=2)
        else:
            # Continue with existing trajectory - just update model if needed
            self.model = model

    @override
    async def on_run_end(self, kwargs: Dict[str, Any], old_items: List[Dict[str, Any]], new_items: List[Dict[str, Any]]) -> None:
        """Finalize run tracking by updating metadata with completion status, usage, and new items."""
        if not self.trajectory_id:
            return
        
        # Update metadata with completion status, total usage, and new items
        trajectory_path = self.trajectory_dir / self.trajectory_id
        metadata_path = trajectory_path / "metadata.json"
        
        # Read existing metadata
        if metadata_path.exists():
            with open(metadata_path, "r") as f:
                metadata = json.load(f)
        else:
            metadata = {}
        
        # Update metadata with completion info
        metadata.update({
            "status": "completed",
            "completed_at": str(uuid.uuid1().time),
            "total_usage": self.total_usage,
            "new_items": sanitize_image_urls(new_items),
            "total_turns": self.current_turn
        })
        
        # Save updated metadata
        with open(metadata_path, "w") as f:
            json.dump(metadata, f, indent=2)
    
    @override 
    async def on_api_start(self, kwargs: Dict[str, Any]) -> None:
        if not self.trajectory_id:
            return
        
        self._save_artifact("api_start", { "kwargs": kwargs })
    
    @override
    async def on_api_end(self, kwargs: Dict[str, Any], result: Any) -> None:
        """Save API call result."""
        if not self.trajectory_id:
            return
        
        self._save_artifact("api_result", { "kwargs": kwargs, "result": result })

    @override
    async def on_screenshot(self, screenshot: Union[str, bytes], name: str = "screenshot") -> None:
        """Save a screenshot."""
        if isinstance(screenshot, str):
            screenshot = base64.b64decode(screenshot)
        self._save_artifact(name, screenshot)

    @override
    async def on_usage(self, usage: Dict[str, Any]) -> None:
        """Called when usage information is received."""
        self._update_usage(usage)

    @override
    async def on_responses(self, kwargs: Dict[str, Any], responses: Dict[str, Any]) -> None:
        """Save responses to the current turn directory and update usage statistics."""
        if not self.trajectory_id:
            return
        
        # Save responses
        turn_dir = self._get_turn_dir()
        response_data = {
            "timestamp": str(uuid.uuid1().time),
            "model": self.model,
            "kwargs": kwargs,
            "response": responses
        }
        
        self._save_artifact("agent_response", response_data)
        
        # Increment turn counter
        self.current_turn += 1

    def _draw_crosshair_on_image(self, image_bytes: bytes, x: int, y: int) -> bytes:
        """
        Draw a red dot and crosshair at the specified coordinates on the image.
        
        Args:
            image_bytes: The original image as bytes
            x: X coordinate for the crosshair
            y: Y coordinate for the crosshair
            
        Returns:
            Modified image as bytes with red dot and crosshair
        """
        # Open the image
        image = Image.open(io.BytesIO(image_bytes))
        draw = ImageDraw.Draw(image)
        
        # Draw crosshair lines (red, 2px thick)
        crosshair_size = 20
        line_width = 2
        color = "red"
        
        # Horizontal line
        draw.line([(x - crosshair_size, y), (x + crosshair_size, y)], fill=color, width=line_width)
        # Vertical line
        draw.line([(x, y - crosshair_size), (x, y + crosshair_size)], fill=color, width=line_width)
        
        # Draw center dot (filled circle)
        dot_radius = 3
        draw.ellipse([(x - dot_radius, y - dot_radius), (x + dot_radius, y + dot_radius)], fill=color)
        
        # Convert back to bytes
        output = io.BytesIO()
        image.save(output, format='PNG')
        return output.getvalue()

    @override
    async def on_computer_call_end(self, item: Dict[str, Any], result: List[Dict[str, Any]]) -> None:
        """
        Called when a computer call has completed.
        Saves screenshots and computer call output.
        """
        if not self.trajectory_id:
            return
        
        self._save_artifact("computer_call_result", { "item": item, "result": result })
        
        # Check if action has x/y coordinates and there's a screenshot in the result
        action = item.get("action", {})
        if "x" in action and "y" in action:
            # Look for screenshot in the result
            for result_item in result:
                if (result_item.get("type") == "computer_call_output" and 
                    result_item.get("output", {}).get("type") == "input_image"):
                    
                    image_url = result_item["output"]["image_url"]
                
                    # Extract base64 image data
                    if image_url.startswith("data:image/"):
                        # Format: data:image/png;base64,<base64_data>
                        base64_data = image_url.split(",", 1)[1]
                    else:
                        # Assume it's just base64 data
                        base64_data = image_url
                    
                    try:
                        # Decode the image
                        image_bytes = base64.b64decode(base64_data)
                        
                        # Draw crosshair at the action coordinates
                        annotated_image = self._draw_crosshair_on_image(
                            image_bytes, 
                            int(action["x"]), 
                            int(action["y"])
                        )
                        
                        # Save as screenshot_action
                        self._save_artifact("screenshot_action", annotated_image)
                        
                    except Exception as e:
                        # If annotation fails, just log and continue
                        print(f"Failed to annotate screenshot: {e}")
                    
                    break  # Only process the first screenshot found

        # Increment turn counter
        self.current_turn += 1