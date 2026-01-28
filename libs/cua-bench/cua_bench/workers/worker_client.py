import base64
import io
import random
import re
import uuid
from typing import Any, Dict, List

import numpy as np
import requests
from cua_bench.actions import action_to_dict
from cua_bench.types import (
    ClickAction,
    DoneAction,
    DoubleClickAction,
    DragAction,
    HotkeyAction,
    RightClickAction,
    ScrollAction,
    TypeAction,
    WaitAction,
)
from PIL import Image

DEBUG = True


def print_debug(msg):
    if DEBUG:
        print(msg)


def pillow_to_jpg_string(img):
    # Save to bytes buffer as JPG
    buffer = io.BytesIO()
    img.save(buffer, format="JPEG", quality=80)
    jpg_bytes = buffer.getvalue()
    # Encode bytes to base64 string
    jpg_string = base64.b64encode(jpg_bytes).decode("utf-8")
    return jpg_string


def jpg_string_to_pillow(jpg_string):
    # Decode base64 string to bytes
    jpg_bytes = base64.b64decode(jpg_string)
    # Convert bytes to PIL Image
    buffer = io.BytesIO(jpg_bytes)
    img = Image.open(buffer)
    return img


def rgb_to_jpg_string(rgb_array):
    """
    Convert RGB array to JPG-encoded base64 string.
    Input: numpy array of shape (height, width, 3) with values 0-255
    Output: base64-encoded string
    """
    img = Image.fromarray(rgb_array.astype("uint8"), "RGB")
    buffer = io.BytesIO()
    img.save(buffer, format="JPEG", quality=80)
    jpg_bytes = buffer.getvalue()
    buffer.close()
    jpg_string = base64.b64encode(jpg_bytes).decode("utf-8")
    return jpg_string


def jpg_string_to_rgb(jpg_string):
    """
    Convert JPG-encoded base64 string back to RGB array.
    Input: base64-encoded string
    Output: numpy array of shape (height, width, 3) with values 0-255
    """
    jpg_bytes = base64.b64decode(jpg_string)
    buffer = io.BytesIO(jpg_bytes)
    img = Image.open(buffer)
    rgb_array = np.array(img)
    buffer.close()
    return rgb_array


def parse_action(action_string):
    pattern = r"(\w+)\((.*?)\)"
    match = re.match(pattern, action_string)
    if not match:
        return None, None
    function_name = match.group(1)
    args_string = match.group(2)
    if not args_string:
        return function_name, []
    args = []
    for arg in re.split(r",\s*", args_string):
        try:
            args.append(int(arg))
        except ValueError:
            args.append(arg)
    return function_name, args


def check_type(inputs, types):
    if len(inputs) != len(types):
        return False
    for i, t in zip(inputs, types):
        if not isinstance(i, t):
            return False
    return True


class CBEnvWorkerClient:
    """HTTP client for CUA-Bench worker servers.

    This client manages communication with the worker server, image processing,
    observation history tracking, and action normalization.

    Args:
        env_config: Configuration dict with keys:
            - server_url: URL of the worker server
            - task_configs: List of task configs, each with env_path, task_index, split
            - img_w: Image width (default: 1920)
            - img_h: Image height (default: 1080)
            - max_step: Maximum steps per episode (default: 50)
            - max_hist: Maximum observation history length (default: 10)
            - timeout: Environment timeout in seconds (default: 300)
    """

    vision_start_token = "<|vision_start|>"
    vision_end_token = "<|vision_end|>"
    think_start_token = "<|think_start|>"
    think_end_token = "<|think_end|>"
    action_start_token = "<|action_start|>"
    action_end_token = "<|action_end|>"
    valid_fn_names = [
        "click",
        "left_double",
        "right_single",
        "drag",
        "hotkey",
        "type",
        "scroll",
        "wait",
        "call_user",
        "done",
    ]
    vlm_img_w = 1260
    vlm_img_h = 700
    dynamic_img_size = True

    def __init__(self, env_config):
        print_debug("[CBEnvWorkerClient] Initializing worker...")
        self.env_config = env_config
        self.server_url = env_config["server_url"]
        self.max_step = env_config["max_step"]
        self.max_hist = env_config["max_hist"]
        self.task_configs: List[Dict[str, Any]] = env_config["task_configs"]
        self.img_h = env_config.get("img_h", 1920)
        self.img_w = env_config.get("img_w", 1080)
        self.timeout = env_config["timeout"]
        self.env_id = None
        self.uid = None
        self.step_count = 0
        self.done = False
        self.prompt = None

    def reset(self):
        ret, meta_info = self.reset_attempt()
        while ret is None:
            print_debug("[CBEnvWorkerClient] Reset failed, retrying...")
            ret, meta_info = self.reset_attempt()
        return ret, meta_info

    def reset_attempt(self):
        print_debug("[CBEnvWorkerClient] Resetting environment...")
        if not self.task_configs:
            print_debug("[CBEnvWorkerClient] ERROR: No task configs available")
            return None, None

        # Select a random task config
        task_config = random.choice(self.task_configs)
        env_path = task_config["env_path"]
        task_index = task_config.get("task_index", 0)
        split = task_config.get("split", "train")
        print_debug(
            f"[CBEnvWorkerClient] Selected env_path: {env_path}, task_index: {task_index}, split: {split}"
        )

        try:
            headers = {"Content-Type": "application/json"}
            print_debug(f"[CBEnvWorkerClient] Sending reset request to {self.server_url}/reset")
            reset = requests.post(
                f"{self.server_url}/reset",
                headers=headers,
                json={
                    "env_path": env_path,
                    "task_index": task_index,
                    "split": split,
                    "timeout": self.timeout,
                },
            )
            reset.raise_for_status()
            reset_data = reset.json()
            print_debug("[CBEnvWorkerClient] Reset successful, received response data")
            jpg_string = reset_data["screenshot"]
            instruction = reset_data["instruction"]
            self.env_id = reset_data["env_id"]
            print_debug(f"[CBEnvWorkerClient] Env ID assigned: {self.env_id}")

        except requests.RequestException as e:
            print_debug(f"[CBEnvWorkerClient] ERROR in reset processing: {e}")
            return None, None

        jpg_string = self.check_and_resize_image(jpg_string)
        self.prompt = {
            "instruction": instruction,
            "steps": [f"{self.vision_start_token}{jpg_string}{self.vision_end_token}"],
        }
        self.uid = str(uuid.uuid4())
        self.step_count = 0
        self.done = False

        obs = self.prompt_to_input_obs(self.prompt)
        ret = {"obs": obs, "done": False, "is_init": True}
        meta_info = {"uid": self.uid}
        return ret, meta_info

    def prompt_to_input_obs(self, prompt):
        obs = prompt["instruction"]
        for s in prompt["steps"]:
            obs = obs + s
        return obs

    def check_and_fix_action(self, action_str):
        """Parse action string and return (normalized_str, Action object for server)."""
        print_debug(f"[CBEnvWorkerClient ID: {self.env_id}] Processing action: {action_str}")
        fn_name, args = parse_action(action_str)
        default_action = WaitAction(seconds=1.0)

        if fn_name is None or fn_name not in self.valid_fn_names:
            return "wait()", default_action

        if fn_name == "click":
            if not check_type(args, [int, int]):
                return "wait()", default_action
            x = int(args[0] / 1000.0 * self.img_w)
            y = int(args[1] / 1000.0 * self.img_h)
            return f"click({args[0]},{args[1]})", ClickAction(x=x, y=y)

        elif fn_name == "left_double":
            if not check_type(args, [int, int]):
                return "wait()", default_action
            x = int(args[0] / 1000.0 * self.img_w)
            y = int(args[1] / 1000.0 * self.img_h)
            return f"left_double({args[0]},{args[1]})", DoubleClickAction(x=x, y=y)

        elif fn_name == "right_single":
            if not check_type(args, [int, int]):
                return "wait()", default_action
            x = int(args[0] / 1000.0 * self.img_w)
            y = int(args[1] / 1000.0 * self.img_h)
            return f"right_single({args[0]},{args[1]})", RightClickAction(x=x, y=y)

        elif fn_name == "drag":
            if not check_type(args, [int, int, int, int]):
                return "wait()", default_action
            x1 = int(args[0] / 1000.0 * self.img_w)
            y1 = int(args[1] / 1000.0 * self.img_h)
            x2 = int(args[2] / 1000.0 * self.img_w)
            y2 = int(args[3] / 1000.0 * self.img_h)
            return f"drag({args[0]},{args[1]},{args[2]},{args[3]})", DragAction(
                from_x=x1, from_y=y1, to_x=x2, to_y=y2
            )

        elif fn_name == "hotkey":
            args_str = ",".join(args)
            return f"hotkey({args_str})", HotkeyAction(keys=list(args))

        elif fn_name == "type":
            if not check_type(args, [str]):
                return "wait()", default_action
            return f"type({args[0]})", TypeAction(text=args[0])

        elif fn_name == "scroll":
            if not check_type(args, [int, int, str]):
                return "wait()", default_action
            direction = args[2]
            if direction not in ["up", "down"]:
                return "wait()", default_action
            amount = 100 if direction == "up" else -100
            return f"scroll({args[0]},{args[1]},{args[2]})", ScrollAction(
                direction=direction, amount=abs(amount)
            )

        elif fn_name == "wait":
            if len(args) != 0:
                return "wait()", default_action
            return "wait()", WaitAction(seconds=1.0)

        elif fn_name == "call_user":
            # Treat call_user as wait since there's no equivalent
            if len(args) != 0:
                return "wait()", default_action
            return "call_user()", WaitAction(seconds=1.0)

        elif fn_name == "done":
            if len(args) != 0:
                return "wait()", default_action
            return "done()", DoneAction()

        else:
            raise ValueError(f"Unexpected action: {fn_name}")

    def reward_shaping(self, reward):
        return reward

    def check_and_resize_image(self, jpg_string):
        print_debug(f"[CBEnvWorkerClient ID: {self.env_id}] Processing and resizing image...")
        img = jpg_string_to_pillow(jpg_string)
        w, h = img.size
        if self.dynamic_img_size:
            self.img_w = w
            self.img_h = h
        assert w == self.img_w and h == self.img_h
        img_rs = img.resize((self.vlm_img_w, self.vlm_img_h))
        jpg_string_rs = pillow_to_jpg_string(img_rs)
        return jpg_string_rs

    def step(self, action):
        ret, meta_info = self.step_attempt(action)
        while ret is None:
            print_debug(f"[CBEnvWorkerClient ID: {self.env_id}] Step failed, retrying...")
            ret, meta_info = self.step_attempt(action)
        return ret, meta_info

    def step_attempt(self, action):
        print_debug(
            f"[CBEnvWorkerClient ID: {self.env_id}] Step {self.step_count + 1} with action: {action}"
        )
        assert not self.done
        assert isinstance(action, str)
        prev_obs = self.prompt_to_input_obs(self.prompt)
        try:
            print_debug(f"[CBEnvWorkerClient ID: {self.env_id}] Parsing action tokens...")
            think_pattern = (
                re.escape(self.think_start_token) + r"(.*?)" + re.escape(self.think_end_token)
            )
            action_pattern = (
                re.escape(self.action_start_token) + r"(.*?)" + re.escape(self.action_end_token)
            )

            think_match = re.search(think_pattern, action)
            action_match = re.search(action_pattern, action)

            think_str = think_match.group(1).strip() if think_match else ""
            action_str = action_match.group(1).strip() if action_match else ""
            print_debug(
                f"[CBEnvWorkerClient ID: {self.env_id}] Parsed action tokens: think_str={think_str}, action_str={action_str}"
            )
        except Exception as e:
            print_debug(f"[CBEnvWorkerClient ID: {self.env_id}] ERROR in action parsing: {e}")
            return None, None

        action_str, action_obj = self.check_and_fix_action(action_str)
        print_debug(f"[CBEnvWorkerClient ID: {self.env_id}] Normalized action: {action_str}")
        print_debug(f"[CBEnvWorkerClient ID: {self.env_id}] Action object: {action_obj}")

        if self.step_count >= self.max_step:
            print_debug(f"[CBEnvWorkerClient ID: {self.env_id}] Max steps reached, forcing done()")
            action_obj = DoneAction()
            action_str = "done()"

        try:
            print_debug(
                f"[CBEnvWorkerClient ID: {self.env_id}] Sending step request to {self.server_url}/step"
            )
            action_dict = action_to_dict(action_obj)
            step_ret = requests.post(
                f"{self.server_url}/step", json={"action": action_dict, "env_id": self.env_id}
            )
            step_ret.raise_for_status()
            step_data = step_ret.json()
            print_debug(f"[CBEnvWorkerClient ID: {self.env_id}] Step request successful")

            jpg_string = step_data["screenshot"]
            reward = step_data["reward"]
            done = step_data["done"]
            print_debug(
                f"[CBEnvWorkerClient ID: {self.env_id}] Step result - reward: {reward}, done: {done}"
            )

            assert isinstance(reward, (int, float))
            assert isinstance(done, bool)
            if isinstance(action_obj, DoneAction):
                assert done
            self.done = done
        except Exception as e:
            print_debug(f"[CBEnvWorkerClient ID: {self.env_id}] ERROR in step execution: {e}")
            return None, None

        action_clean = (
            f"{self.think_start_token}{think_str}{self.think_end_token}"
            + f"{self.action_start_token}{action_str}{self.action_end_token}"
        )

        jpg_string = self.check_and_resize_image(jpg_string)
        reward = self.reward_shaping(reward)

        self.step_count += 1
        self.prompt["steps"].append(action_clean)
        self.prompt["steps"].append(f"{self.vision_start_token}{jpg_string}{self.vision_end_token}")
        curr_obs = self.prompt_to_input_obs(self.prompt)

        ret = {
            "prev_obs": prev_obs,
            "action": action_clean,
            "obs": curr_obs,
            "reward": reward,
            "done": self.done,
            "is_init": False,
        }
        meta_info = {"uid": self.uid}
        print_debug(
            f"[CBEnvWorkerClient ID: {self.env_id}] Step {self.step_count} completed successfully"
        )
        return ret, meta_info

    def render(self):
        """
        Renders the current state in self.prompt as a sequence of text-image pairs into a single image
        Returns:
            PIL.Image: Combined image showing the instruction and interaction history
        """
        import re

        from PIL import Image, ImageDraw, ImageFont

        # Constants for rendering
        PADDING = 20
        TEXT_HEIGHT = 100
        FONT_SIZE = 20
        CLICK_RADIUS = 5
        CLICK_COLOR = "red"
        try:
            font = ImageFont.truetype("DejaVuSans.ttf", FONT_SIZE)
        except Exception:
            font = ImageFont.load_default()

        # First calculate total height needed
        total_height = PADDING
        total_height += TEXT_HEIGHT  # Initial instruction

        for step in self.prompt["steps"]:
            if self.vision_start_token in step:
                # Image height + padding
                total_height += self.vlm_img_h + PADDING
            else:
                # Text height + padding
                total_height += TEXT_HEIGHT + PADDING

        # Create blank image
        combined_img = Image.new("RGB", (self.vlm_img_w + 2 * PADDING, total_height), "white")
        draw = ImageDraw.Draw(combined_img)

        # Start drawing from top
        y_offset = PADDING

        # Draw instruction
        draw.text((PADDING, y_offset), self.prompt["instruction"], font=font, fill="black")
        y_offset += TEXT_HEIGHT + PADDING

        # Draw each step
        for step in self.prompt["steps"]:
            if self.vision_start_token in step:
                # Extract and paste image
                img_str = step[len(self.vision_start_token) : -len(self.vision_end_token)]
                img = jpg_string_to_pillow(img_str)
                combined_img.paste(img, (PADDING, y_offset))

                # Check previous step for click coordinates
                if len(self.prompt["steps"]) > 1:
                    prev_step = self.prompt["steps"][-2]  # Get the step before the image
                    if "<|action_start|>" in prev_step and "<|action_end|>" in prev_step:
                        action = prev_step.split("<|action_start|>")[1].split("<|action_end|>")[0]
                        # Look for click(x,y) pattern
                        click_match = re.search(r"click\((\d+),(\d+)\)", action)
                        if click_match:
                            # Get normalized coordinates
                            norm_x, norm_y = map(int, click_match.groups())
                            # Denormalize coordinates (assuming coordinates are normalized to 100)
                            x = int((norm_x / 1000.0) * self.vlm_img_w)
                            y = int((norm_y / 1000.0) * self.vlm_img_h)
                            # Draw click point
                            draw.ellipse(
                                [
                                    PADDING + x - CLICK_RADIUS,
                                    y_offset + y - CLICK_RADIUS,
                                    PADDING + x + CLICK_RADIUS,
                                    y_offset + y + CLICK_RADIUS,
                                ],
                                fill=CLICK_COLOR,
                            )

                y_offset += self.vlm_img_h + PADDING
            else:
                # Draw text (action/thought)
                draw.text((PADDING, y_offset), step, font=font, fill="black")
                y_offset += TEXT_HEIGHT + PADDING

        return combined_img
