"""Tests for the gym HTTP client interface (/reset, /step endpoints).

This module tests the CBEnvWorkerClient which communicates with worker servers
via REST API endpoints.
"""

import base64
import io
from unittest.mock import MagicMock, patch

import pytest
from cua_bench.workers.worker_client import CBEnvWorkerClient, create_client_from_env
from PIL import Image


class TestCBEnvWorkerClientInit:
    """Tests for CBEnvWorkerClient initialization."""

    def test_init_defaults(self):
        """Test client initialization with defaults."""
        client = CBEnvWorkerClient(server_url="http://localhost:8001")
        assert client.server_url == "http://localhost:8001"
        assert client.img_w == 1920
        assert client.img_h == 1080
        assert client.max_step == 50
        assert client.max_hist == 10
        assert client.timeout == 300
        assert client.env_id is None
        assert client.uid is None
        assert client.step_count == 0
        assert client.done is False

    def test_init_custom(self):
        """Test client initialization with custom values."""
        client = CBEnvWorkerClient(
            server_url="http://10.0.0.5:9000",
            img_w=1280,
            img_h=720,
            max_step=100,
            max_hist=20,
            timeout=600,
        )
        assert client.server_url == "http://10.0.0.5:9000"
        assert client.img_w == 1280
        assert client.img_h == 720
        assert client.max_step == 100
        assert client.max_hist == 20
        assert client.timeout == 600


class TestCBEnvWorkerClientReset:
    """Tests for CBEnvWorkerClient.reset() - the /reset endpoint."""

    @patch("requests.post")
    def test_reset_calls_correct_endpoint(self, mock_post):
        """Test that reset() calls POST /reset."""
        # Create test image
        img = Image.new("RGB", (1920, 1080), color="blue")
        buffer = io.BytesIO()
        img.save(buffer, format="PNG")
        img_b64 = base64.b64encode(buffer.getvalue()).decode("utf-8")

        mock_response = MagicMock()
        mock_response.json.return_value = {
            "screenshot": img_b64,
            "instruction": "Click the button",
            "env_id": 0,
        }
        mock_post.return_value = mock_response

        client = CBEnvWorkerClient(server_url="http://localhost:8001")
        client.reset("./task", task_index=0)

        # Verify POST /reset was called
        mock_post.assert_called_once()
        call_args = mock_post.call_args
        assert "/reset" in call_args[0][0]
        assert call_args[1]["json"]["env_path"] == "./task"
        assert call_args[1]["json"]["task_index"] == 0

    @patch("requests.post")
    def test_reset_returns_observation(self, mock_post):
        """Test that reset() returns observation dict."""
        img = Image.new("RGB", (1920, 1080), color="blue")
        buffer = io.BytesIO()
        img.save(buffer, format="PNG")
        img_b64 = base64.b64encode(buffer.getvalue()).decode("utf-8")

        mock_response = MagicMock()
        mock_response.json.return_value = {
            "screenshot": img_b64,
            "instruction": "Click the red button",
            "env_id": 0,
        }
        mock_post.return_value = mock_response

        client = CBEnvWorkerClient(server_url="http://localhost:8001")
        env_ret, meta_info = client.reset("./task", task_index=0)

        # Check env_ret structure
        assert "obs" in env_ret
        assert env_ret["done"] is False
        assert env_ret["is_init"] is True

        # Check meta_info
        assert "uid" in meta_info
        assert client.uid == meta_info["uid"]

    @patch("requests.post")
    def test_reset_stores_env_id(self, mock_post):
        """Test that reset() stores the env_id."""
        img = Image.new("RGB", (1920, 1080), color="blue")
        buffer = io.BytesIO()
        img.save(buffer, format="PNG")
        img_b64 = base64.b64encode(buffer.getvalue()).decode("utf-8")

        mock_response = MagicMock()
        mock_response.json.return_value = {
            "screenshot": img_b64,
            "instruction": "Test",
            "env_id": 5,
        }
        mock_post.return_value = mock_response

        client = CBEnvWorkerClient(server_url="http://localhost:8001")
        client.reset("./task")

        assert client.env_id == 5

    @patch("requests.post")
    def test_reset_clears_state(self, mock_post):
        """Test that reset() clears previous episode state."""
        img = Image.new("RGB", (1920, 1080), color="blue")
        buffer = io.BytesIO()
        img.save(buffer, format="PNG")
        img_b64 = base64.b64encode(buffer.getvalue()).decode("utf-8")

        mock_response = MagicMock()
        mock_response.json.return_value = {
            "screenshot": img_b64,
            "instruction": "Test",
            "env_id": 0,
        }
        mock_post.return_value = mock_response

        client = CBEnvWorkerClient(server_url="http://localhost:8001")
        client.step_count = 10
        client.done = True

        client.reset("./task")

        assert client.step_count == 0
        assert client.done is False


class TestCBEnvWorkerClientStep:
    """Tests for CBEnvWorkerClient.step() - the /step endpoint."""

    @patch("requests.post")
    def test_step_calls_correct_endpoint(self, mock_post):
        """Test that step() calls POST /step."""
        img = Image.new("RGB", (1920, 1080), color="green")
        buffer = io.BytesIO()
        img.save(buffer, format="PNG")
        img_b64 = base64.b64encode(buffer.getvalue()).decode("utf-8")

        mock_response = MagicMock()
        mock_response.json.return_value = {
            "screenshot": img_b64,
            "reward": 0.0,
            "done": False,
        }
        mock_post.return_value = mock_response

        client = CBEnvWorkerClient(server_url="http://localhost:8001")
        client.env_id = 0
        client.uid = "test-uid"
        client.prompt = {"instruction": "Test", "steps": ["<|vision_start|>img<|vision_end|>"]}
        client._orig_w = 1920
        client._orig_h = 1080

        action = {"type": "ClickAction", "x": 100, "y": 200}
        client.step(action)

        # Verify POST /step was called
        mock_post.assert_called_once()
        call_args = mock_post.call_args
        assert "/step" in call_args[0][0]
        assert call_args[1]["json"]["env_id"] == 0
        assert "action" in call_args[1]["json"]

    @patch("requests.post")
    def test_step_returns_observation(self, mock_post):
        """Test that step() returns observation dict."""
        img = Image.new("RGB", (1920, 1080), color="green")
        buffer = io.BytesIO()
        img.save(buffer, format="PNG")
        img_b64 = base64.b64encode(buffer.getvalue()).decode("utf-8")

        mock_response = MagicMock()
        mock_response.json.return_value = {
            "screenshot": img_b64,
            "reward": 0.5,
            "done": False,
        }
        mock_post.return_value = mock_response

        client = CBEnvWorkerClient(server_url="http://localhost:8001")
        client.env_id = 0
        client.uid = "test-uid"
        client.prompt = {"instruction": "Test", "steps": ["<|vision_start|>img<|vision_end|>"]}
        client._orig_w = 1920
        client._orig_h = 1080

        env_ret, meta_info = client.step({"type": "ClickAction", "x": 100, "y": 200})

        # Check env_ret structure
        assert "obs" in env_ret
        assert "action" in env_ret
        assert "reward" in env_ret
        assert "done" in env_ret
        assert env_ret["is_init"] is False

    @patch("requests.post")
    def test_step_increments_counter(self, mock_post):
        """Test that step() increments step_count."""
        img = Image.new("RGB", (1920, 1080), color="green")
        buffer = io.BytesIO()
        img.save(buffer, format="PNG")
        img_b64 = base64.b64encode(buffer.getvalue()).decode("utf-8")

        mock_response = MagicMock()
        mock_response.json.return_value = {
            "screenshot": img_b64,
            "reward": 0.0,
            "done": False,
        }
        mock_post.return_value = mock_response

        client = CBEnvWorkerClient(server_url="http://localhost:8001")
        client.env_id = 0
        client.uid = "test-uid"
        client.prompt = {"instruction": "Test", "steps": ["<|vision_start|>img<|vision_end|>"]}
        client._orig_w = 1920
        client._orig_h = 1080

        assert client.step_count == 0

        client.step({"type": "ClickAction", "x": 100, "y": 200})
        assert client.step_count == 1

        client.step({"type": "ClickAction", "x": 200, "y": 300})
        assert client.step_count == 2

    @patch("requests.post")
    def test_step_sets_done_flag(self, mock_post):
        """Test that step() sets done flag when episode ends."""
        img = Image.new("RGB", (1920, 1080), color="green")
        buffer = io.BytesIO()
        img.save(buffer, format="PNG")
        img_b64 = base64.b64encode(buffer.getvalue()).decode("utf-8")

        mock_response = MagicMock()
        mock_response.json.return_value = {
            "screenshot": img_b64,
            "reward": 1.0,
            "done": True,
        }
        mock_post.return_value = mock_response

        client = CBEnvWorkerClient(server_url="http://localhost:8001")
        client.env_id = 0
        client.uid = "test-uid"
        client.prompt = {"instruction": "Test", "steps": ["<|vision_start|>img<|vision_end|>"]}
        client._orig_w = 1920
        client._orig_h = 1080

        env_ret, _ = client.step({"type": "DoneAction"})

        assert client.done is True
        assert env_ret["done"] is True
        assert env_ret["reward"] == 1.0

    def test_step_requires_reset(self):
        """Test that step() raises when reset hasn't been called."""
        client = CBEnvWorkerClient(server_url="http://localhost:8001")

        with pytest.raises(RuntimeError, match="not initialized"):
            client.step({"type": "ClickAction", "x": 100, "y": 200})

    def test_step_after_done_raises(self):
        """Test that step() raises after episode is done."""
        client = CBEnvWorkerClient(server_url="http://localhost:8001")
        client.env_id = 0
        client.done = True

        with pytest.raises(RuntimeError, match="Episode is done"):
            client.step({"type": "ClickAction", "x": 100, "y": 200})


class TestCBEnvWorkerClientActions:
    """Tests for action processing in CBEnvWorkerClient."""

    def test_process_action_click(self):
        """Test click action processing."""
        client = CBEnvWorkerClient(server_url="http://localhost:8001")
        client._orig_w = 1920
        client._orig_h = 1080

        action = {"type": "click", "x": 960, "y": 540}
        action_str, action_denorm = client._process_action(action)

        assert "click(" in action_str
        assert action_denorm["type"] == "ClickAction"
        assert action_denorm["x"] == 960
        assert action_denorm["y"] == 540

    def test_process_action_normalized_coords(self):
        """Test action processing with normalized coordinates."""
        client = CBEnvWorkerClient(server_url="http://localhost:8001")
        client._orig_w = 1920
        client._orig_h = 1080

        # Normalized coordinates (0.5, 0.5) should map to center
        action = {"type": "click", "x": 0.5, "y": 0.5}
        action_str, action_denorm = client._process_action(action)

        assert action_denorm["x"] == 960  # 0.5 * 1920
        assert action_denorm["y"] == 540  # 0.5 * 1080

    def test_process_action_type(self):
        """Test type action processing."""
        client = CBEnvWorkerClient(server_url="http://localhost:8001")

        action = {"type": "TypeAction", "text": "Hello World"}
        action_str, action_denorm = client._process_action(action)

        assert 'type("Hello World")' in action_str
        assert action_denorm["text"] == "Hello World"

    def test_process_action_done(self):
        """Test done action processing."""
        client = CBEnvWorkerClient(server_url="http://localhost:8001")

        action = {"type": "DoneAction"}
        action_str, action_denorm = client._process_action(action)

        assert "done()" in action_str
        assert action_denorm["type"] == "DoneAction"


class TestCBEnvWorkerClientScreenshots:
    """Tests for screenshot processing in CBEnvWorkerClient."""

    def test_process_screenshot_resize(self):
        """Test screenshot processing and resizing."""
        client = CBEnvWorkerClient(
            server_url="http://localhost:8001",
            img_w=640,
            img_h=480,
        )

        # Create a test image at original resolution
        img = Image.new("RGB", (1920, 1080), color="red")
        buffer = io.BytesIO()
        img.save(buffer, format="PNG")
        img_bytes = buffer.getvalue()
        img_b64 = base64.b64encode(img_bytes).decode("utf-8")

        # Process the screenshot
        result = client._process_screenshot(img_b64)

        # Verify result is valid base64 JPEG at target resolution
        result_bytes = base64.b64decode(result)
        result_img = Image.open(io.BytesIO(result_bytes))
        assert result_img.size == (640, 480)
        assert result_img.format == "JPEG"

    def test_process_screenshot_stores_original_dims(self):
        """Test that processing stores original dimensions."""
        client = CBEnvWorkerClient(server_url="http://localhost:8001")

        img = Image.new("RGB", (2560, 1440), color="blue")
        buffer = io.BytesIO()
        img.save(buffer, format="PNG")
        img_b64 = base64.b64encode(buffer.getvalue()).decode("utf-8")

        client._process_screenshot(img_b64)

        assert client._orig_w == 2560
        assert client._orig_h == 1440


class TestCBEnvWorkerClientObservations:
    """Tests for observation formatting in CBEnvWorkerClient."""

    def test_prompt_to_obs(self):
        """Test prompt to observation conversion."""
        client = CBEnvWorkerClient(server_url="http://localhost:8001")

        prompt = {
            "instruction": "Click the button",
            "steps": [
                "<|vision_start|>img1<|vision_end|>",
                "<|action_start|>click(0.5,0.5)<|action_end|>",
                "<|vision_start|>img2<|vision_end|>",
            ],
        }

        obs = client._prompt_to_obs(prompt)

        assert "<|instruction|>Click the button<|instruction_end|>" in obs
        assert "<|vision_start|>img1<|vision_end|>" in obs
        assert "<|action_start|>click(0.5,0.5)<|action_end|>" in obs
        assert "<|vision_start|>img2<|vision_end|>" in obs

    def test_prompt_to_obs_none(self):
        """Test prompt to observation with None prompt."""
        client = CBEnvWorkerClient(server_url="http://localhost:8001")
        obs = client._prompt_to_obs(None)
        assert obs == ""


class TestCreateClientFromEnv:
    """Tests for create_client_from_env function."""

    @patch.dict(
        "os.environ",
        {
            "OSGYM_SERVER_URL": "http://custom:9000",
            "OSGYM_IMG_W": "1280",
            "OSGYM_IMG_H": "720",
            "OSGYM_MAX_STEP": "100",
            "OSGYM_MAX_HIST": "20",
            "OSGYM_TIMEOUT": "600",
        },
    )
    def test_create_from_env_vars(self):
        """Test creating client from environment variables."""
        client = create_client_from_env()
        assert client.server_url == "http://custom:9000"
        assert client.img_w == 1280
        assert client.img_h == 720
        assert client.max_step == 100
        assert client.max_hist == 20
        assert client.timeout == 600

    @patch.dict("os.environ", {}, clear=True)
    def test_create_from_env_defaults(self):
        """Test creating client with defaults when env vars missing."""
        client = create_client_from_env()
        assert client.server_url == "http://localhost:8001"
        assert client.img_w == 1920
        assert client.img_h == 1080

    def test_create_from_env_override(self):
        """Test creating client with override parameters."""
        client = create_client_from_env(
            server_url="http://override:8000",
            img_w=800,
        )
        assert client.server_url == "http://override:8000"
        assert client.img_w == 800
