"""
Process human demonstrations from noVNC recordings into semantic action traces.

Parses noVNC recording files (.js or .bin format) and generates structured
trajectory data with VLM-generated captions suitable for prompting agents.

Usage:
    python -m agent.process_demonstration recording.js --output trajectory.json
    python -m agent.process_demonstration recording.js --task "Login to email"
"""

import argparse
import base64
import io
import json
import os
import re
import struct
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Optional

# PIL is optional - only needed for frame extraction
try:
    from PIL import Image

    HAS_PIL = True
except ImportError:
    HAS_PIL = False


@dataclass
class InputEvent:
    """Represents a user input event from the recording."""

    timestamp: float  # milliseconds from start
    event_type: str  # 'key', 'click', 'drag', 'scroll', 'type'
    data: dict = field(default_factory=dict)
    screenshot_before: Optional[bytes] = None
    screenshot_crop: Optional[bytes] = None


@dataclass
class TrajectoryStep:
    """A single step in the demonstration trajectory."""

    step_idx: int
    screenshot: Optional[str] = None  # base64 encoded
    cropped_screenshot: Optional[str] = None  # base64 encoded
    caption: dict = field(default_factory=dict)
    raw_event: Optional[dict] = None


class NoVNCRecordingParser:
    """Parser for noVNC recording files (.js and .bin formats)."""

    # RFB message types (client to server)
    MSG_SET_PIXEL_FORMAT = 0
    MSG_SET_ENCODINGS = 2
    MSG_FRAMEBUFFER_UPDATE_REQUEST = 3
    MSG_KEY_EVENT = 4
    MSG_POINTER_EVENT = 5
    MSG_CLIENT_CUT_TEXT = 6

    # Special key mapping (keysym to readable name)
    SPECIAL_KEYS = {
        0xFF08: "BackSpace",
        0xFF09: "Tab",
        0xFF0D: "Return",
        0xFF0A: "Enter",
        0xFF1B: "Escape",
        0xFF50: "Home",
        0xFF51: "Left",
        0xFF52: "Up",
        0xFF53: "Right",
        0xFF54: "Down",
        0xFF55: "PageUp",
        0xFF56: "PageDown",
        0xFF57: "End",
        0xFF63: "Insert",
        0xFFFF: "Delete",
        0xFFE1: "Shift_L",
        0xFFE2: "Shift_R",
        0xFFE3: "Control_L",
        0xFFE4: "Control_R",
        0xFFE9: "Alt_L",
        0xFFEA: "Alt_R",
        0xFFEB: "Super_L",
        0xFFEC: "Super_R",
        0xFFBE: "F1",
        0xFFBF: "F2",
        0xFFC0: "F3",
        0xFFC1: "F4",
        0xFFC2: "F5",
        0xFFC3: "F6",
        0xFFC4: "F7",
        0xFFC5: "F8",
        0xFFC6: "F9",
        0xFFC7: "F10",
        0xFFC8: "F11",
        0xFFC9: "F12",
        0x0020: "Space",
    }

    def __init__(self):
        self.frames: list[dict] = []
        self.server_frames: list[dict] = []
        self.client_frames: list[dict] = []
        self._last_button_mask = 0

    def parse_js_file(self, path: str) -> list[dict]:
        """Parse a .js recording file with VNC_frame_data array."""
        with open(path, "r", encoding="utf-8") as f:
            content = f.read()

        # Extract VNC_frame_data array content
        match = re.search(r"var\s+VNC_frame_data\s*=\s*\[(.*?)\];", content, re.DOTALL)
        if not match:
            # Try alternative format
            match = re.search(r"VNC_frame_data\s*=\s*\[(.*?)\];", content, re.DOTALL)

        if not match:
            raise ValueError("Could not find VNC_frame_data array in JS file")

        array_content = match.group(1)

        # Parse each frame string
        self.frames = []

        # Find all string entries (they're JSON-escaped strings)
        frame_pattern = re.compile(r'"([^"\\]*(?:\\.[^"\\]*)*)"')

        for frame_match in frame_pattern.finditer(array_content):
            frame_str = frame_match.group(1)

            # Handle escape sequences
            frame_str = frame_str.replace("\\n", "\n").replace("\\r", "\r")
            frame_str = frame_str.replace('\\"', '"').replace("\\\\", "\\")

            if frame_str == "EOF":
                break

            if not frame_str or len(frame_str) < 2:
                continue

            frame = self._parse_frame_string(frame_str)
            if frame:
                self.frames.append(frame)

        self._separate_frames()
        return self.frames

    def parse_bin_file(self, path: str) -> list[dict]:
        """Parse a .bin binary recording file.

        Binary format: fromClient(1) + timestamp(4) + dataLen(4) + data(dataLen)
        """
        with open(path, "rb") as f:
            data = f.read()

        self.frames = []
        offset = 0

        while offset + 9 <= len(data):
            from_client = data[offset] == 1
            timestamp = struct.unpack(">I", data[offset + 1 : offset + 5])[0]
            data_len = struct.unpack(">I", data[offset + 5 : offset + 9])[0]

            if offset + 9 + data_len > len(data):
                break

            frame_data = data[offset + 9 : offset + 9 + data_len]

            self.frames.append(
                {"from_client": from_client, "timestamp": timestamp, "data": frame_data}
            )

            offset += 9 + data_len

        self._separate_frames()
        return self.frames

    def _parse_frame_string(self, frame_str: str) -> Optional[dict]:
        """Parse a single frame string from JS format.

        Format: {timestamp{base64_data  (server frame)
        Format: }timestamp{base64_data  (client frame)
        """
        if not frame_str:
            return None

        from_client = frame_str[0] == "}"

        # Find the second { which separates timestamp from data
        data_start = frame_str.find("{", 1)
        if data_start == -1:
            return None

        try:
            timestamp = int(frame_str[1:data_start])
        except ValueError:
            return None

        base64_data = frame_str[data_start + 1 :]

        try:
            frame_data = base64.b64decode(base64_data)
        except Exception:
            # Try binary decoding (older format)
            frame_data = bytes([ord(c) for c in base64_data])

        return {"from_client": from_client, "timestamp": timestamp, "data": frame_data}

    def _separate_frames(self):
        """Separate frames into server and client frames."""
        self.server_frames = [f for f in self.frames if not f.get("from_client")]
        self.client_frames = [f for f in self.frames if f.get("from_client")]

    def decode_client_message(self, data: bytes) -> Optional[dict]:
        """Decode a client-to-server RFB message."""
        if not data or len(data) < 1:
            return None

        msg_type = data[0]

        if msg_type == self.MSG_KEY_EVENT and len(data) >= 8:
            down = data[1] == 1
            keysym = struct.unpack(">I", data[4:8])[0]
            key_name = self._keysym_to_name(keysym)
            return {"type": "KeyEvent", "down": down, "keysym": keysym, "key_name": key_name}

        elif msg_type == self.MSG_POINTER_EVENT and len(data) >= 6:
            button_mask = data[1]
            x = struct.unpack(">H", data[2:4])[0]
            y = struct.unpack(">H", data[4:6])[0]

            # Detect button changes
            prev_mask = self._last_button_mask
            pressed = button_mask & ~prev_mask
            released = prev_mask & ~button_mask
            self._last_button_mask = button_mask

            events = []
            button_names = ["left", "middle", "right", "scroll_up", "scroll_down"]
            for i, name in enumerate(button_names):
                bit = 1 << i
                if pressed & bit:
                    events.append({"button": name, "action": "down"})
                if released & bit:
                    events.append({"button": name, "action": "up"})

            return {
                "type": "PointerEvent",
                "x": x,
                "y": y,
                "button_mask": button_mask,
                "events": events,
                "is_move": len(events) == 0,
            }

        elif msg_type == self.MSG_CLIENT_CUT_TEXT and len(data) >= 8:
            length = struct.unpack(">I", data[4:8])[0]
            text = data[8 : 8 + length].decode("latin-1", errors="replace")
            return {"type": "ClientCutText", "text": text}

        elif msg_type == self.MSG_FRAMEBUFFER_UPDATE_REQUEST:
            return {"type": "FramebufferUpdateRequest"}
        elif msg_type == self.MSG_SET_PIXEL_FORMAT:
            return {"type": "SetPixelFormat"}
        elif msg_type == self.MSG_SET_ENCODINGS:
            return {"type": "SetEncodings"}

        return {"type": "Unknown", "msg_type": msg_type}

    def _keysym_to_name(self, keysym: int) -> str:
        """Convert keysym to readable key name."""
        if keysym in self.SPECIAL_KEYS:
            return self.SPECIAL_KEYS[keysym]
        if 0x20 <= keysym <= 0x7E:
            return chr(keysym)
        return f"0x{keysym:04x}"

    def extract_input_events(self) -> list[InputEvent]:
        """Extract meaningful input events from client frames."""
        events = []
        key_buffer = []
        last_key_time = 0
        drag_start = None
        drag_points = []

        for frame in self.client_frames:
            msg = self.decode_client_message(frame["data"])
            if not msg:
                continue

            timestamp = frame["timestamp"]

            if msg["type"] == "KeyEvent":
                if msg["down"]:
                    # Check if we should flush the key buffer (time gap > 500ms)
                    if key_buffer and timestamp - last_key_time > 500:
                        if len(key_buffer) > 1:
                            events.append(
                                InputEvent(
                                    timestamp=last_key_time,
                                    event_type="type",
                                    data={"text": "".join(key_buffer)},
                                )
                            )
                        else:
                            events.append(
                                InputEvent(
                                    timestamp=last_key_time,
                                    event_type="key",
                                    data={"key": key_buffer[0], "action": "press"},
                                )
                            )
                        key_buffer = []

                    key_name = msg["key_name"]

                    # Handle special keys immediately
                    if key_name in [
                        "Return",
                        "Enter",
                        "Tab",
                        "Escape",
                        "BackSpace",
                        "Delete",
                    ] or key_name.startswith("F"):
                        if key_buffer:
                            events.append(
                                InputEvent(
                                    timestamp=last_key_time,
                                    event_type="type",
                                    data={"text": "".join(key_buffer)},
                                )
                            )
                            key_buffer = []
                        events.append(
                            InputEvent(
                                timestamp=timestamp,
                                event_type="key",
                                data={"key": key_name, "action": "press"},
                            )
                        )
                    elif len(key_name) == 1:
                        key_buffer.append(key_name)
                        last_key_time = timestamp

            elif msg["type"] == "PointerEvent":
                x, y = msg["x"], msg["y"]

                for btn_event in msg.get("events", []):
                    button = btn_event["button"]
                    action = btn_event["action"]

                    if button in ["scroll_up", "scroll_down"]:
                        events.append(
                            InputEvent(
                                timestamp=timestamp,
                                event_type="scroll",
                                data={
                                    "direction": "up" if button == "scroll_up" else "down",
                                    "x": x,
                                    "y": y,
                                },
                            )
                        )
                    elif button == "left":
                        if action == "down":
                            drag_start = {"x": x, "y": y, "timestamp": timestamp}
                            drag_points = [(x, y)]
                        elif action == "up":
                            if drag_start:
                                dx = abs(x - drag_start["x"])
                                dy = abs(y - drag_start["y"])
                                duration = timestamp - drag_start["timestamp"]

                                if dx > 10 or dy > 10:
                                    # This is a drag
                                    events.append(
                                        InputEvent(
                                            timestamp=drag_start["timestamp"],
                                            event_type="drag",
                                            data={
                                                "start": {
                                                    "x": drag_start["x"],
                                                    "y": drag_start["y"],
                                                },
                                                "end": {"x": x, "y": y},
                                                "path": drag_points + [(x, y)],
                                            },
                                        )
                                    )
                                else:
                                    # This is a click
                                    events.append(
                                        InputEvent(
                                            timestamp=timestamp,
                                            event_type="click",
                                            data={"x": x, "y": y, "button": "left"},
                                        )
                                    )
                                drag_start = None
                                drag_points = []
                    elif button == "right" and action == "up":
                        events.append(
                            InputEvent(
                                timestamp=timestamp,
                                event_type="click",
                                data={"x": x, "y": y, "button": "right"},
                            )
                        )

                # Track drag movement
                if drag_start and msg.get("is_move"):
                    drag_points.append((x, y))

            elif msg["type"] == "ClientCutText":
                events.append(
                    InputEvent(timestamp=timestamp, event_type="paste", data={"text": msg["text"]})
                )

        # Flush remaining key buffer
        if key_buffer:
            if len(key_buffer) > 1:
                events.append(
                    InputEvent(
                        timestamp=last_key_time,
                        event_type="type",
                        data={"text": "".join(key_buffer)},
                    )
                )
            else:
                events.append(
                    InputEvent(
                        timestamp=last_key_time,
                        event_type="key",
                        data={"key": key_buffer[0], "action": "press"},
                    )
                )

        return events


class FrameExtractor:
    """Extract screen frames from VNC server data at specific timestamps."""

    def __init__(self):
        self._framebuffer: Optional[Image.Image] = None
        self._width = 0
        self._height = 0

    def get_frame_at_timestamp(
        self, server_frames: list[dict], target_timestamp: float, look_back_ms: float = 100
    ) -> Optional[bytes]:
        """Get the screen state just before the given timestamp.

        This is a simplified implementation. A full implementation would
        need to decode all VNC encoding types and maintain the framebuffer.
        For now, this returns None - screenshots should be captured separately.
        """
        # Full VNC frame decoding is complex (needs to handle Raw, CopyRect,
        # Tight, ZRLE, etc). For production use, consider:
        # 1. Capturing screenshots alongside the recording
        # 2. Using a headless VNC client to replay and capture
        # 3. Using the noVNC playback UI with screenshot capture
        return None


class DemonstrationCaptioner:
    """Generate semantic captions for demonstration steps using a VLM."""

    DEFAULT_SYSTEM_PROMPT = """You are analyzing a human demonstration of a computer task.
For each action shown, provide a semantic description that would help an AI agent replicate this workflow.

Output a JSON object with exactly these fields:
{
  "observation": "What the screen shows (describe both the cropped action area and full context)",
  "think": "What the user likely intends to accomplish with this action",
  "action": "Concrete description of the action taken (click, type, drag, etc.)",
  "expectation": "What should happen immediately after this action"
}

Rules:
- Never include specific coordinates, file paths, or timestamps
- Describe UI elements by their visual appearance and labels
- Focus on the semantic meaning, not low-level details
- Use clear, actionable language an agent could follow"""

    def __init__(
        self,
        provider: str = "anthropic",
        model: str = "claude-sonnet-4-20250514",
        api_key: Optional[str] = None,
    ):
        self.provider = provider.lower()
        self.model = model
        self.api_key = api_key or os.environ.get(
            "ANTHROPIC_API_KEY" if self.provider == "anthropic" else "OPENAI_API_KEY"
        )

    def generate_caption(
        self,
        event: InputEvent,
        screenshot_b64: Optional[str] = None,
        crop_b64: Optional[str] = None,
        task_description: str = "",
        recent_steps: list[dict] = None,
    ) -> dict:
        """Generate a semantic caption for the given event."""
        if not self.api_key:
            # Return a basic caption without VLM
            return self._generate_basic_caption(event)

        prompt = self._build_prompt(event, task_description, recent_steps or [])

        try:
            if self.provider == "anthropic":
                return self._call_anthropic(prompt, screenshot_b64, crop_b64)
            else:
                return self._call_openai(prompt, screenshot_b64, crop_b64)
        except Exception as e:
            print(f"Warning: VLM call failed: {e}", file=sys.stderr)
            return self._generate_basic_caption(event)

    def _generate_basic_caption(self, event: InputEvent) -> dict:
        """Generate a basic caption without VLM."""
        action_desc = ""
        observation = "Screenshot not available"
        think = ""
        expectation = ""

        if event.event_type == "click":
            x, y = event.data.get("x", 0), event.data.get("y", 0)
            button = event.data.get("button", "left")
            action_desc = f"{'Right-click' if button == 'right' else 'Click'} at screen position"
            think = "User intends to interact with an element at this location"
            expectation = "The clicked element responds (selection, menu, or action)"
        elif event.event_type == "type":
            text = event.data.get("text", "")
            action_desc = f"Type: {text}"
            think = "User is entering text input"
            expectation = "Text appears in the focused input field"
        elif event.event_type == "key":
            key = event.data.get("key", "")
            action_desc = f"Press {key} key"
            think = f"User pressed {key} to trigger an action"
            expectation = f"System responds to {key} keypress"
        elif event.event_type == "drag":
            action_desc = "Click-and-drag operation"
            think = "User is moving or selecting something via drag"
            expectation = "Element moves or selection is made"
        elif event.event_type == "scroll":
            direction = event.data.get("direction", "down")
            action_desc = f"Scroll {direction}"
            think = f"User scrolls to see more content {direction}"
            expectation = "View scrolls in the specified direction"
        elif event.event_type == "paste":
            action_desc = "Paste clipboard content"
            think = "User pastes previously copied content"
            expectation = "Clipboard content appears at cursor position"

        return {
            "observation": observation,
            "think": think,
            "action": action_desc,
            "expectation": expectation,
        }

    def _build_prompt(
        self, event: InputEvent, task_description: str, recent_steps: list[dict]
    ) -> str:
        """Build the prompt for VLM caption generation."""
        event_desc = self._describe_event(event)

        recent_context = ""
        if recent_steps:
            recent_context = "\n\nRecent steps (for context):\n"
            for step in recent_steps[-3:]:
                cap = step.get("caption", {})
                recent_context += f"- {cap.get('action', 'Unknown action')}\n"

        return f"""{self.DEFAULT_SYSTEM_PROMPT}

Current action: {event_desc}
Task context: {task_description or 'General computer use'}
{recent_context}

Analyze the screenshot(s) and provide your JSON response."""

    def _describe_event(self, event: InputEvent) -> str:
        """Create a text description of the event."""
        if event.event_type == "click":
            button = event.data.get("button", "left")
            return f"{'Right-click' if button == 'right' else 'Click'}"
        elif event.event_type == "type":
            return f"Type: '{event.data.get('text', '')}'"
        elif event.event_type == "key":
            return f"Press {event.data.get('key', '')} key"
        elif event.event_type == "drag":
            return "Drag operation"
        elif event.event_type == "scroll":
            return f"Scroll {event.data.get('direction', 'down')}"
        elif event.event_type == "paste":
            return "Paste from clipboard"
        return event.event_type

    def _call_anthropic(
        self, prompt: str, screenshot_b64: Optional[str], crop_b64: Optional[str]
    ) -> dict:
        """Call Anthropic API for caption generation."""
        import httpx

        content = [{"type": "text", "text": prompt}]

        if crop_b64:
            content.append(
                {
                    "type": "image",
                    "source": {"type": "base64", "media_type": "image/png", "data": crop_b64},
                }
            )
        if screenshot_b64:
            content.append(
                {
                    "type": "image",
                    "source": {"type": "base64", "media_type": "image/png", "data": screenshot_b64},
                }
            )

        response = httpx.post(
            "https://api.anthropic.com/v1/messages",
            headers={
                "x-api-key": self.api_key,
                "anthropic-version": "2023-06-01",
                "content-type": "application/json",
            },
            json={
                "model": self.model,
                "max_tokens": 1024,
                "messages": [{"role": "user", "content": content}],
            },
            timeout=120,
        )
        response.raise_for_status()

        text = response.json()["content"][0]["text"]
        return self._extract_json(text)

    def _call_openai(
        self, prompt: str, screenshot_b64: Optional[str], crop_b64: Optional[str]
    ) -> dict:
        """Call OpenAI API for caption generation."""
        import httpx

        content = [{"type": "text", "text": prompt}]

        if crop_b64:
            content.append(
                {"type": "image_url", "image_url": {"url": f"data:image/png;base64,{crop_b64}"}}
            )
        if screenshot_b64:
            content.append(
                {
                    "type": "image_url",
                    "image_url": {"url": f"data:image/png;base64,{screenshot_b64}"},
                }
            )

        response = httpx.post(
            "https://api.openai.com/v1/chat/completions",
            headers={"Authorization": f"Bearer {self.api_key}", "Content-Type": "application/json"},
            json={
                "model": self.model,
                "messages": [{"role": "user", "content": content}],
                "temperature": 0.2,
            },
            timeout=120,
        )
        response.raise_for_status()

        text = response.json()["choices"][0]["message"]["content"]
        return self._extract_json(text)

    def _extract_json(self, text: str) -> dict:
        """Extract JSON object from LLM response."""
        # Try to find JSON in code blocks first
        json_match = re.search(r"```(?:json)?\s*(\{.*?\})\s*```", text, re.DOTALL)
        if json_match:
            try:
                return json.loads(json_match.group(1))
            except json.JSONDecodeError:
                pass

        # Try to find raw JSON
        json_match = re.search(r'\{[^{}]*"observation"[^{}]*\}', text, re.DOTALL)
        if json_match:
            try:
                return json.loads(json_match.group(0))
            except json.JSONDecodeError:
                pass

        # Return empty dict if parsing fails
        return {"observation": "", "think": "", "action": "", "expectation": ""}


class DemonstrationProcessor:
    """Main processor that orchestrates the full pipeline."""

    def __init__(
        self,
        vlm_provider: str = "anthropic",
        vlm_model: str = "claude-sonnet-4-20250514",
        api_key: Optional[str] = None,
    ):
        self.parser = NoVNCRecordingParser()
        self.captioner = DemonstrationCaptioner(
            provider=vlm_provider, model=vlm_model, api_key=api_key
        )

    def process(
        self, recording_path: str, task_description: str = "", screenshots_dir: Optional[str] = None
    ) -> dict:
        """Process a recording file into a trajectory with captions.

        Args:
            recording_path: Path to .js or .bin recording file
            task_description: Optional description of the task being demonstrated
            screenshots_dir: Optional directory containing timestamped screenshots

        Returns:
            Dictionary with trajectory and skill_prompt
        """
        path = Path(recording_path)

        # Parse recording
        if path.suffix == ".bin":
            self.parser.parse_bin_file(str(path))
        else:
            self.parser.parse_js_file(str(path))

        # Extract input events
        events = self.parser.extract_input_events()

        if not events:
            print("Warning: No input events found in recording", file=sys.stderr)
            return {"trajectory": [], "skill_prompt": ""}

        print(f"Extracted {len(events)} input events from recording")

        # Load screenshots if directory provided
        screenshots = {}
        if screenshots_dir:
            screenshots = self._load_screenshots(screenshots_dir)

        # Generate trajectory
        trajectory = []
        for idx, event in enumerate(events, start=1):
            print(f"Processing step {idx}/{len(events)}: {event.event_type}")

            # Find closest screenshot
            screenshot_b64 = None
            crop_b64 = None
            if screenshots:
                screenshot_b64, crop_b64 = self._find_closest_screenshot(
                    event.timestamp, screenshots
                )

            # Generate caption
            caption = self.captioner.generate_caption(
                event=event,
                screenshot_b64=screenshot_b64,
                crop_b64=crop_b64,
                task_description=task_description,
                recent_steps=trajectory[-3:] if trajectory else [],
            )

            step = TrajectoryStep(
                step_idx=idx,
                screenshot=screenshot_b64,
                cropped_screenshot=crop_b64,
                caption=caption,
                raw_event={
                    "timestamp": event.timestamp,
                    "type": event.event_type,
                    "data": event.data,
                },
            )
            trajectory.append(self._step_to_dict(step))

        # Generate skill prompt
        skill_prompt = self._generate_skill_prompt(trajectory, task_description)

        return {
            "trajectory": trajectory,
            "skill_prompt": skill_prompt,
            "metadata": {
                "recording_file": str(path.name),
                "task_description": task_description,
                "total_steps": len(trajectory),
                "total_frames": len(self.parser.frames),
                "client_frames": len(self.parser.client_frames),
                "server_frames": len(self.parser.server_frames),
            },
        }

    def _load_screenshots(self, screenshots_dir: str) -> dict[float, tuple[str, str]]:
        """Load screenshots from directory, indexed by timestamp."""
        screenshots = {}
        dir_path = Path(screenshots_dir)

        if not dir_path.exists():
            return screenshots

        for file in dir_path.glob("*.png"):
            # Try to extract timestamp from filename
            match = re.search(r"(\d+(?:\.\d+)?)", file.stem)
            if match:
                timestamp = float(match.group(1))

                with open(file, "rb") as f:
                    img_data = base64.b64encode(f.read()).decode("utf-8")

                # Check for corresponding crop
                crop_path = file.with_suffix(".crop.png")
                crop_data = None
                if crop_path.exists():
                    with open(crop_path, "rb") as f:
                        crop_data = base64.b64encode(f.read()).decode("utf-8")

                screenshots[timestamp] = (img_data, crop_data)

        return screenshots

    def _find_closest_screenshot(
        self, timestamp: float, screenshots: dict[float, tuple[str, str]]
    ) -> tuple[Optional[str], Optional[str]]:
        """Find the screenshot closest to (but before) the given timestamp."""
        if not screenshots:
            return None, None

        # Find all timestamps <= target
        valid_times = [t for t in screenshots.keys() if t <= timestamp]

        if not valid_times:
            # Use earliest available
            earliest = min(screenshots.keys())
            return screenshots[earliest]

        closest = max(valid_times)
        return screenshots[closest]

    def _step_to_dict(self, step: TrajectoryStep) -> dict:
        """Convert TrajectoryStep to dictionary."""
        return {
            "step_idx": step.step_idx,
            "screenshot": step.screenshot,
            "cropped_screenshot": step.cropped_screenshot,
            "caption": step.caption,
            "raw_event": step.raw_event,
        }

    def _generate_skill_prompt(self, trajectory: list[dict], task_description: str) -> str:
        """Generate a skill prompt that can be used to guide an agent."""
        if not trajectory:
            return ""

        steps_text = []
        for step in trajectory:
            caption = step.get("caption", {})
            action = caption.get("action", "Unknown action")
            expectation = caption.get("expectation", "")
            steps_text.append(f"Step {step['step_idx']}: {action}")
            if expectation:
                steps_text.append(f"  Expected result: {expectation}")

        steps_description = "\n".join(steps_text)

        prompt = f"""You have been shown a demonstration of how to perform this task:
{task_description or "Complete the workflow shown in the demonstration"}

The demonstration consisted of the following steps:
{steps_description}

Follow this workflow pattern, adapting as needed for the current screen state.
Key observations from the demonstration:
- Total steps: {len(trajectory)}
- Main actions: {', '.join(set(s.get('caption', {}).get('action', '').split()[0] for s in trajectory if s.get('caption', {}).get('action')))}

Execute each step while monitoring for expected outcomes before proceeding."""

        return prompt


def main():
    """CLI entry point."""
    parser = argparse.ArgumentParser(
        description="Process noVNC demonstration recordings into semantic trajectories",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Examples:
    python -m agent.process_demonstration recording.js
    python -m agent.process_demonstration recording.js --output traj.json
    python -m agent.process_demonstration recording.js --task "Login to email" --screenshots ./screenshots/
    python -m agent.process_demonstration recording.bin --provider openai --model gpt-4o
        """,
    )

    parser.add_argument("recording", help="Path to noVNC recording file (.js or .bin)")
    parser.add_argument(
        "--output", "-o", help="Output JSON file path (default: <recording>_trajectory.json)"
    )
    parser.add_argument(
        "--task", "-t", default="", help="Description of the task being demonstrated"
    )
    parser.add_argument("--screenshots", "-s", help="Directory containing timestamped screenshots")
    parser.add_argument(
        "--provider",
        "-p",
        choices=["anthropic", "openai"],
        default="anthropic",
        help="VLM provider for caption generation (default: anthropic)",
    )
    parser.add_argument(
        "--model", "-m", default="claude-haiku-4-5", help="Model to use for caption generation"
    )
    parser.add_argument(
        "--no-captions",
        action="store_true",
        help="Skip VLM caption generation (output raw events only)",
    )

    args = parser.parse_args()

    recording_path = Path(args.recording)
    if not recording_path.exists():
        print(f"Error: Recording file not found: {recording_path}", file=sys.stderr)
        sys.exit(1)

    output_path = args.output
    if not output_path:
        output_path = recording_path.with_suffix(".trajectory.json")

    print(f"Processing: {recording_path}")
    print(f"Task: {args.task or '(not specified)'}")

    processor = DemonstrationProcessor(
        vlm_provider=args.provider if not args.no_captions else "none",
        vlm_model=args.model,
        api_key=None if args.no_captions else None,  # Will use env var
    )

    result = processor.process(
        recording_path=str(recording_path),
        task_description=args.task,
        screenshots_dir=args.screenshots,
    )

    with open(output_path, "w", encoding="utf-8") as f:
        json.dump(result, f, indent=2, ensure_ascii=False)

    print(f"\nOutput saved to: {output_path}")
    print(f"Trajectory steps: {len(result['trajectory'])}")

    if result.get("skill_prompt"):
        print("\n--- Skill Prompt Preview ---")
        print(
            result["skill_prompt"][:500] + "..."
            if len(result["skill_prompt"]) > 500
            else result["skill_prompt"]
        )


if __name__ == "__main__":
    main()
