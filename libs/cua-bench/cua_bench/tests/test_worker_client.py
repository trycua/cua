"""Tests for the CBEnvWorkerClient HTTP client interface.

This module tests the CBEnvWorkerClient which communicates with worker servers
via REST API endpoints.
"""

import base64
import io
from unittest.mock import MagicMock, patch

from cua_bench.workers.worker_client import CBEnvWorkerClient
from PIL import Image


def create_test_image(width=1024, height=768, color="blue"):
    """Create a test image and return base64 encoded string."""
    img = Image.new("RGB", (width, height), color=color)
    buffer = io.BytesIO()
    img.save(buffer, format="PNG")
    return base64.b64encode(buffer.getvalue()).decode("utf-8")


class TestCBEnvWorkerClientInit:
    """Tests for CBEnvWorkerClient initialization."""

    def test_init_with_env_config(self):
        """Test client initialization with env_config dict."""
        task_configs = [{"env_path": "./task", "task_index": 0, "split": "train"}]
        env_config = {
            "server_url": "http://localhost:8001",
            "task_configs": task_configs,
            "max_step": 50,
            "max_hist": 10,
            "timeout": 300,
        }
        client = CBEnvWorkerClient(env_config)

        assert client.server_url == "http://localhost:8001"
        assert client.max_step == 50
        assert client.max_hist == 10
        assert client.timeout == 300
        assert client.task_configs == task_configs
        assert client.env_id is None
        assert client.uid is None
        assert client.step_count == 0
        assert client.done is False

    def test_init_custom_values(self):
        """Test client initialization with custom values."""
        task_configs = [{"env_path": "./custom", "task_index": 5, "split": "test"}]
        env_config = {
            "server_url": "http://10.0.0.5:9000",
            "task_configs": task_configs,
            "max_step": 100,
            "max_hist": 20,
            "timeout": 600,
        }
        client = CBEnvWorkerClient(env_config)

        assert client.server_url == "http://10.0.0.5:9000"
        assert client.max_step == 100
        assert client.max_hist == 20
        assert client.timeout == 600


class TestCBEnvWorkerClientReset:
    """Tests for CBEnvWorkerClient.reset() - the /reset endpoint."""

    @patch("requests.post")
    def test_reset_calls_correct_endpoint(self, mock_post):
        """Test that reset() calls POST /reset."""
        img_b64 = create_test_image(1024, 768)

        mock_response = MagicMock()
        mock_response.json.return_value = {
            "screenshot": img_b64,
            "instruction": "Click the button",
            "env_id": 0,
        }
        mock_post.return_value = mock_response

        task_configs = [{"env_path": "./task", "task_index": 0, "split": "train"}]
        env_config = {
            "server_url": "http://localhost:8001",
            "task_configs": task_configs,
            "max_step": 50,
            "max_hist": 10,
            "timeout": 300,
        }
        client = CBEnvWorkerClient(env_config)
        client.reset()

        # Verify POST /reset was called
        mock_post.assert_called_once()
        call_args = mock_post.call_args
        assert "/reset" in call_args[0][0]
        assert call_args[1]["json"]["env_path"] == "./task"
        assert call_args[1]["json"]["task_index"] == 0

    @patch("requests.post")
    def test_reset_returns_observation(self, mock_post):
        """Test that reset() returns observation dict."""
        img_b64 = create_test_image(1024, 768)

        mock_response = MagicMock()
        mock_response.json.return_value = {
            "screenshot": img_b64,
            "instruction": "Click the red button",
            "env_id": 0,
        }
        mock_post.return_value = mock_response

        task_configs = [{"env_path": "./task", "task_index": 0, "split": "train"}]
        env_config = {
            "server_url": "http://localhost:8001",
            "task_configs": task_configs,
            "max_step": 50,
            "max_hist": 10,
            "timeout": 300,
        }
        client = CBEnvWorkerClient(env_config)
        env_ret, meta_info = client.reset()

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
        img_b64 = create_test_image(1024, 768)

        mock_response = MagicMock()
        mock_response.json.return_value = {
            "screenshot": img_b64,
            "instruction": "Test",
            "env_id": 5,
        }
        mock_post.return_value = mock_response

        task_configs = [{"env_path": "./task", "task_index": 0, "split": "train"}]
        env_config = {
            "server_url": "http://localhost:8001",
            "task_configs": task_configs,
            "max_step": 50,
            "max_hist": 10,
            "timeout": 300,
        }
        client = CBEnvWorkerClient(env_config)
        client.reset()

        assert client.env_id == 5

    @patch("requests.post")
    def test_reset_clears_state(self, mock_post):
        """Test that reset() clears previous episode state."""
        img_b64 = create_test_image(1024, 768)

        mock_response = MagicMock()
        mock_response.json.return_value = {
            "screenshot": img_b64,
            "instruction": "Test",
            "env_id": 0,
        }
        mock_post.return_value = mock_response

        task_configs = [{"env_path": "./task", "task_index": 0, "split": "train"}]
        env_config = {
            "server_url": "http://localhost:8001",
            "task_configs": task_configs,
            "max_step": 50,
            "max_hist": 10,
            "timeout": 300,
        }
        client = CBEnvWorkerClient(env_config)
        client.step_count = 10
        client.done = True

        client.reset()

        assert client.step_count == 0
        assert client.done is False


class TestCBEnvWorkerClientStep:
    """Tests for CBEnvWorkerClient.step() - the /step endpoint."""

    @patch("requests.post")
    def test_step_calls_correct_endpoint(self, mock_post):
        """Test that step() calls POST /step."""
        img_b64 = create_test_image(1024, 768)

        # Setup mock for both reset and step
        def mock_post_side_effect(url, **kwargs):
            mock_resp = MagicMock()
            if "/reset" in url:
                mock_resp.json.return_value = {
                    "screenshot": img_b64,
                    "instruction": "Test",
                    "env_id": 0,
                }
            else:
                mock_resp.json.return_value = {
                    "screenshot": img_b64,
                    "reward": 0.0,
                    "done": False,
                }
            return mock_resp

        mock_post.side_effect = mock_post_side_effect

        task_configs = [{"env_path": "./task", "task_index": 0, "split": "train"}]
        env_config = {
            "server_url": "http://localhost:8001",
            "task_configs": task_configs,
            "max_step": 50,
            "max_hist": 10,
            "timeout": 300,
        }
        client = CBEnvWorkerClient(env_config)
        client.reset()

        # Step with action string
        action = "<|action_start|>click(500,500)<|action_end|>"
        client.step(action)

        # Verify POST /step was called (second call after reset)
        assert mock_post.call_count == 2
        step_call = mock_post.call_args_list[1]
        assert "/step" in step_call[0][0]
        assert step_call[1]["json"]["env_id"] == 0
        assert "action" in step_call[1]["json"]

    @patch("requests.post")
    def test_step_returns_observation(self, mock_post):
        """Test that step() returns observation dict."""
        img_b64 = create_test_image(1024, 768)

        def mock_post_side_effect(url, **kwargs):
            mock_resp = MagicMock()
            if "/reset" in url:
                mock_resp.json.return_value = {
                    "screenshot": img_b64,
                    "instruction": "Test",
                    "env_id": 0,
                }
            else:
                mock_resp.json.return_value = {
                    "screenshot": img_b64,
                    "reward": 0.5,
                    "done": False,
                }
            return mock_resp

        mock_post.side_effect = mock_post_side_effect

        task_configs = [{"env_path": "./task", "task_index": 0, "split": "train"}]
        env_config = {
            "server_url": "http://localhost:8001",
            "task_configs": task_configs,
            "max_step": 50,
            "max_hist": 10,
            "timeout": 300,
        }
        client = CBEnvWorkerClient(env_config)
        client.reset()

        action = "<|action_start|>click(500,500)<|action_end|>"
        env_ret, meta_info = client.step(action)

        # Check env_ret structure
        assert "obs" in env_ret
        assert "action" in env_ret
        assert "reward" in env_ret
        assert "done" in env_ret
        assert env_ret["is_init"] is False

    @patch("requests.post")
    def test_step_increments_counter(self, mock_post):
        """Test that step() increments step_count."""
        img_b64 = create_test_image(1024, 768)

        def mock_post_side_effect(url, **kwargs):
            mock_resp = MagicMock()
            if "/reset" in url:
                mock_resp.json.return_value = {
                    "screenshot": img_b64,
                    "instruction": "Test",
                    "env_id": 0,
                }
            else:
                mock_resp.json.return_value = {
                    "screenshot": img_b64,
                    "reward": 0.0,
                    "done": False,
                }
            return mock_resp

        mock_post.side_effect = mock_post_side_effect

        task_configs = [{"env_path": "./task", "task_index": 0, "split": "train"}]
        env_config = {
            "server_url": "http://localhost:8001",
            "task_configs": task_configs,
            "max_step": 50,
            "max_hist": 10,
            "timeout": 300,
        }
        client = CBEnvWorkerClient(env_config)
        client.reset()

        assert client.step_count == 0

        client.step("<|action_start|>click(500,500)<|action_end|>")
        assert client.step_count == 1

        client.step("<|action_start|>click(600,600)<|action_end|>")
        assert client.step_count == 2

    @patch("requests.post")
    def test_step_sets_done_flag(self, mock_post):
        """Test that step() sets done flag when episode ends."""
        img_b64 = create_test_image(1024, 768)

        def mock_post_side_effect(url, **kwargs):
            mock_resp = MagicMock()
            if "/reset" in url:
                mock_resp.json.return_value = {
                    "screenshot": img_b64,
                    "instruction": "Test",
                    "env_id": 0,
                }
            else:
                mock_resp.json.return_value = {
                    "screenshot": img_b64,
                    "reward": 1.0,
                    "done": True,
                }
            return mock_resp

        mock_post.side_effect = mock_post_side_effect

        task_configs = [{"env_path": "./task", "task_index": 0, "split": "train"}]
        env_config = {
            "server_url": "http://localhost:8001",
            "task_configs": task_configs,
            "max_step": 50,
            "max_hist": 10,
            "timeout": 300,
        }
        client = CBEnvWorkerClient(env_config)
        client.reset()

        env_ret, _ = client.step("<|action_start|>done()<|action_end|>")

        assert client.done is True
        assert env_ret["done"] is True
        assert env_ret["reward"] == 1.0


class TestCBEnvWorkerClientActions:
    """Tests for action processing in CBEnvWorkerClient."""

    def test_check_and_fix_action_click(self):
        """Test click action processing."""
        task_configs = [{"env_path": "./task", "task_index": 0, "split": "train"}]
        env_config = {
            "server_url": "http://localhost:8001",
            "task_configs": task_configs,
            "max_step": 50,
            "max_hist": 10,
            "timeout": 300,
        }
        client = CBEnvWorkerClient(env_config)

        action_str, action_obj = client.check_and_fix_action("click(500,500)")

        assert "click(" in action_str
        # Action object is a ClickAction with denormalized coordinates
        from cua_bench.types import ClickAction

        assert isinstance(action_obj, ClickAction)

    def test_check_and_fix_action_finish(self):
        """Test finish action processing."""
        task_configs = [{"env_path": "./task", "task_index": 0, "split": "train"}]
        env_config = {
            "server_url": "http://localhost:8001",
            "task_configs": task_configs,
            "max_step": 50,
            "max_hist": 10,
            "timeout": 300,
        }
        client = CBEnvWorkerClient(env_config)

        action_str, action_obj = client.check_and_fix_action("done()")

        assert "done()" in action_str
        from cua_bench.types import DoneAction

        assert isinstance(action_obj, DoneAction)

    def test_check_and_fix_action_invalid(self):
        """Test invalid action falls back to wait."""
        task_configs = [{"env_path": "./task", "task_index": 0, "split": "train"}]
        env_config = {
            "server_url": "http://localhost:8001",
            "task_configs": task_configs,
            "max_step": 50,
            "max_hist": 10,
            "timeout": 300,
        }
        client = CBEnvWorkerClient(env_config)

        action_str, action_obj = client.check_and_fix_action("invalid_action()")

        assert action_str == "wait()"
        from cua_bench.types import WaitAction

        assert isinstance(action_obj, WaitAction)


class TestCBEnvWorkerClientObservations:
    """Tests for observation formatting in CBEnvWorkerClient."""

    def test_prompt_to_input_obs(self):
        """Test prompt to observation conversion."""
        task_configs = [{"env_path": "./task", "task_index": 0, "split": "train"}]
        env_config = {
            "server_url": "http://localhost:8001",
            "task_configs": task_configs,
            "max_step": 50,
            "max_hist": 10,
            "timeout": 300,
        }
        client = CBEnvWorkerClient(env_config)

        prompt = {
            "instruction": "Click the button",
            "steps": [
                "<|vision_start|>img1<|vision_end|>",
                "<|action_start|>click(500,500)<|action_end|>",
                "<|vision_start|>img2<|vision_end|>",
            ],
        }

        obs = client.prompt_to_input_obs(prompt)

        assert "Click the button" in obs
        assert "<|vision_start|>img1<|vision_end|>" in obs
        assert "<|action_start|>click(500,500)<|action_end|>" in obs
        assert "<|vision_start|>img2<|vision_end|>" in obs
