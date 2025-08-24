"""
Test volume support functionality in the snapshot manager.

Tests volume detection, backup, storage, restore, and bind mount warnings.
"""

import tempfile
from unittest.mock import Mock, patch
import pytest

import docker
from snapshot_manager.providers.docker_provider import DockerSnapshotProvider
from snapshot_manager.storage import FileSystemSnapshotStorage
from snapshot_manager.models import SnapshotMetadata, SnapshotTrigger


class TestVolumeSupport:
    """Test volume support functionality."""

    @pytest.fixture
    def temp_storage(self):
        """Create temporary storage for testing."""
        with tempfile.TemporaryDirectory() as temp_dir:
            yield temp_dir

    @pytest.fixture
    def mock_docker_client(self):
        """Mock Docker client for volume testing."""
        mock_client = Mock(spec=docker.DockerClient)
        mock_client.ping.return_value = True

        # Mock container with volumes
        mock_container = Mock()
        mock_container.id = "test_container_id"
        mock_container.name = "test_container"
        mock_container.attrs = {
            "Mounts": [
                {
                    "Type": "volume",
                    "Name": "test_named_volume",
                    "Source": "/var/lib/docker/volumes/test_named_volume/_data",
                    "Destination": "/app/data",
                    "Mode": "rw",
                    "RW": True,
                    "Propagation": "",
                },
                {
                    "Type": "bind",
                    "Source": "/host/path",
                    "Destination": "/app/bind",
                    "Mode": "rw",
                    "RW": True,
                    "Propagation": "rprivate",
                },
            ]
        }

        # Mock volume operations
        mock_volume = Mock()
        mock_volume.name = "test_named_volume"
        mock_volume.attrs = {"Driver": "local"}

        # Mock temporary container for volume operations
        mock_temp_container = Mock()
        mock_temp_container.name = "volume-backup-test_named_volume-temp"

        # Mock tar stream data
        mock_temp_container.get_archive.return_value = (iter([b"fake_tar_data"]), None)
        mock_temp_container.exec_run.return_value = Mock(exit_code=0, output=b"fake_output")
        mock_temp_container.stop = Mock()
        mock_temp_container.remove = Mock()
        mock_temp_container.put_archive = Mock()

        mock_client.containers.get.return_value = mock_container
        mock_client.containers.run.return_value = mock_temp_container
        mock_client.volumes.get.return_value = mock_volume
        mock_client.volumes.create.return_value = mock_volume
        mock_client.images.get.return_value = Mock()  # Alpine image exists

        return mock_client

    @pytest.fixture
    def docker_provider(self, mock_docker_client):
        """Create DockerSnapshotProvider with mocked client."""
        return DockerSnapshotProvider(docker_client=mock_docker_client)

    @pytest.fixture
    def storage(self, temp_storage):
        """Create FileSystemSnapshotStorage."""
        return FileSystemSnapshotStorage(base_path=temp_storage)

    def test_volume_detection(self, docker_provider):
        """Test volume detection functionality."""
        # Test volume analysis
        analysis = docker_provider._analyze_container_volumes("test_container")

        # Should detect one named volume and one bind mount
        assert analysis["total_volumes"] == 1
        assert analysis["total_bind_mounts"] == 1

        # Check named volume details
        named_volumes = analysis["named_volumes"]
        assert len(named_volumes) == 1
        assert named_volumes[0]["name"] == "test_named_volume"
        assert named_volumes[0]["destination"] == "/app/data"
        assert named_volumes[0]["type"] == "volume"

        # Check bind mount details
        bind_mounts = analysis["bind_mounts"]
        assert len(bind_mounts) == 1
        assert bind_mounts[0]["source"] == "/host/path"
        assert bind_mounts[0]["destination"] == "/app/bind"
        assert bind_mounts[0]["type"] == "bind"

    def test_volume_backup(self, docker_provider):
        """Test volume backup functionality."""
        # Test single volume backup
        tar_data = docker_provider._backup_named_volume_as_tar("test_named_volume")
        assert tar_data == b"fake_tar_data"

        # Test full container volume backup
        analysis = docker_provider._analyze_container_volumes("test_container")

        # Mock the backup method to return expected data for the full test
        with patch.object(
            docker_provider, "_backup_named_volume_as_tar", return_value=b"fake_tar_data"
        ):
            volume_backups = docker_provider._backup_container_volumes(analysis)

        # Should only backup named volumes, not bind mounts
        assert len(volume_backups) == 1
        assert "test_named_volume" in volume_backups
        assert volume_backups["test_named_volume"] == b"fake_tar_data"

    @pytest.mark.asyncio
    async def test_volume_storage(self, storage):
        """Test volume storage operations."""
        snapshot_id = "test_snapshot_123"
        volume_name = "test_volume"
        test_data = b"test volume data"

        # Test save
        await storage.save_volume_data(snapshot_id, volume_name, test_data)

        # Test load
        loaded_data = await storage.load_volume_data(snapshot_id, volume_name)
        assert loaded_data == test_data

        # Test list
        volume_files = await storage.list_volume_files(snapshot_id)
        assert volume_files == [volume_name]

        # Test delete specific
        await storage.delete_volume_data(snapshot_id, volume_name)
        volume_files = await storage.list_volume_files(snapshot_id)
        assert volume_files == []

    def test_volume_restore(self, docker_provider):
        """Test volume restore functionality."""
        test_data = b"restore_test_data"

        # Mock volume not found initially to test creation
        from docker.errors import NotFound

        docker_provider.client.volumes.get.side_effect = [NotFound("Volume not found"), Mock()]

        # Test volume restore
        docker_provider._restore_volume_from_tar("restored_volume", test_data)

        # Verify volume creation and container operations were called
        docker_provider.client.volumes.create.assert_called_with("restored_volume")
        docker_provider.client.containers.run.assert_called()

    @pytest.mark.asyncio
    async def test_full_container_volume_restore(self, docker_provider, storage):
        """Test complete container volume restore workflow."""
        # Setup test data
        snapshot_id = "test_snapshot_456"
        volume_name = "app_data"
        test_tar_data = b"complete_restore_test"

        # Save volume data to storage
        await storage.save_volume_data(snapshot_id, volume_name, test_tar_data)

        # Create snapshot metadata with volume information
        snapshot_metadata = SnapshotMetadata(
            snapshot_id=snapshot_id,
            container_id="test_container",
            container_name="test_container",
            trigger=SnapshotTrigger.MANUAL,
            agent_metadata={
                "volume_analysis": {
                    "named_volumes": [
                        {"name": volume_name, "destination": "/app/data", "type": "volume"}
                    ],
                    "bind_mounts": [],
                    "total_volumes": 1,
                    "total_bind_mounts": 0,
                }
            },
        )

        # Test volume restore
        volume_mappings = await docker_provider._restore_container_volumes(
            snapshot_metadata, storage
        )

        # Verify restore mapping
        assert volume_name in volume_mappings
        assert volume_mappings[volume_name] == volume_name

    def test_bind_mount_warnings(self, docker_provider, caplog):
        """Test that bind mount warnings are properly logged."""
        import logging

        # Set up logging to capture warnings
        caplog.set_level(logging.WARNING)

        # Analyze container with bind mounts
        analysis = docker_provider._analyze_container_volumes("test_container")

        # Trigger volume backup which should generate warnings
        docker_provider._backup_container_volumes(analysis)

        # Check that bind mount warnings were logged
        warning_messages = [
            record.message for record in caplog.records if record.levelno >= logging.WARNING
        ]

        # Should have warnings about bind mounts
        bind_mount_warnings = [msg for msg in warning_messages if "bind mount" in msg.lower()]
        assert len(bind_mount_warnings) > 0

        # Should mention the specific bind mount path
        path_mentioned = any("/host/path" in msg for msg in bind_mount_warnings)
        assert path_mentioned

    def test_volume_error_handling(self, docker_provider):
        """Test error handling in volume operations."""
        from docker.errors import NotFound
        from snapshot_manager.interfaces import SnapshotError

        # Test backup of non-existent volume
        docker_provider.client.volumes.get.side_effect = NotFound("Volume not found")

        with pytest.raises(SnapshotError, match="Volume .* not found"):
            docker_provider._backup_named_volume_as_tar("nonexistent_volume")

    @pytest.mark.asyncio
    async def test_storage_error_handling(self, storage):
        """Test storage error handling."""
        from snapshot_manager.interfaces import StorageError

        # Test loading non-existent volume data
        with pytest.raises(StorageError, match="Volume data not found"):
            await storage.load_volume_data("nonexistent_snapshot", "nonexistent_volume")

    def test_volume_analysis_empty_container(self, mock_docker_client):
        """Test volume analysis for container with no volumes."""
        # Mock container with no mounts
        mock_container = Mock()
        mock_container.attrs = {"Mounts": []}
        mock_docker_client.containers.get.return_value = mock_container

        provider = DockerSnapshotProvider(docker_client=mock_docker_client)
        analysis = provider._analyze_container_volumes("empty_container")

        assert analysis["total_volumes"] == 0
        assert analysis["total_bind_mounts"] == 0
        assert analysis["named_volumes"] == []
        assert analysis["bind_mounts"] == []

    @pytest.mark.asyncio
    async def test_storage_directory_structure(self, temp_storage):
        """Test that storage creates proper directory structure."""
        storage = FileSystemSnapshotStorage(base_path=temp_storage)

        # Save some volume data
        await storage.save_volume_data("test_snap", "test_vol", b"test_data")

        # Check directory structure
        from pathlib import Path

        base_path = Path(temp_storage)

        assert (base_path / "metadata").exists()
        assert (base_path / "volumes").exists()
        assert (base_path / "volumes" / "test_snap_test_vol.tar").exists()


if __name__ == "__main__":
    pytest.main([__file__])
