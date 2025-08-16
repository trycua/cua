"""
Docker implementation of the snapshot provider.

This provider uses Docker's native commit and save/load functionality
to create efficient container snapshots.
"""

import logging
from datetime import datetime
from io import BytesIO
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

    def _analyze_container_volumes(self, container_id: str) -> Dict[str, Any]:
        """
        Analyze volumes attached to a container.

        Args:
            container_id: ID of the container to analyze

        Returns:
            Dictionary with volume analysis results
        """
        try:
            container = self.client.containers.get(container_id)
            mounts = container.attrs.get("Mounts", [])

            logger.debug(f"Container mounts structure: {mounts}")

            named_volumes = []
            bind_mounts = []

            for mount in mounts:
                # Handle different mount structure formats
                if isinstance(mount, dict):
                    mount_info = {
                        "type": mount.get("Type"),
                        "source": mount.get("Source"),
                        "destination": mount.get("Destination"),
                        "mode": mount.get("Mode", "rw"),
                        "rw": mount.get("RW", True),
                        "name": mount.get("Name"),
                    }

                    if mount.get("Type") == "volume":
                        named_volumes.append(mount_info)
                    elif mount.get("Type") == "bind":
                        bind_mounts.append(mount_info)
                else:
                    logger.warning(f"Unexpected mount structure: {type(mount)} - {mount}")

            return {
                "named_volumes": named_volumes,
                "bind_mounts": bind_mounts,
                "total_volumes": len(named_volumes),
                "total_bind_mounts": len(bind_mounts),
            }

        except NotFound:
            raise ContainerNotFoundError(f"Container {container_id} not found")
        except DockerException as e:
            raise SnapshotError(f"Failed to analyze container volumes: {e}")

    def _backup_named_volume_as_tar(self, volume_name: str) -> bytes:
        """
        Create a tar archive of a named volume's data.

        Args:
            volume_name: Name of the Docker volume to backup

        Returns:
            Tar archive as bytes

        Raises:
            SnapshotError: If volume backup fails
        """
        try:
            logger.info(f"Creating tar backup of volume: {volume_name}")

            # Verify volume exists
            try:
                volume = self.client.volumes.get(volume_name)
                logger.debug(f"Volume {volume_name} found: {volume.attrs}")
            except NotFound:
                raise SnapshotError(f"Volume {volume_name} not found")

            # Create temporary container to access volume data
            # Using alpine for minimal footprint
            temp_container_config = {
                "image": "alpine:latest",
                "command": "sleep 30",
                "volumes": {volume_name: {"bind": "/volume_data", "mode": "ro"}},
                "detach": True,
                "remove": False,
                "name": f"volume-backup-{volume_name}-temp",
            }

            temp_container = None
            try:
                # Pull alpine if not available (but don't wait too long)
                try:
                    self.client.images.get("alpine:latest")
                except NotFound:
                    logger.info("Pulling alpine:latest for volume backup...")
                    self.client.images.pull("alpine:latest")

                # Create and start temporary container
                temp_container = self.client.containers.run(**temp_container_config)
                logger.debug(f"Created temporary container: {temp_container.name}")

                # Create tar archive of volume contents
                tar_stream, _ = temp_container.get_archive("/volume_data")

                # Convert stream to bytes
                tar_data = BytesIO()
                for chunk in tar_stream:
                    tar_data.write(chunk)

                tar_bytes = tar_data.getvalue()
                logger.info(f"Volume {volume_name} backed up: {len(tar_bytes)} bytes")

                return tar_bytes

            finally:
                # Clean up temporary container
                if temp_container:
                    try:
                        temp_container.stop(timeout=5)
                        temp_container.remove()
                        logger.debug(f"Cleaned up temporary container: {temp_container.name}")
                    except Exception as e:
                        logger.warning(f"Failed to clean up temporary container: {e}")

        except DockerException as e:
            raise SnapshotError(f"Failed to backup volume {volume_name}: {e}")
        except Exception as e:
            raise SnapshotError(f"Unexpected error backing up volume {volume_name}: {e}")

    def _backup_container_volumes(self, volume_analysis: Dict[str, Any]) -> Dict[str, bytes]:
        """
        Backup all named volumes from a container's volume analysis.

        Args:
            volume_analysis: Result from _analyze_container_volumes

        Returns:
            Dictionary mapping volume names to tar archive bytes

        Raises:
            SnapshotError: If any volume backup fails
        """
        volume_backups = {}

        # Process named volumes
        for volume_info in volume_analysis.get("named_volumes", []):
            volume_name = volume_info.get("name")
            if not volume_name:
                logger.warning(f"Skipping volume without name: {volume_info}")
                continue

            try:
                tar_data = self._backup_named_volume_as_tar(volume_name)
                volume_backups[volume_name] = tar_data
                logger.info(f"Successfully backed up volume: {volume_name}")
            except Exception as e:
                logger.error(f"Failed to backup volume {volume_name}: {e}")
                # Continue with other volumes, but track the failure
                # We could make this configurable (fail-fast vs best-effort)
                raise SnapshotError(f"Volume backup failed for {volume_name}: {e}")

        # Log bind mount warnings
        bind_mounts = volume_analysis.get("bind_mounts", [])
        if bind_mounts:
            logger.warning(
                f"⚠️  {len(bind_mounts)} bind mount(s) detected - NOT included in snapshot:"
            )
            for mount in bind_mounts:
                logger.warning(f"   Bind mount: {mount.get('source')} → {mount.get('destination')}")
                logger.warning("   Recommendation: Use named volumes for portable snapshots")

        logger.info(
            f"Volume backup completed: {len(volume_backups)} volumes, {len(bind_mounts)} bind mounts skipped"
        )
        return volume_backups

    def _restore_volume_from_tar(self, volume_name: str, tar_data: bytes) -> None:
        """
        Restore a volume from tar archive data.

        Args:
            volume_name: Name of the volume to create/restore
            tar_data: Tar archive data to extract

        Raises:
            SnapshotError: If volume restore fails
        """
        try:
            logger.info(f"Restoring volume from tar: {volume_name} ({len(tar_data)} bytes)")

            # Create the volume if it doesn't exist
            try:
                self.client.volumes.get(volume_name)
                logger.debug(f"Volume {volume_name} already exists, will overwrite data")
            except NotFound:
                logger.info(f"Creating new volume: {volume_name}")
                self.client.volumes.create(volume_name)

            # Create temporary container to restore volume data
            temp_container_config = {
                "image": "alpine:latest",
                "command": "sleep 30",
                "volumes": {volume_name: {"bind": "/volume_data", "mode": "rw"}},
                "detach": True,
                "remove": False,
                "name": f"volume-restore-{volume_name}-temp",
            }

            temp_container = None
            try:
                # Ensure alpine image is available
                try:
                    self.client.images.get("alpine:latest")
                except NotFound:
                    logger.info("Pulling alpine:latest for volume restore...")
                    self.client.images.pull("alpine:latest")

                # Create and start temporary container
                temp_container = self.client.containers.run(**temp_container_config)
                logger.debug(f"Created temporary restore container: {temp_container.name}")

                # Clear existing volume contents first
                logger.debug("Clearing existing volume contents...")
                temp_container.exec_run("sh -c 'rm -rf /volume_data/* /volume_data/.*' || true")

                # Extract tar archive into volume
                logger.debug("Extracting tar data into volume...")
                temp_container.put_archive("/", tar_data)

                # Verify extraction worked
                result = temp_container.exec_run("ls -la /volume_data/")
                if result.exit_code == 0:
                    logger.debug(f"Volume contents after restore:\n{result.output.decode()}")
                else:
                    logger.warning(f"Could not verify volume contents: {result.output.decode()}")

                logger.info(f"Volume {volume_name} restored successfully")

            finally:
                # Clean up temporary container
                if temp_container:
                    try:
                        temp_container.stop(timeout=5)
                        temp_container.remove()
                        logger.debug(
                            f"Cleaned up temporary restore container: {temp_container.name}"
                        )
                    except Exception as e:
                        logger.warning(f"Failed to clean up temporary restore container: {e}")

        except DockerException as e:
            raise SnapshotError(f"Failed to restore volume {volume_name}: {e}")
        except Exception as e:
            raise SnapshotError(f"Unexpected error restoring volume {volume_name}: {e}")

    async def _restore_container_volumes(
        self, snapshot_metadata: SnapshotMetadata, storage: Any
    ) -> Dict[str, str]:
        """
        Restore all volumes for a container from storage.

        Args:
            snapshot_metadata: Metadata of the snapshot being restored
            storage: Storage instance to load volume data from

        Returns:
            Dictionary mapping original volume names to restored volume names

        Raises:
            SnapshotError: If volume restore fails
        """
        volume_mappings: Dict[str, str] = {}

        # Get volume information from original snapshot
        volume_analysis = snapshot_metadata.agent_metadata.get("volume_analysis", {})
        original_volumes = volume_analysis.get("named_volumes", [])

        if not original_volumes:
            logger.info("No named volumes to restore")
            return volume_mappings

        logger.info(
            f"Restoring {len(original_volumes)} volumes from snapshot {snapshot_metadata.snapshot_id}"
        )

        # Get list of available volume files from storage
        available_volumes = await storage.list_volume_files(snapshot_metadata.snapshot_id)

        for volume_info in original_volumes:
            original_volume_name = volume_info.get("name")
            if not original_volume_name:
                logger.warning(f"Skipping volume without name: {volume_info}")
                continue

            if original_volume_name not in available_volumes:
                logger.warning(f"Volume data not found in storage: {original_volume_name}")
                continue

            try:
                # Load volume tar data from storage
                tar_data = await storage.load_volume_data(
                    snapshot_metadata.snapshot_id, original_volume_name
                )

                # Generate new volume name for restoration
                # For now, we'll restore with the same name (could be made configurable)
                restored_volume_name = original_volume_name

                # Restore the volume
                self._restore_volume_from_tar(restored_volume_name, tar_data)

                volume_mappings[original_volume_name] = restored_volume_name
                logger.info(
                    f"Successfully restored volume: {original_volume_name} → {restored_volume_name}"
                )

            except Exception as e:
                logger.error(f"Failed to restore volume {original_volume_name}: {e}")
                # Continue with other volumes, but track the failure
                raise SnapshotError(f"Volume restore failed for {original_volume_name}: {e}")

        logger.info(f"Volume restore completed: {len(volume_mappings)} volumes restored")
        return volume_mappings

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

            # Analyze volumes attached to the container
            volume_analysis = self._analyze_container_volumes(container_id)
            logger.info(
                f"Volume analysis: {volume_analysis['total_volumes']} named volumes, {volume_analysis['total_bind_mounts']} bind mounts"
            )

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

            # Backup volumes if any are attached
            volume_backups = {}
            if volume_analysis['total_volumes'] > 0:
                logger.info(f"Backing up {volume_analysis['total_volumes']} named volumes...")
                try:
                    volume_backups = self._backup_container_volumes(volume_analysis)
                    logger.info(f"Successfully backed up {len(volume_backups)} volumes")
                except Exception as e:
                    logger.error(f"Volume backup failed: {e}")
                    # Continue with snapshot but log the failure
                    # We could make this configurable (fail vs continue)

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
                    "volume_analysis": volume_analysis,
                    "volume_backups": list(volume_backups.keys()) if volume_backups else [],
                }
            )

            logger.info(
                f"Successfully created snapshot {metadata.snapshot_id} for container {container_id}"
            )
            
            # Attach volume backup data to metadata for manager to store
            # This is a temporary attribute, not persisted in metadata JSON
            setattr(metadata, '_volume_backups', volume_backups)
            
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
                volume_binds = []

                for mount in original_mounts:
                    if mount.get("Type") == "bind":
                        # Bind mounts - format: "host_path:container_path:mode"
                        mode = mount.get("Mode", "rw")
                        volume_binds.append(f"{mount['Source']}:{mount['Destination']}:{mode}")
                    elif mount.get("Type") == "volume":
                        # Named volumes - format: "volume_name:container_path:mode"  
                        mode = mount.get("Mode", "rw")
                        volume_name = mount.get("Name", "")
                        if volume_name:
                            volume_binds.append(f"{volume_name}:{mount['Destination']}:{mode}")

                if volume_binds:
                    run_config["volumes"] = volume_binds

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
            return str(container.id)

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
            return int(image.attrs.get("Size", 0))
        except NotFound:
            return 0
        except DockerException as e:
            logger.error(f"Error getting snapshot size: {e}")
            return 0
