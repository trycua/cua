"""
Critical test cases for the snapshot manager system.
"""

import shutil
import tempfile
from unittest.mock import AsyncMock, Mock

import docker
import pytest

from snapshot_manager.manager import SnapshotManager
from snapshot_manager.models import SnapshotConfig, SnapshotStatus, SnapshotTrigger
from snapshot_manager.providers.docker_provider import DockerSnapshotProvider
from snapshot_manager.storage import FileSystemSnapshotStorage


class TestSnapshotManager:
    """Test cases for the SnapshotManager class."""

    @pytest.fixture
    def temp_storage(self):
        """Create a temporary storage directory for testing."""
        temp_dir = tempfile.mkdtemp()
        yield temp_dir
        shutil.rmtree(temp_dir)

    @pytest.fixture
    def mock_docker_client(self):
        """Mock Docker client for testing."""
        mock_client = Mock(spec=docker.DockerClient)
        mock_client.ping.return_value = True

        # Mock container
        mock_container = Mock()
        mock_container.id = "test_container_id"
        mock_container.name = "test_container"
        mock_container.status = "running"
        mock_container.image.id = "test_image_id"
        mock_container.labels = {}
        mock_container.attrs = {
            "Created": "2024-01-01T00:00:00Z",
            "Config": {"Env": [], "WorkingDir": "/app"},
            "Mounts": [],
            "NetworkSettings": {},
        }

        # Mock commit operation
        mock_image = Mock()
        mock_image.id = "snapshot_image_id"
        mock_image.attrs = {"Size": 1024 * 1024 * 100}  # 100MB
        mock_container.commit.return_value = mock_image

        mock_client.containers.get.return_value = mock_container
        mock_client.containers.run.return_value = mock_container
        mock_client.images.get.return_value = mock_image

        return mock_client

    @pytest.fixture
    def snapshot_manager(self, temp_storage, mock_docker_client):
        """Create a SnapshotManager instance for testing."""
        config = SnapshotConfig(
            storage_path=temp_storage, max_snapshots_per_container=3, max_total_snapshots=10
        )

        provider = DockerSnapshotProvider(docker_client=mock_docker_client)
        storage = FileSystemSnapshotStorage(base_path=temp_storage)

        return SnapshotManager(provider=provider, storage=storage, config=config)

    @pytest.mark.asyncio
    async def test_create_snapshot(self, snapshot_manager):
        """Test creating a snapshot."""
        metadata = await snapshot_manager.create_snapshot(
            container_id="test_container",
            trigger=SnapshotTrigger.MANUAL,
            description="Test snapshot",
        )

        assert metadata.snapshot_id is not None
        assert metadata.container_name == "test_container"
        assert metadata.trigger == SnapshotTrigger.MANUAL
        assert metadata.status == SnapshotStatus.COMPLETED
        assert metadata.description == "Test snapshot"

    @pytest.mark.asyncio
    async def test_list_snapshots(self, snapshot_manager):
        """Test listing snapshots."""
        # Create a few snapshots
        await snapshot_manager.create_snapshot("test_container", SnapshotTrigger.MANUAL)
        await snapshot_manager.create_snapshot("test_container", SnapshotTrigger.RUN_START)

        # List all snapshots
        snapshots = await snapshot_manager.list_snapshots()
        assert len(snapshots) == 2

        # List snapshots for specific container
        container_snapshots = await snapshot_manager.list_snapshots(
            container_id="test_container_id"
        )
        assert len(container_snapshots) == 2

    @pytest.mark.asyncio
    async def test_restore_snapshot(self, snapshot_manager):
        """Test restoring a snapshot."""
        # Create a snapshot
        metadata = await snapshot_manager.create_snapshot("test_container", SnapshotTrigger.MANUAL)

        # Restore it
        from snapshot_manager.models import RestoreOptions
        options = RestoreOptions(new_container_name="restored_container")
        
        restored_id = await snapshot_manager.restore_snapshot(metadata.snapshot_id, options)
        assert restored_id is not None

    @pytest.mark.asyncio
    async def test_snapshot_limits(self, snapshot_manager):
        """Test that snapshot limits are enforced."""
        # Create first snapshot to establish the container_id
        first_snapshot = await snapshot_manager.create_snapshot(
            "test_container", SnapshotTrigger.MANUAL, description="First snapshot"
        )
        
        # Now use the resolved container_id for subsequent snapshots
        container_id = first_snapshot.container_id
        
        # Create more snapshots using the actual container_id
        for i in range(3):  # Create 3 more (total will be 4, limit is 3)
            await snapshot_manager.create_snapshot(
                container_id, SnapshotTrigger.MANUAL, description=f"Snapshot {i+2}"
            )

        # Should only have 3 snapshots due to limit enforcement
        snapshots = await snapshot_manager.list_snapshots()
        assert len(snapshots) == 3

    @pytest.mark.asyncio
    async def test_delete_snapshot(self, snapshot_manager):
        """Test deleting individual snapshots."""
        # Create a snapshot
        metadata = await snapshot_manager.create_snapshot(
            "test_container", SnapshotTrigger.MANUAL, description="To be deleted"
        )
        
        # Verify it exists
        snapshots = await snapshot_manager.list_snapshots()
        assert len(snapshots) == 1
        
        # Delete it
        await snapshot_manager.delete_snapshot(metadata.snapshot_id)
        
        # Verify it's gone
        snapshots = await snapshot_manager.list_snapshots()
        assert len(snapshots) == 0

    @pytest.mark.asyncio
    async def test_cleanup_old_snapshots(self, snapshot_manager):
        """Test time-based cleanup of old snapshots."""
        from datetime import datetime, timedelta
        from unittest.mock import patch
        
        # Create snapshots with different ages by mocking timestamp
        snapshots_created = []
        
        # Create an "old" snapshot (10 days ago)
        with patch('snapshot_manager.models.datetime') as mock_datetime:
            old_time = datetime.now() - timedelta(days=10)
            mock_datetime.now.return_value = old_time
            
            old_snapshot = await snapshot_manager.create_snapshot(
                "test_container", SnapshotTrigger.MANUAL, description="Old snapshot"
            )
            # Manually update the timestamp in storage
            old_snapshot.timestamp = old_time
            await snapshot_manager.storage.update_metadata(old_snapshot)
            snapshots_created.append(old_snapshot)
        
        # Create a recent snapshot (1 day ago)
        recent_snapshot = await snapshot_manager.create_snapshot(
            "test_container", SnapshotTrigger.MANUAL, description="Recent snapshot"
        )
        snapshots_created.append(recent_snapshot)
        
        # Verify we have 2 snapshots
        all_snapshots = await snapshot_manager.list_snapshots()
        assert len(all_snapshots) == 2
        
        # Cleanup snapshots older than 5 days
        cleaned_count = await snapshot_manager.cleanup_old_snapshots(max_age_days=5)
        
        # Should have cleaned up 1 old snapshot
        assert cleaned_count == 1
        
        # Verify only recent snapshot remains
        remaining_snapshots = await snapshot_manager.list_snapshots()
        assert len(remaining_snapshots) == 1
        assert remaining_snapshots[0].snapshot_id == recent_snapshot.snapshot_id


class TestDockerSnapshotProvider:
    """Critical tests for DockerSnapshotProvider."""

    @pytest.fixture
    def mock_docker_client(self):
        """Mock Docker client."""
        mock_client = Mock(spec=docker.DockerClient)
        mock_client.ping.return_value = True
        return mock_client

    @pytest.fixture
    def docker_provider(self, mock_docker_client):
        """Create a DockerSnapshotProvider for testing."""
        return DockerSnapshotProvider(docker_client=mock_docker_client)

    @pytest.mark.asyncio
    async def test_validate_container_success(self, docker_provider, mock_docker_client):
        """Test successful container validation."""
        mock_container = Mock()
        mock_container.id = "test_id"
        mock_container.name = "test_name"
        mock_container.status = "running"
        mock_container.image.id = "test_image"
        mock_container.labels = {}
        mock_container.attrs = {"Created": "2024-01-01", "Config": {}, "Mounts": []}

        mock_docker_client.containers.get.return_value = mock_container
        is_valid = await docker_provider.validate_container("test_container")
        assert is_valid is True

    @pytest.mark.asyncio
    async def test_validate_container_not_found(self, docker_provider, mock_docker_client):
        """Test container validation when container doesn't exist."""
        from docker.errors import NotFound
        mock_docker_client.containers.get.side_effect = NotFound("Container not found")
        is_valid = await docker_provider.validate_container("nonexistent_container")
        assert is_valid is False


class TestSnapshotCallback:
    """Critical tests for SnapshotCallback."""

    @pytest.fixture
    def mock_snapshot_manager(self):
        """Mock SnapshotManager for testing."""
        manager = Mock(spec=SnapshotManager)
        manager.should_create_snapshot = AsyncMock(return_value=True)
        manager.create_snapshot = AsyncMock()
        return manager

    @pytest.fixture
    def snapshot_callback(self, mock_snapshot_manager):
        """Create a SnapshotCallback for testing."""
        from snapshot_manager.callback import SnapshotCallback
        return SnapshotCallback(snapshot_manager=mock_snapshot_manager)

    @pytest.mark.asyncio
    async def test_on_run_start(self, snapshot_callback, mock_snapshot_manager):
        """Test run start callback creates snapshot."""
        kwargs = {"container_id": "test_container"}
        await snapshot_callback.on_run_start(kwargs, [])
        
        mock_snapshot_manager.create_snapshot.assert_called_once()
        call_args = mock_snapshot_manager.create_snapshot.call_args
        assert call_args[1]["trigger"] == SnapshotTrigger.RUN_START


if __name__ == "__main__":
    pytest.main([__file__])
