"""
Docker implementation of the snapshot provider.

This provider uses Docker's native commit and save/load functionality
to create efficient container snapshots.
"""

import logging
from datetime import datetime
from typing import Any, Dict, Optional

import docker
from docker.errors import DockerException, NotFound

from ..interfaces import ContainerNotFoundError, SnapshotError, SnapshotProvider
from ..models import RestoreOptions, SnapshotMetadata, SnapshotStatus

logger = logging.getLogger(__name__)


class DockerSnapshotProvider(SnapshotProvider):
    """
    Docker-based snapshot provider.

    Uses Docker's commit functionality to create container snapshots
    as new images, providing efficient storage and fast restoration.
    """

    def __init__(self, docker_client: Optional[docker.DockerClient] = None):
        """
        Initialize the Docker snapshot provider.

        Args:
            docker_client: Optional Docker client instance. If None, creates a new one.
        """
        self.client = docker_client or docker.from_env()
        self._validate_docker_connection()

    def _validate_docker_connection(self) -> None:
        """Validate that Docker is available and accessible."""
        try:
            self.client.ping()
            logger.info("Docker connection validated successfully")
        except DockerException as e:
            raise SnapshotError(f"Failed to connect to Docker: {e}")

    def _generate_image_tag(self, metadata: SnapshotMetadata) -> str:
        """
        Generate a Docker image tag for the snapshot.

        Args:
            metadata: Snapshot metadata

        Returns:
            Docker image tag string
        """
        timestamp = metadata.timestamp.strftime("%Y%m%d_%H%M%S")
        # Docker tags must be lowercase and contain only valid characters
        safe_name = metadata.container_name.lower().replace("_", "-")
        return f"cua-snapshot/{safe_name}:{metadata.trigger.value}-{timestamp}"

    def _get_container_info(self, container_id: str) -> Dict[str, Any]:
        """
        Get detailed information about a container.

        Args:
            container_id: ID or name of the container

        Returns:
            Dictionary with container information

        Raises:
            ContainerNotFoundError: If container doesn't exist
        """
        try:
            container = self.client.containers.get(container_id)
            return {
                "id": container.id,
                "name": container.name,
                "status": container.status,
                "image": container.image.id,
                "labels": container.labels,
                "created": container.attrs["Created"],
                "config": container.attrs["Config"],
                "mounts": container.attrs["Mounts"] if "Mounts" in container.attrs else [],
                "network_settings": (
                    container.attrs["NetworkSettings"]
                    if "NetworkSettings" in container.attrs
                    else {}
                ),
            }
        except NotFound:
            raise ContainerNotFoundError(f"Container '{container_id}' not found")
        except DockerException as e:
            raise SnapshotError(f"Error getting container info: {e}")

    async def validate_container(self, container_id: str) -> bool:
        """
        Validate that a container exists and can be snapshotted.

        Args:
            container_id: ID or name of the container

        Returns:
            True if container is valid for snapshotting
        """
        try:
            container_info = self._get_container_info(container_id)

            # Check if container is in a state that can be snapshotted
            valid_statuses = ["running", "paused", "exited", "created"]
            if container_info["status"] not in valid_statuses:
                logger.warning(
                    f"Container {container_id} has status '{container_info['status']}', may not be snapshotable"
                )
                return False

            return True
        except ContainerNotFoundError:
            return False
        except Exception as e:
            logger.error(f"Error validating container {container_id}: {e}")
            return False

    async def create_snapshot(
        self, container_id: str, metadata: SnapshotMetadata
    ) -> SnapshotMetadata:
        """
        Create a snapshot of the specified container using Docker commit.

        Args:
            container_id: ID or name of the container to snapshot
            metadata: Metadata for the snapshot

        Returns:
            Updated metadata with storage information

        Raises:
            SnapshotError: If snapshot creation fails
        """
        logger.info(f"Creating snapshot for container {container_id}")

        try:
            # Validate container exists and get info
            container_info = self._get_container_info(container_id)

            # Update metadata with container information
            metadata.container_id = container_info["id"]
            metadata.container_name = container_info["name"]
            metadata.status = SnapshotStatus.CREATING

            # Generate image tag
            image_tag = self._generate_image_tag(metadata)
            metadata.image_tag = image_tag

            # Get the container object
            container = self.client.containers.get(container_id)

            # Create commit message
            commit_message = f"CUA Snapshot: {metadata.trigger.value}"
            if metadata.description:
                commit_message += f" - {metadata.description}"

            # Create the snapshot using Docker commit
            # This creates a new image from the container's current state
            logger.info(f"Committing container {container_id} to image {image_tag}")

            # Run the commit operation
            image = container.commit(
                repository=image_tag.split(":")[0],
                tag=image_tag.split(":")[1],
                message=commit_message,
                changes=None,  # Could be used for additional Dockerfile-like changes
            )

            # Update metadata with image information
            metadata.image_id = image.id
            metadata.status = SnapshotStatus.COMPLETED

            # Calculate size
            try:
                # Get image size from Docker
                size_bytes = await self.get_snapshot_size(metadata)
                metadata.size_bytes = size_bytes
            except Exception as e:
                logger.warning(f"Could not determine snapshot size: {e}")

            # Add Docker-specific metadata
            metadata.labels.update(
                {
                    "docker.image.id": image.id,
                    "docker.container.id": container_info["id"],
                    "docker.container.name": container_info["name"],
                    "docker.created": datetime.now().isoformat(),
                    "cua.snapshot.version": "1.0",
                }
            )

            # Store original container configuration for restoration
            metadata.agent_metadata.update(
                {
                    "original_config": container_info["config"],
                    "original_mounts": container_info["mounts"],
                    "original_network_settings": container_info["network_settings"],
                    "original_image": container_info["image"],
                }
            )

            logger.info(
                f"Successfully created snapshot {metadata.snapshot_id} for container {container_id}"
            )
            return metadata

        except ContainerNotFoundError:
            metadata.status = SnapshotStatus.FAILED
            raise
        except DockerException as e:
            metadata.status = SnapshotStatus.FAILED
            raise SnapshotError(f"Docker error creating snapshot: {e}")
        except Exception as e:
            metadata.status = SnapshotStatus.FAILED
            raise SnapshotError(f"Unexpected error creating snapshot: {e}")

    async def restore_snapshot(
        self, metadata: SnapshotMetadata, options: Optional[RestoreOptions] = None
    ) -> str:
        """
        Restore a container from a snapshot.

        Args:
            metadata: Metadata of the snapshot to restore
            options: Optional restore configuration

        Returns:
            ID of the restored container

        Raises:
            SnapshotError: If restore fails
        """
        logger.info(f"Restoring snapshot {metadata.snapshot_id}")

        if not metadata.image_id:
            raise SnapshotError("Snapshot has no image ID - cannot restore")

        try:
            # Set default options
            if options is None:
                options = RestoreOptions()

            # Get the snapshot image
            try:
                image = self.client.images.get(metadata.image_id)
            except NotFound:
                raise SnapshotError(f"Snapshot image {metadata.image_id} not found")

            # Prepare container configuration
            container_name = (
                options.new_container_name
                or f"{metadata.container_name}_restored_{int(datetime.now().timestamp())}"
            )

            # Get original configuration from metadata
            original_config = metadata.agent_metadata.get("original_config", {})
            original_mounts = metadata.agent_metadata.get("original_mounts", [])

            # Build run configuration
            run_config = {
                "name": container_name,
                "detach": True,
            }

            # Apply environment overrides
            env_vars = original_config.get("Env", [])
            if options.environment_overrides:
                # Convert list of "KEY=VALUE" to dict, apply overrides, convert back
                env_dict = {}
                for env_var in env_vars:
                    if "=" in env_var:
                        key, value = env_var.split("=", 1)
                        env_dict[key] = value

                env_dict.update(options.environment_overrides)
                run_config["environment"] = env_dict
            else:
                run_config["environment"] = env_vars

            # Apply port mappings
            if options.port_mappings:
                run_config["ports"] = options.port_mappings
            elif "ExposedPorts" in original_config:
                # Try to preserve original port mappings if possible
                run_config["ports"] = {
                    port: None for port in original_config["ExposedPorts"].keys()
                }

            # Handle volumes and mounts if preserving them
            if options.preserve_volumes and original_mounts:
                mounts = []
                volumes = {}

                for mount in original_mounts:
                    if mount.get("Type") == "bind":
                        # Bind mounts
                        mounts.append(f"{mount['Source']}:{mount['Destination']}")
                    elif mount.get("Type") == "volume":
                        # Named volumes
                        volumes[mount["Destination"]] = {}

                if mounts:
                    run_config["volumes"] = mounts
                if volumes:
                    run_config["volumes"] = volumes

            # Apply working directory
            if "WorkingDir" in original_config:
                run_config["working_dir"] = original_config["WorkingDir"]

            # Apply user
            if "User" in original_config:
                run_config["user"] = original_config["User"]

            # Apply command and entrypoint
            if "Cmd" in original_config and original_config["Cmd"]:
                run_config["command"] = original_config["Cmd"]

            if "Entrypoint" in original_config and original_config["Entrypoint"]:
                run_config["entrypoint"] = original_config["Entrypoint"]

            # Create and start the restored container
            logger.info(
                f"Creating restored container '{container_name}' from image {metadata.image_id}"
            )

            container = self.client.containers.run(image.id, **run_config)

            # Update restoration tracking
            metadata.restoration_count += 1

            logger.info(
                f"Successfully restored container {container.id} from snapshot {metadata.snapshot_id}"
            )
            return container.id

        except DockerException as e:
            raise SnapshotError(f"Docker error restoring snapshot: {e}")
        except Exception as e:
            raise SnapshotError(f"Unexpected error restoring snapshot: {e}")

    async def delete_snapshot(self, metadata: SnapshotMetadata) -> None:
        """
        Delete a snapshot and clean up its storage.

        Args:
            metadata: Metadata of the snapshot to delete

        Raises:
            SnapshotError: If deletion fails
        """
        logger.info(f"Deleting snapshot {metadata.snapshot_id}")

        try:
            if metadata.image_id:
                try:
                    # Remove the Docker image
                    self.client.images.remove(metadata.image_id, force=True)
                    logger.info(f"Removed Docker image {metadata.image_id}")
                except NotFound:
                    logger.warning(f"Image {metadata.image_id} not found during deletion")
                except DockerException as e:
                    logger.error(f"Error removing Docker image: {e}")
                    # Don't raise here - we still want to clean up metadata

            # Update status
            metadata.status = SnapshotStatus.DELETED

        except Exception as e:
            raise SnapshotError(f"Error deleting snapshot: {e}")

    async def get_snapshot_size(self, metadata: SnapshotMetadata) -> int:
        """
        Get the size of a snapshot in bytes.

        Args:
            metadata: Metadata of the snapshot

        Returns:
            Size in bytes
        """
        if not metadata.image_id:
            return 0

        try:
            image = self.client.images.get(metadata.image_id)
            # Docker image size is in the attrs
            return image.attrs.get("Size", 0)
        except NotFound:
            return 0
        except DockerException as e:
            logger.error(f"Error getting snapshot size: {e}")
            return 0
