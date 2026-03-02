"""Shared test fixtures for cua-cli tests."""

import sys
from unittest.mock import AsyncMock, MagicMock, patch

import pytest

# Mock external modules before importing cua_cli
# This allows tests to run without cua-computer installed
mock_providers = MagicMock()
mock_providers.VMProviderFactory = MagicMock()
mock_providers.VMProviderType = MagicMock()
mock_providers.VMProviderType.CLOUD = "cloud"

mock_computer = MagicMock()
mock_computer.providers = mock_providers
mock_computer.providers.cloud = MagicMock()
mock_computer.providers.cloud.provider = MagicMock()
mock_computer.providers.cloud.provider.CloudProvider = MagicMock()

sys.modules["computer"] = mock_computer
sys.modules["computer.providers"] = mock_providers
sys.modules["computer.providers.cloud"] = mock_computer.providers.cloud
sys.modules["computer.providers.cloud.provider"] = mock_computer.providers.cloud.provider


@pytest.fixture
def disable_telemetry(monkeypatch):
    """Disable telemetry for tests."""
    monkeypatch.setenv("CUA_TELEMETRY_DISABLED", "1")


@pytest.fixture
def temp_credentials_db(tmp_path, monkeypatch):
    """Provide a temporary credentials database."""
    cua_dir = tmp_path / ".cua"
    cua_dir.mkdir(parents=True)
    db_path = cua_dir / "credentials.db"

    # Patch the CREDENTIALS_DB path in the store module
    monkeypatch.setattr("cua_cli.auth.store.CREDENTIALS_DB", db_path)

    yield db_path

    if db_path.exists():
        db_path.unlink()


@pytest.fixture
def mock_api_key(monkeypatch):
    """Set a mock API key in environment."""
    monkeypatch.setenv("CUA_API_KEY", "test-api-key-12345")
    return "test-api-key-12345"


@pytest.fixture
def clear_api_key_env(monkeypatch):
    """Ensure no API key is in environment."""
    monkeypatch.delenv("CUA_API_KEY", raising=False)


@pytest.fixture
def mock_aiohttp_session():
    """Mock aiohttp.ClientSession for API tests."""
    with patch("aiohttp.ClientSession") as mock_session:
        mock_instance = AsyncMock()
        mock_session.return_value.__aenter__.return_value = mock_instance
        mock_session.return_value.__aexit__.return_value = None
        yield mock_instance


@pytest.fixture
def mock_cloud_provider():
    """Mock CloudProvider for sandbox tests."""
    with patch("cua_cli.commands.sandbox.VMProviderFactory") as mock_factory:
        mock_provider = AsyncMock()
        mock_provider.__aenter__.return_value = mock_provider
        mock_provider.__aexit__.return_value = None
        mock_factory.create_provider.return_value = mock_provider
        yield mock_provider


@pytest.fixture
def sample_vm():
    """Sample VM object for testing."""
    vm = MagicMock()
    vm.name = "test-sandbox-1"
    vm.status = "running"
    vm.os_type = "linux"
    vm.created_at = "2024-01-15T10:00:00Z"
    vm.size = "medium"
    vm.region = "north-america"
    vm.vnc_url = "https://vnc.example.com/test-sandbox-1"
    vm.server_url = "https://server.example.com:8000"
    return vm


@pytest.fixture
def sample_vm_list(sample_vm):
    """Sample VM list for testing."""
    vm2 = MagicMock()
    vm2.name = "test-sandbox-2"
    vm2.status = "stopped"
    vm2.os_type = "macos"
    vm2.created_at = "2024-01-14T08:30:00Z"
    vm2.size = "large"
    vm2.region = "europe"
    vm2.vnc_url = None
    vm2.server_url = None
    return [sample_vm, vm2]


@pytest.fixture
def temp_skills_dir(tmp_path, monkeypatch):
    """Provide a temporary skills directory."""
    skills_dir = tmp_path / ".cua" / "skills"
    skills_dir.mkdir(parents=True)

    # Patch the SKILLS_DIR in the skills module
    monkeypatch.setattr("cua_cli.commands.skills.SKILLS_DIR", skills_dir)

    return skills_dir


@pytest.fixture
def sample_skill(temp_skills_dir):
    """Create a sample skill for testing."""
    skill_dir = temp_skills_dir / "test-skill"
    skill_dir.mkdir()

    # Create SKILL.md with proper frontmatter format expected by _parse_frontmatter
    skill_file = skill_dir / "SKILL.md"
    skill_file.write_text(
        """---
name: test-skill
description: A sample skill for testing
---

# Test Skill

A sample skill for testing.

## Steps

1. Click on the button
2. Type some text
3. Press Enter
"""
    )

    # Create trajectory directory with trajectory.json and a video file
    trajectory_dir = skill_dir / "trajectory"
    trajectory_dir.mkdir()

    # Create trajectory.json for proper skill info extraction
    trajectory_json = trajectory_dir / "trajectory.json"
    trajectory_json.write_text(
        """{
        "trajectory": [
            {"step_idx": 1, "caption": {"action": "click"}},
            {"step_idx": 2, "caption": {"action": "type"}}
        ],
        "metadata": {
            "created_at": "2024-01-15T10:00:00Z"
        }
    }"""
    )

    # Create a fake MP4 file for replay tests
    video_file = trajectory_dir / "test-skill.mp4"
    video_file.write_bytes(b"fake mp4 content")

    return skill_dir


@pytest.fixture
def mock_image_api_client():
    """Mock CloudAPIClient for image tests."""
    with patch("cua_cli.commands.image.CloudAPIClient") as mock_client_class:
        mock_client = AsyncMock()
        mock_client_class.return_value = mock_client
        yield mock_client


@pytest.fixture
def sample_cloud_images():
    """Sample cloud images for testing."""
    return [
        {
            "name": "ubuntu-22.04",
            "image_type": "qcow2",
            "created_at": "2024-01-10T12:00:00Z",
            "versions": [
                {
                    "tag": "latest",
                    "size_bytes": 5368709120,
                    "status": "ready",
                    "created_at": "2024-01-10T12:00:00Z",
                },
                {
                    "tag": "v1.0",
                    "size_bytes": 5000000000,
                    "status": "ready",
                    "created_at": "2024-01-05T10:00:00Z",
                },
            ],
        },
        {
            "name": "macos-sonoma",
            "image_type": "qcow2",
            "created_at": "2024-01-08T09:00:00Z",
            "versions": [
                {
                    "tag": "latest",
                    "size_bytes": 21474836480,
                    "status": "ready",
                    "created_at": "2024-01-08T09:00:00Z",
                },
            ],
        },
    ]


@pytest.fixture
def temp_image_file(tmp_path):
    """Create a temporary image file for upload tests."""
    image_file = tmp_path / "test-image" / "data.img"
    image_file.parent.mkdir(parents=True)
    # Create a small test file
    image_file.write_bytes(b"fake image content" * 1000)
    return image_file


@pytest.fixture
def mock_webbrowser():
    """Mock webbrowser.open for browser tests."""
    with patch("webbrowser.open") as mock_open:
        yield mock_open


@pytest.fixture
def mock_rich_console():
    """Mock rich console for output tests."""
    with patch("cua_cli.utils.output.Console") as mock_console_class:
        mock_console = MagicMock()
        mock_console_class.return_value = mock_console
        yield mock_console


@pytest.fixture
def args_namespace():
    """Create an argparse.Namespace factory for command tests."""
    import argparse

    def create_args(**kwargs):
        return argparse.Namespace(**kwargs)

    return create_args
