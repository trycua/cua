"""HTTP client for CUA-Bench worker servers.

This client provides a convenient interface for interacting with worker servers,
managing observation history and action formatting for RL training.

Example:
    client = CBEnvWorkerClient(
        server_url="http://localhost:8001",
        img_w=1920, img_h=1080,
        max_step=50, max_hist=10, timeout=300
    )
    ret, meta_info = client.reset("./tasks/click-button", task_index=0)
    ret, meta_info = client.step(action_dict)
"""

import base64
import io
import re
import uuid
from dataclasses import dataclass, field
from typing import Any, Dict, Optional, Tuple, Union

import requests
from cua_bench.actions import action_to_dict, parse_action_string
from PIL import Image


@dataclass
class CBEnvWorkerClient:
    """HTTP client for CUA-Bench worker servers.

    This client manages:
    - Communication with the worker server
    - Image resizing and formatting
    - Observation history tracking
    - Action normalization (coordinate scaling)

    Attributes:
        server_url: URL of the worker server
        img_w: Target image width for observations
        img_h: Target image height for observations
        max_step: Maximum steps per episode
        max_hist: Maximum observation history length
        timeout: Environment timeout in seconds
        env_id: Current environment ID (set after reset)
        uid: Current episode unique ID
        step_count: Current step count
        done: Whether the current episode is done
        prompt: Current observation prompt with history
    """

    server_url: str
    img_w: int = 1920
    img_h: int = 1080
    max_step: int = 50
    max_hist: int = 10
    timeout: int = 300

    # State (set after reset)
    env_id: Optional[int] = field(default=None, init=False)
    uid: Optional[str] = field(default=None, init=False)
    step_count: int = field(default=0, init=False)
    done: bool = field(default=False, init=False)
    prompt: Optional[Dict[str, Any]] = field(default=None, init=False)

    # Original screen dimensions (detected from first screenshot)
    _orig_w: int = field(default=1920, init=False)
    _orig_h: int = field(default=1080, init=False)

    def reset(
        self,
        env_path: str,
        task_index: int = 0,
        split: str = "train",
    ) -> Tuple[Dict[str, Any], Dict[str, Any]]:
        """Reset the environment to a task.

        Args:
            env_path: Path to the environment directory
            task_index: Task variant index (default: 0)
            split: Dataset split (default: "train")

        Returns:
            Tuple of (env_ret, meta_info) where:
            - env_ret: {"obs": observation, "done": False, "is_init": True}
            - meta_info: {"uid": episode_uid}
        """
        # Reset state
        self.step_count = 0
        self.done = False
        self.uid = str(uuid.uuid4())

        # Make request
        response = requests.post(
            f"{self.server_url}/reset",
            json={
                "env_path": env_path,
                "task_index": task_index,
                "split": split,
                "timeout": self.timeout,
            },
            timeout=60,
        )
        if not response.ok:
            try:
                detail = response.json().get("detail", response.text)
            except Exception:
                detail = response.text
            raise RuntimeError(f"Server error ({response.status_code}): {detail}")
        data = response.json()

        self.env_id = data["env_id"]
        instruction = data["instruction"]

        # Process screenshot
        screenshot_b64 = data["screenshot"]
        jpg_string = self._process_screenshot(screenshot_b64)

        # Build observation prompt
        self.prompt = {
            "instruction": instruction,
            "steps": [f"<|vision_start|>{jpg_string}<|vision_end|>"],
        }

        obs = self._prompt_to_obs(self.prompt)

        return {
            "obs": obs,
            "done": False,
            "is_init": True,
        }, {"uid": self.uid}

    def step(self, action: Union[str, Dict[str, Any]]) -> Tuple[Dict[str, Any], Dict[str, Any]]:
        """Execute an action in the environment.

        Args:
            action: Action string or dictionary. Can be:
                - Action string: "click(0.5,0.5)" or "<|action_start|>click(0.5,0.5)<|action_end|>"
                - Raw action dict: {"type": "ClickAction", "x": 100, "y": 200}
                - Normalized action: {"type": "click", "x": 0.5, "y": 0.5}

        Returns:
            Tuple of (env_ret, meta_info) where:
            - env_ret: {
                "prev_obs": previous observation,
                "action": action string,
                "obs": new observation,
                "reward": reward (if done),
                "done": done flag,
                "is_init": False
              }
            - meta_info: {"uid": episode_uid}
        """
        if self.env_id is None:
            raise RuntimeError("Environment not initialized. Call reset() first.")

        if self.done:
            raise RuntimeError("Episode is done. Call reset() to start a new episode.")

        # Store previous observation
        prev_obs = self._prompt_to_obs(self.prompt) if self.prompt else None

        # Parse action string to dict if needed
        if isinstance(action, str):
            action = self._parse_action_string(action)

        # Normalize and denormalize action
        action_str, action_denormalized = self._process_action(action)

        # Make request
        response = requests.post(
            f"{self.server_url}/step",
            json={
                "action": action_denormalized,
                "env_id": self.env_id,
            },
            timeout=60,
        )
        if not response.ok:
            try:
                detail = response.json().get("detail", response.text)
            except Exception:
                detail = response.text
            raise RuntimeError(f"Server error ({response.status_code}): {detail}")
        data = response.json()

        # Update state
        self.step_count += 1
        self.done = data["done"]

        # Process screenshot
        screenshot_b64 = data["screenshot"]
        jpg_string = self._process_screenshot(screenshot_b64)

        # Update prompt history
        if self.prompt is not None:
            self.prompt["steps"].append(action_str)
            self.prompt["steps"].append(f"<|vision_start|>{jpg_string}<|vision_end|>")

            # Trim history if needed
            if len(self.prompt["steps"]) > self.max_hist * 2:
                # Keep the last max_hist action-observation pairs
                self.prompt["steps"] = self.prompt["steps"][-(self.max_hist * 2) :]

        obs = self._prompt_to_obs(self.prompt)

        return {
            "prev_obs": prev_obs,
            "action": action_str,
            "obs": obs,
            "reward": data["reward"],
            "done": data["done"],
            "is_init": False,
        }, {"uid": self.uid}

    def get_screenshot(self) -> bytes:
        """Get the current screenshot from the environment.

        Returns:
            Raw screenshot bytes (PNG format)
        """
        if self.env_id is None:
            raise RuntimeError("Environment not initialized. Call reset() first.")

        response = requests.get(
            f"{self.server_url}/screenshot",
            params={"env_id": self.env_id},
            timeout=30,
        )
        if not response.ok:
            try:
                detail = response.json().get("detail", response.text)
            except Exception:
                detail = response.text
            raise RuntimeError(f"Server error ({response.status_code}): {detail}")
        data = response.json()

        screenshot_b64 = data["screenshot"]
        return base64.b64decode(screenshot_b64)

    def shutdown(self) -> None:
        """Shut down the current environment."""
        if self.env_id is not None:
            try:
                requests.post(
                    f"{self.server_url}/shutdown",
                    json={"env_id": self.env_id},
                    timeout=10,
                )
            except Exception:
                pass
            self.env_id = None

    def health_check(self) -> Dict[str, Any]:
        """Check the health of the worker server.

        Returns:
            Health status dictionary
        """
        response = requests.get(f"{self.server_url}/health", timeout=10)
        if not response.ok:
            try:
                detail = response.json().get("detail", response.text)
            except Exception:
                detail = response.text
            raise RuntimeError(f"Server error ({response.status_code}): {detail}")
        return response.json()

    def _process_screenshot(self, screenshot_b64: str) -> str:
        """Process a screenshot: resize and convert to base64 JPEG.

        Args:
            screenshot_b64: Base64-encoded screenshot (any format)

        Returns:
            Base64-encoded JPEG string
        """
        # Decode screenshot
        screenshot_bytes = base64.b64decode(screenshot_b64)
        img = Image.open(io.BytesIO(screenshot_bytes))

        # Store original dimensions
        self._orig_w, self._orig_h = img.size

        # Resize if needed
        if img.size != (self.img_w, self.img_h):
            img = img.resize((self.img_w, self.img_h), Image.Resampling.LANCZOS)

        # Convert to RGB if needed
        if img.mode != "RGB":
            img = img.convert("RGB")

        # Encode as JPEG
        buffer = io.BytesIO()
        img.save(buffer, format="JPEG", quality=85)
        jpg_bytes = buffer.getvalue()

        return base64.b64encode(jpg_bytes).decode("utf-8")

    def _process_action(self, action: Dict[str, Any]) -> Tuple[str, Dict[str, Any]]:
        """Process an action: normalize for string representation and denormalize for server.

        Args:
            action: Action dictionary (may have normalized or absolute coordinates)

        Returns:
            Tuple of (action_str, action_denormalized)
            - action_str: String representation for observation history
            - action_denormalized: Action dict with absolute coordinates for server
        """
        action_type = action.get("type") or action.get("action_type", "")
        action_type_lower = action_type.lower().replace("_", "").replace("action", "")

        # Build action string and denormalized action
        action_str_parts = []
        action_denorm = {"type": action_type}

        # Handle coordinate normalization/denormalization
        def denorm_coord(val: float, dim: int) -> int:
            """Denormalize a coordinate from [0,1] to [0,dim]."""
            if isinstance(val, float) and 0 <= val <= 1:
                return int(val * dim)
            return int(val)

        def norm_coord(val: float, dim: int) -> float:
            """Normalize a coordinate from [0,dim] to [0,1]."""
            if isinstance(val, float) and val <= 1:
                return val
            return val / dim

        if action_type_lower in ("click", "clickaction"):
            x = action.get("x", 0)
            y = action.get("y", 0)
            x_denorm = denorm_coord(x, self._orig_w)
            y_denorm = denorm_coord(y, self._orig_h)
            x_norm = norm_coord(x, self._orig_w)
            y_norm = norm_coord(y, self._orig_h)
            action_str_parts = [f"click({x_norm:.4f},{y_norm:.4f})"]
            action_denorm = {"type": "ClickAction", "x": x_denorm, "y": y_denorm}

        elif action_type_lower in ("rightclick", "rightclickaction"):
            x = action.get("x", 0)
            y = action.get("y", 0)
            x_denorm = denorm_coord(x, self._orig_w)
            y_denorm = denorm_coord(y, self._orig_h)
            x_norm = norm_coord(x, self._orig_w)
            y_norm = norm_coord(y, self._orig_h)
            action_str_parts = [f"right_click({x_norm:.4f},{y_norm:.4f})"]
            action_denorm = {"type": "RightClickAction", "x": x_denorm, "y": y_denorm}

        elif action_type_lower in ("doubleclick", "doubleclickaction"):
            x = action.get("x", 0)
            y = action.get("y", 0)
            x_denorm = denorm_coord(x, self._orig_w)
            y_denorm = denorm_coord(y, self._orig_h)
            x_norm = norm_coord(x, self._orig_w)
            y_norm = norm_coord(y, self._orig_h)
            action_str_parts = [f"double_click({x_norm:.4f},{y_norm:.4f})"]
            action_denorm = {"type": "DoubleClickAction", "x": x_denorm, "y": y_denorm}

        elif action_type_lower in ("drag", "dragaction"):
            from_x = action.get("from_x", 0)
            from_y = action.get("from_y", 0)
            to_x = action.get("to_x", 0)
            to_y = action.get("to_y", 0)
            duration = action.get("duration", 1.0)
            from_x_denorm = denorm_coord(from_x, self._orig_w)
            from_y_denorm = denorm_coord(from_y, self._orig_h)
            to_x_denorm = denorm_coord(to_x, self._orig_w)
            to_y_denorm = denorm_coord(to_y, self._orig_h)
            action_str_parts = [
                f"drag({norm_coord(from_x, self._orig_w):.4f},{norm_coord(from_y, self._orig_h):.4f},"
                f"{norm_coord(to_x, self._orig_w):.4f},{norm_coord(to_y, self._orig_h):.4f})"
            ]
            action_denorm = {
                "type": "DragAction",
                "from_x": from_x_denorm,
                "from_y": from_y_denorm,
                "to_x": to_x_denorm,
                "to_y": to_y_denorm,
                "duration": duration,
            }

        elif action_type_lower in ("scroll", "scrollaction"):
            direction = action.get("direction", "up")
            amount = action.get("amount", 100)
            action_str_parts = [f"scroll({direction},{amount})"]
            action_denorm = {"type": "ScrollAction", "direction": direction, "amount": amount}

        elif action_type_lower in ("type", "typeaction"):
            text = action.get("text", "")
            action_str_parts = [f'type("{text}")']
            action_denorm = {"type": "TypeAction", "text": text}

        elif action_type_lower in ("key", "keyaction"):
            key = action.get("key", "")
            action_str_parts = [f"key({key})"]
            action_denorm = {"type": "KeyAction", "key": key}

        elif action_type_lower in ("hotkey", "hotkeyaction"):
            keys = action.get("keys", [])
            keys_str = "+".join(keys)
            action_str_parts = [f"hotkey({keys_str})"]
            action_denorm = {"type": "HotkeyAction", "keys": keys}

        elif action_type_lower in ("wait", "waitaction"):
            seconds = action.get("seconds", 1.0)
            action_str_parts = [f"wait({seconds})"]
            action_denorm = {"type": "WaitAction", "seconds": seconds}

        elif action_type_lower in ("done", "doneaction"):
            action_str_parts = ["done()"]
            action_denorm = {"type": "DoneAction"}

        else:
            # Unknown action, pass through
            action_str_parts = [str(action)]
            action_denorm = action

        action_str = f"<|action_start|>{''.join(action_str_parts)}<|action_end|>"
        return action_str, action_denorm

    def _prompt_to_obs(self, prompt: Optional[Dict[str, Any]]) -> str:
        """Convert a prompt to an observation string.

        Args:
            prompt: Prompt dictionary with instruction and steps

        Returns:
            Observation string for model input
        """
        if prompt is None:
            return ""

        parts = [f"<|instruction|>{prompt['instruction']}<|instruction_end|>"]
        for step in prompt["steps"]:
            parts.append(step)

        return "".join(parts)

    def _parse_action_string(self, action_str: str) -> Dict[str, Any]:
        """Parse action string from model response into a dict.

        Handles formats:
        - Raw action string: "click(0.5,0.5)"
        - With tags: "<|action_start|>click(0.5,0.5)<|action_end|>"

        Args:
            action_str: Action string to parse

        Returns:
            Action dict suitable for _process_action()
        """
        # Extract action from tags if present
        action_match = re.search(r"<\|action_start\|>(.*?)<\|action_end\|>", action_str, re.DOTALL)
        if action_match:
            action_str = action_match.group(1).strip()

        # Try to parse the action string
        try:
            action = parse_action_string(action_str)
            return action_to_dict(action)
        except ValueError:
            # Default to done if parsing fails
            return {"type": "DoneAction"}


# Convenience function to create a client from environment variables
def create_client_from_env(
    server_url: Optional[str] = None,
    **kwargs,
) -> CBEnvWorkerClient:
    """Create a worker client from environment variables.

    Environment variables:
        OSGYM_SERVER_URL: Worker server URL (default: http://localhost:8001)
        OSGYM_IMG_W: Image width (default: 1920)
        OSGYM_IMG_H: Image height (default: 1080)
        OSGYM_MAX_STEP: Max steps per episode (default: 50)
        OSGYM_MAX_HIST: Max observation history (default: 10)
        OSGYM_TIMEOUT: Environment timeout (default: 300)

    Args:
        server_url: Override server URL
        **kwargs: Override other client parameters

    Returns:
        Configured CBEnvWorkerClient instance
    """
    import os

    config = {
        "server_url": server_url or os.environ.get("OSGYM_SERVER_URL", "http://localhost:8001"),
        "img_w": int(os.environ.get("OSGYM_IMG_W", 1920)),
        "img_h": int(os.environ.get("OSGYM_IMG_H", 1080)),
        "max_step": int(os.environ.get("OSGYM_MAX_STEP", 50)),
        "max_hist": int(os.environ.get("OSGYM_MAX_HIST", 10)),
        "timeout": int(os.environ.get("OSGYM_TIMEOUT", 300)),
    }
    config.update(kwargs)

    return CBEnvWorkerClient(**config)
