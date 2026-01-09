#!/usr/bin/env python3
"""
Azure Batch orchestration script for Windows Arena benchmark.

Usage:
    python run_azure_batch.py --experiments_json experiments.json
    python run_azure_batch.py --exp_name gpt4o --num_workers 1 --model_name gpt-4o
"""

import argparse
import json
import logging
import os
import time
import traceback
from datetime import datetime, timedelta
from pathlib import Path
from typing import Any, Dict

import azure.batch.models as batchmodels

# Azure Batch SDK
from azure.batch import BatchServiceClient
from azure.batch.batch_auth import SharedKeyCredentials

# Azure Storage SDK
from azure.storage.blob import (
    BlobServiceClient,
    ContainerSasPermissions,
    generate_container_sas,
)

# ============================================================================
# Configuration Loading
# ============================================================================


def load_config() -> Dict[str, Any]:
    """Load Azure configuration from .env.local file"""
    script_dir = Path(__file__).parent
    env_path = script_dir / ".." / ".env.local"

    config = {}
    with env_path.resolve().open("r") as f:
        for line in f:
            line = line.strip()
            # Skip empty lines and comments
            if not line or line.startswith("#"):
                continue
            # Parse KEY=value
            if "=" in line:
                key, value = line.split("=", 1)
                config[key.strip()] = value.strip()

    return config


def load_args_as_dict() -> Dict[str, Any]:
    """Parse command line arguments"""
    parser = argparse.ArgumentParser(description="Run Windows Arena benchmark on Azure Batch")

    # Experiment parameters (same as run_azure.py)
    parser.add_argument("--agent", default="navi", help="Agent to use (default: navi)")
    parser.add_argument(
        "--docker_img_name",
        default="winarenamlacr.azurecr.io/winarena:latest",
        help="Docker image name (defaults to ACR)",
    )
    parser.add_argument("--exp_name", default="exp0", help="Experiment name")
    parser.add_argument("--num_workers", type=int, default=1, help="Number of parallel workers")
    parser.add_argument(
        "--json_name", default="evaluation_examples_windows/test_all.json", help="Test JSON file"
    )
    parser.add_argument("--model_name", default="gpt-4o-mini", help="Model name")
    parser.add_argument("--som_origin", default="oss", help="SOM origin (oss, a11y, mixed)")
    parser.add_argument("--a11y_backend", default="uia", help="Accessibility backend (uia, win32)")

    # Azure Batch specific parameters
    parser.add_argument(
        "--pool_vm_size",
        default="Standard_D4s_v3",
        help="VM size for pool (default: Standard_D4s_v3)",
    )
    parser.add_argument(
        "--pool_node_count", type=int, default=None, help="Fixed pool size (default: num_workers)"
    )
    parser.add_argument(
        "--auto_cleanup",
        type=lambda x: x.lower() == "true",
        default=True,
        help="Delete pool after job completion",
    )

    # Batch experiment management
    parser.add_argument("--experiments_json", help="Path to JSON file with multiple experiments")
    parser.add_argument("--update_json", action="store_true", help="Update experiments JSON")

    args, _ = parser.parse_known_args()
    return vars(args)


def load_experiments(json_path: str) -> Dict[str, Any]:
    """Load experiments from JSON file"""
    with open(json_path, "r") as f:
        return json.load(f)


def save_experiments(experiments: Dict[str, Any], json_path: str):
    """Save experiments to JSON file"""
    with open(json_path, "w") as f:
        json.dump(experiments, f, indent=2)


# ============================================================================
# Azure Client Setup
# ============================================================================


def create_batch_client(azure_config: Dict[str, Any]) -> BatchServiceClient:
    """Create Azure Batch client with credentials"""
    credentials = SharedKeyCredentials(
        account_name=azure_config["AZURE_BATCH_ACCOUNT_NAME"],
        key=azure_config["AZURE_BATCH_ACCOUNT_KEY"],
    )
    return BatchServiceClient(
        credentials=credentials, batch_url=f"https://{azure_config['AZURE_BATCH_ACCOUNT_URL']}"
    )


def create_blob_client(azure_config: Dict[str, Any]) -> BlobServiceClient:
    """Create Azure Blob Storage client"""
    connection_string = (
        f"DefaultEndpointsProtocol=https;"
        f"AccountName={azure_config['AZURE_STORAGE_ACCOUNT_NAME']};"
        f"AccountKey={azure_config['AZURE_STORAGE_ACCOUNT_KEY']};"
        f"EndpointSuffix=core.windows.net"
    )
    return BlobServiceClient.from_connection_string(connection_string)


# ============================================================================
# Pool Management
# ============================================================================


def create_pool(
    batch_client: BatchServiceClient,
    pool_id: str,
    vm_size: str,
    node_count: int,
    docker_image: str,
    azure_config: Dict[str, Any],
) -> None:
    """Create a container-enabled pool with privileged mode support"""

    logging.info(f"Creating pool {pool_id} with {node_count} nodes of size {vm_size}...")

    # Container configuration
    # Note: We don't pre-fetch the image here because it's ~40GB and would fill the temp disk
    # Instead, we configure Docker to use the OS disk in the start task, then pull the image
    container_conf = batchmodels.ContainerConfiguration(
        type="dockerCompatible",
        container_image_names=[],  # Don't pre-fetch, will pull after Docker is reconfigured
    )

    # Virtual machine configuration using Ubuntu with container support
    # Use OSDisk with larger size to accommodate large container images (~40GB for winarena)
    os_disk = batchmodels.OSDisk(
        ephemeral_os_disk_settings=None,  # Use managed disk, not ephemeral
        caching=batchmodels.CachingType.read_write,
        managed_disk=batchmodels.ManagedDisk(
            storage_account_type=batchmodels.StorageAccountType.standard_lrs
        ),
        disk_size_gb=128,  # Large enough for ~40GB container image + OS + temp space
    )

    vm_configuration = batchmodels.VirtualMachineConfiguration(
        image_reference=batchmodels.ImageReference(
            publisher="microsoft-azure-batch",
            offer="ubuntu-server-container",
            sku="20-04-lts",
            version="latest",
        ),
        node_agent_sku_id="batch.node.ubuntu 20.04",
        container_configuration=container_conf,
        os_disk=os_disk,
    )

    # Start task to prepare the node:
    # 1. Configure Docker to use OS disk instead of temp disk (for large images)
    # 2. Pull the container image
    # Note: No need to stop systemd-resolved/named/nginx - with --privileged mode,
    # the container has its own network namespace and dnsmasq runs isolated inside it.
    # Using base64 encoded script to avoid shell escaping issues
    import base64

    # Get ACR credentials if available
    acr_server = azure_config.get("AZURE_ACR_SERVER", "")
    acr_username = azure_config.get("AZURE_ACR_USERNAME", "")
    acr_password = azure_config.get("AZURE_ACR_PASSWORD", "")

    # Add docker login if using ACR
    docker_login_cmd = ""
    if acr_server and docker_image.startswith(acr_server):
        docker_login_cmd = f"""
# Login to Azure Container Registry for faster pulls
echo "Logging into ACR: {acr_server}..."
echo '{acr_password}' | docker login {acr_server} -u {acr_username} --password-stdin
"""

    start_task_script = f"""#!/bin/bash
set -ex
echo "Starting node configuration..."

# Configure Docker to use OS disk for data (the 128GB managed disk we configured)
echo "Configuring Docker to use OS disk for image storage..."
mkdir -p /var/lib/docker_new
systemctl stop docker 2>/dev/null || true

# Create Docker daemon config to use the new data-root
mkdir -p /etc/docker
cat > /etc/docker/daemon.json << 'DOCKEREOF'
{{"data-root": "/var/lib/docker_new", "storage-driver": "overlay2"}}
DOCKEREOF

cat /etc/docker/daemon.json

# Start Docker with new config
systemctl start docker
{docker_login_cmd}
echo "Docker configured. Pulling container image..."
df -h

# Pull the container image
docker pull {docker_image}

echo "Image pulled successfully"
docker images
df -h
echo "Node startup complete"
"""
    # Encode the script as base64
    encoded_script = base64.b64encode(start_task_script.encode()).decode()

    start_task = batchmodels.StartTask(
        command_line=f'/bin/bash -c "echo {encoded_script} | base64 -d | bash"',
        user_identity=batchmodels.UserIdentity(
            auto_user=batchmodels.AutoUserSpecification(
                scope=batchmodels.AutoUserScope.pool,
                elevation_level=batchmodels.ElevationLevel.admin,
            )
        ),
        wait_for_success=True,
        max_task_retry_count=2,
    )

    # Pool specification
    pool = batchmodels.PoolAddParameter(
        id=pool_id,
        vm_size=vm_size,
        virtual_machine_configuration=vm_configuration,
        target_dedicated_nodes=node_count,
        target_low_priority_nodes=0,
        start_task=start_task,
        task_scheduling_policy=batchmodels.TaskSchedulingPolicy(
            node_fill_type=batchmodels.ComputeNodeFillType.pack
        ),
        enable_inter_node_communication=False,
        task_slots_per_node=1,
        mount_configuration=[
            batchmodels.MountConfiguration(
                azure_blob_file_system_configuration=batchmodels.AzureBlobFileSystemConfiguration(
                    account_name=azure_config["AZURE_STORAGE_ACCOUNT_NAME"],
                    account_key=azure_config["AZURE_STORAGE_ACCOUNT_KEY"],
                    container_name=azure_config["AZURE_STORAGE_CONTAINER_INPUT"],
                    relative_mount_path="input",  # Will mount to $AZ_BATCH_NODE_MOUNTS_DIR/input
                    blobfuse_options="-o attr_timeout=240 -o entry_timeout=240 -o negative_timeout=120 --file-cache-timeout-in-seconds=120",
                )
            )
        ],
    )

    try:
        batch_client.pool.add(pool)
        logging.info(f"Pool {pool_id} created successfully")
    except batchmodels.BatchErrorException as e:
        if "PoolExists" in str(e):
            logging.info(f"Pool {pool_id} already exists, reusing...")
        else:
            raise


def wait_for_pool_ready(
    batch_client: BatchServiceClient, pool_id: str, target_nodes: int, timeout_minutes: int = 30
) -> bool:
    """Wait for pool nodes to be ready"""

    logging.info(f"Waiting for pool {pool_id} to be ready...")

    start_time = datetime.now()
    timeout = timedelta(minutes=timeout_minutes)

    while datetime.now() - start_time < timeout:
        pool = batch_client.pool.get(pool_id)

        logging.info(
            f"Pool allocation state: {pool.allocation_state}, "
            f"dedicated nodes: {pool.current_dedicated_nodes}/{pool.target_dedicated_nodes}"
        )

        if pool.allocation_state == batchmodels.AllocationState.steady:
            nodes = list(batch_client.compute_node.list(pool_id))
            ready_nodes = [
                n
                for n in nodes
                if n.state
                in [batchmodels.ComputeNodeState.idle, batchmodels.ComputeNodeState.running]
            ]

            if len(ready_nodes) >= target_nodes:
                logging.info(f"Pool {pool_id} is ready with {len(ready_nodes)} nodes")
                return True

            # Check for failed nodes
            failed_nodes = [
                n for n in nodes if n.state == batchmodels.ComputeNodeState.start_task_failed
            ]
            if failed_nodes:
                logging.error(f"Found {len(failed_nodes)} nodes with start task failures")
                for node in failed_nodes:
                    logging.error(
                        f"  Node {node.id}: {node.start_task_info.failure_info.message if node.start_task_info and node.start_task_info.failure_info else 'Unknown error'}"
                    )

        time.sleep(30)

    raise TimeoutError(f"Pool {pool_id} did not become ready in {timeout_minutes} minutes")


def delete_pool(batch_client: BatchServiceClient, pool_id: str):
    """Delete a pool"""
    try:
        batch_client.pool.delete(pool_id)
        logging.info(f"Pool {pool_id} deletion initiated")
    except batchmodels.BatchErrorException as e:
        logging.warning(f"Failed to delete pool {pool_id}: {e}")


# ============================================================================
# Job Management
# ============================================================================


def create_job(batch_client: BatchServiceClient, job_id: str, pool_id: str):
    """Create a job on the specified pool"""

    logging.info(f"Creating job {job_id} on pool {pool_id}...")

    job = batchmodels.JobAddParameter(
        id=job_id,
        pool_info=batchmodels.PoolInformation(pool_id=pool_id),
        on_all_tasks_complete=batchmodels.OnAllTasksComplete.terminate_job,
        on_task_failure=batchmodels.OnTaskFailure.no_action,
    )

    try:
        batch_client.job.add(job)
        logging.info(f"Job {job_id} created successfully")
    except batchmodels.BatchErrorException as e:
        if "JobExists" in str(e):
            logging.info(f"Job {job_id} already exists")
        else:
            raise


def delete_job(batch_client: BatchServiceClient, job_id: str):
    """Delete a job"""
    try:
        batch_client.job.delete(job_id)
        logging.info(f"Job {job_id} deleted")
    except batchmodels.BatchErrorException as e:
        logging.warning(f"Failed to delete job {job_id}: {e}")


# ============================================================================
# Storage Operations
# ============================================================================


def generate_container_sas_url(
    azure_config: Dict[str, Any],
    container_name: str,
    permissions: str = "rl",
    expiry_hours: int = 48,
) -> str:
    """Generate SAS URL for a blob container"""

    sas_token = generate_container_sas(
        account_name=azure_config["AZURE_STORAGE_ACCOUNT_NAME"],
        container_name=container_name,
        account_key=azure_config["AZURE_STORAGE_ACCOUNT_KEY"],
        permission=ContainerSasPermissions(
            read="r" in permissions,
            list="l" in permissions,
            write="w" in permissions,
            delete="d" in permissions,
        ),
        expiry=datetime.utcnow() + timedelta(hours=expiry_hours),
    )

    return f"https://{azure_config['AZURE_STORAGE_ACCOUNT_NAME']}.blob.core.windows.net/{container_name}?{sas_token}"


def create_resource_files(
    azure_config: Dict[str, Any], input_container: str, blob_prefix: str = ""
) -> list:
    """Create resource files specification for downloading input data"""

    container_url = generate_container_sas_url(azure_config, input_container, permissions="rl")

    return [
        batchmodels.ResourceFile(
            storage_container_url=container_url, blob_prefix=blob_prefix, file_path="input"
        )
    ]


def create_output_files(azure_config: Dict[str, Any], exp_name: str, worker_id: int) -> list:
    """Create output file upload specification"""

    container_url = generate_container_sas_url(
        azure_config, azure_config["AZURE_STORAGE_CONTAINER_OUTPUT"], permissions="rwdl"
    )

    return [
        # Upload Batch stdout/stderr
        batchmodels.OutputFile(
            file_pattern="../std*.txt",
            destination=batchmodels.OutputFileDestination(
                container=batchmodels.OutputFileBlobContainerDestination(
                    container_url=container_url, path=f"{exp_name}/worker_{worker_id}/logs"
                )
            ),
            upload_options=batchmodels.OutputFileUploadOptions(
                upload_condition=batchmodels.OutputFileUploadCondition.task_completion
            ),
        ),
        # Upload all output files from agent_outputs
        batchmodels.OutputFile(
            file_pattern="agent_outputs/**/*",
            destination=batchmodels.OutputFileDestination(
                container=batchmodels.OutputFileBlobContainerDestination(
                    container_url=container_url, path=f"{exp_name}/worker_{worker_id}"
                )
            ),
            upload_options=batchmodels.OutputFileUploadOptions(
                upload_condition=batchmodels.OutputFileUploadCondition.task_completion
            ),
        ),
    ]


# ============================================================================
# Task Creation
# ============================================================================


def create_task(
    batch_client: BatchServiceClient,
    job_id: str,
    task_id: str,
    worker_id: int,
    num_workers: int,
    exp_name: str,
    config: Dict[str, Any],
    azure_config: Dict[str, Any],
) -> None:
    """Create a task for a single worker"""

    logging.info(f"Creating task {task_id} (worker {worker_id}/{num_workers})...")

    # Environment variables
    env_settings = [
        batchmodels.EnvironmentSetting(name="WORKER_ID", value=str(worker_id)),
        batchmodels.EnvironmentSetting(name="NUM_WORKERS", value=str(num_workers)),
        batchmodels.EnvironmentSetting(name="EXP_NAME", value=exp_name),
        # Storage settings for azcopy
        batchmodels.EnvironmentSetting(
            name="AZURE_STORAGE_ACCOUNT", value=azure_config["AZURE_STORAGE_ACCOUNT_NAME"]
        ),
        batchmodels.EnvironmentSetting(
            name="AZURE_STORAGE_CONTAINER", value=azure_config["AZURE_STORAGE_CONTAINER_INPUT"]
        ),
        batchmodels.EnvironmentSetting(
            name="AZURE_STORAGE_SAS", value=azure_config.get("AZURE_STORAGE_SAS", "")
        ),
    ]

    # Add API key environment variables
    if "OPENAI_API_KEY" in azure_config:
        env_settings.append(
            batchmodels.EnvironmentSetting(
                name="OPENAI_API_KEY", value=azure_config["OPENAI_API_KEY"]
            )
        )
    elif "AZURE_API_KEY" in azure_config and "AZURE_ENDPOINT" in azure_config:
        env_settings.extend(
            [
                batchmodels.EnvironmentSetting(
                    name="AZURE_API_KEY", value=azure_config["AZURE_API_KEY"]
                ),
                batchmodels.EnvironmentSetting(
                    name="AZURE_ENDPOINT", value=azure_config["AZURE_ENDPOINT"]
                ),
            ]
        )
    else:
        raise ValueError(
            "Either OPENAI_API_KEY or both AZURE_API_KEY and AZURE_ENDPOINT must be provided"
        )

    # Container settings with privileged mode for NET_ADMIN capability
    # Mount the blobfuse mount point into the container
    # Note: The winarena image has entrypoint ["/bin/bash", "-c"], so command_line should be
    # the script content directly (not wrapped in /bin/bash -c again)
    # Using taskWorkingDirectory sets container's cwd to the task working directory and
    # stdout/stderr are captured to stdout.txt/stderr.txt
    container_settings = batchmodels.TaskContainerSettings(
        image_name=config["docker_img_name"],
        container_run_options=(
            "--privileged "
            "--shm-size=16g "
            "-v /mnt/batch/tasks/fsmounts/input:/mnt/input "  # No :ro - VM needs write access to storage
        ),
        working_directory=batchmodels.ContainerWorkingDirectory.task_working_directory,
    )

    # Command line - the script to run inside the container
    # Note: The winarena image has entrypoint ["/bin/bash", "-c"], so we need to wrap
    # the entire script in quotes so it gets passed as a single argument to bash -c
    # The container's working directory is the task working directory (via taskWorkingDirectory setting)
    # Using semicolons instead of newlines for command separation
    command_line = (
        '"set -e; '
        f"echo 'Starting Windows Arena task for worker {worker_id}...'; "
        "echo 'Timestamp:' $(date); "
        "echo 'Working directory:' $(pwd); "
        # Display mount contents
        "echo 'Mount contents (/mnt/input):'; "
        "ls -la /mnt/input/ 2>/dev/null || echo 'Mount empty or not accessible'; "
        # Download storage files using azcopy (blobfuse has I/O issues with large files)
        "echo 'Downloading storage files using azcopy (this may take a few minutes)...'; "
        "mkdir -p /storage; "
        'azcopy copy "https://${AZURE_STORAGE_ACCOUNT}.blob.core.windows.net/${AZURE_STORAGE_CONTAINER}/storage/*?${AZURE_STORAGE_SAS}" /storage/ --recursive; '
        "echo 'Storage contents:'; "
        "ls -la /storage/ 2>/dev/null || echo 'Storage empty or not accessible'; "
        # Create output directory
        f"mkdir -p ./agent_outputs/{exp_name}; "
        # Test NET_ADMIN capability
        "echo 'Testing NET_ADMIN capability...'; "
        "ip link add dummy0 type dummy 2>/dev/null && echo 'NET_ADMIN: OK' || echo 'NET_ADMIN: FAILED'; "
        "ip link delete dummy0 2>/dev/null || true; "
        # Start VM
        "echo 'Starting Windows VM...'; "
        "/entry_setup.sh; "
        # Run benchmark
        "echo 'Running benchmark client...'; "
        f"cd /client && python run.py "
        f"--agent_name {config['agent']} "
        f"--worker_id {worker_id} "
        f"--num_workers {num_workers} "
        f"--result_dir $AZ_BATCH_TASK_WORKING_DIR/agent_outputs/{exp_name} "
        f"--test_all_meta_path {config['json_name']} "
        f"--model {config['model_name']} "
        f"--som_origin {config['som_origin']} "
        f"--a11y_backend {config['a11y_backend']} "
        "--emulator_ip 172.30.0.2; "
        "echo 'Task completed successfully'\""
    )

    # No resource files needed - using blobfuse mount instead

    # Output file specifications
    output_files = create_output_files(azure_config, exp_name, worker_id)

    task = batchmodels.TaskAddParameter(
        id=task_id,
        command_line=command_line,
        container_settings=container_settings,
        environment_settings=env_settings,
        # No resource_files - using blobfuse mount instead
        output_files=output_files,
        user_identity=batchmodels.UserIdentity(
            auto_user=batchmodels.AutoUserSpecification(
                scope=batchmodels.AutoUserScope.pool,
                elevation_level=batchmodels.ElevationLevel.admin,
            )
        ),
        constraints=batchmodels.TaskConstraints(
            max_wall_clock_time=timedelta(hours=12), retention_time=timedelta(days=7)
        ),
    )

    batch_client.task.add(job_id, task)
    logging.info(f"Task {task_id} created successfully")


# ============================================================================
# Monitoring
# ============================================================================


def wait_for_tasks_complete(
    batch_client: BatchServiceClient, job_id: str, timeout_hours: int = 12
) -> bool:
    """Wait for all tasks in job to complete"""

    logging.info(f"Waiting for job {job_id} tasks to complete...")

    start_time = datetime.now()
    timeout = timedelta(hours=timeout_hours)

    while datetime.now() - start_time < timeout:
        tasks = list(batch_client.task.list(job_id))

        completed = [t for t in tasks if t.state == batchmodels.TaskState.completed]
        running = [t for t in tasks if t.state == batchmodels.TaskState.running]
        failed = [t for t in completed if t.execution_info and t.execution_info.exit_code != 0]

        logging.info(
            f"Tasks: {len(completed)}/{len(tasks)} completed, {len(running)} running, {len(failed)} failed"
        )

        if len(completed) == len(tasks):
            if failed:
                logging.warning(f"{len(failed)} tasks failed:")
                for t in failed:
                    exit_code = t.execution_info.exit_code if t.execution_info else "N/A"
                    logging.warning(f"  Task {t.id}: exit code {exit_code}")
            return len(failed) == 0

        time.sleep(60)

    raise TimeoutError(f"Job {job_id} did not complete in {timeout_hours} hours")


def get_task_output(
    batch_client: BatchServiceClient, job_id: str, task_id: str, filename: str = "stdout.txt"
) -> str:
    """Retrieve task output file content"""
    try:
        stream = batch_client.file.get_from_task(job_id, task_id, filename)
        return stream.read().decode("utf-8")
    except Exception as e:
        return f"Error retrieving {filename}: {e}"


# ============================================================================
# Main Orchestration
# ============================================================================


def launch_experiment(config: Dict[str, Any]) -> bool:
    """Launch a single experiment on Azure Batch"""

    # Load Azure configuration
    azure_config = load_config()

    # Validate configuration
    required_batch_keys = [
        "AZURE_BATCH_ACCOUNT_NAME",
        "AZURE_BATCH_ACCOUNT_URL",
        "AZURE_BATCH_ACCOUNT_KEY",
        "AZURE_STORAGE_ACCOUNT_NAME",
        "AZURE_STORAGE_ACCOUNT_KEY",
        "AZURE_STORAGE_CONTAINER_INPUT",
        "AZURE_STORAGE_CONTAINER_OUTPUT",
    ]

    missing_keys = [k for k in required_batch_keys if k not in azure_config]
    if missing_keys:
        raise ValueError(f"Missing required configuration keys: {missing_keys}")

    # Create clients
    batch_client = create_batch_client(azure_config)

    # Generate unique IDs
    timestamp = datetime.now().strftime("%Y%m%d%H%M%S")
    exp_name = config["exp_name"]
    pool_id = f"winarena-pool-{exp_name}-{timestamp}"
    job_id = f"winarena-job-{exp_name}-{timestamp}"

    num_workers = config.get("num_workers", 1)
    pool_node_count = config.get("pool_node_count") or num_workers
    vm_size = config.get("pool_vm_size", "Standard_D4s_v3")

    success = False

    try:
        # Create pool
        create_pool(
            batch_client=batch_client,
            pool_id=pool_id,
            vm_size=vm_size,
            node_count=pool_node_count,
            docker_image=config["docker_img_name"],
            azure_config=azure_config,
        )

        # Wait for pool to be ready
        wait_for_pool_ready(batch_client, pool_id, pool_node_count)

        # Create job
        create_job(batch_client, job_id, pool_id)

        # Create tasks for each worker
        for worker_id in range(num_workers):
            task_id = f"worker-{worker_id}"
            create_task(
                batch_client=batch_client,
                job_id=job_id,
                task_id=task_id,
                worker_id=worker_id,
                num_workers=num_workers,
                exp_name=exp_name,
                config=config,
                azure_config=azure_config,
            )

        # Wait for completion
        success = wait_for_tasks_complete(batch_client, job_id)

        # Print task outputs for debugging
        for worker_id in range(num_workers):
            task_id = f"worker-{worker_id}"
            logging.info(f"\n=== Output from {task_id} ===")
            output = get_task_output(batch_client, job_id, task_id)
            logging.info(output[:5000] if len(output) > 5000 else output)

        return success

    except Exception as e:
        logging.error(f"Experiment failed: {e}")
        traceback.print_exc()
        return False

    finally:
        # Cleanup
        if config.get("auto_cleanup", True):
            logging.info("Cleaning up resources...")
            delete_job(batch_client, job_id)
            delete_pool(batch_client, pool_id)


def launch_batch(experiments: Dict[str, Any], json_path: str):
    """Launch multiple experiments sequentially"""

    for exp_name, config in experiments.items():
        # Skip internal metadata keys
        if exp_name.startswith("_"):
            continue

        if config.get("_done", False) or "_stop_time" in config:
            logging.info(f"Skipping completed experiment: {exp_name}")
            continue

        if "_start_time" in config and "_stop_time" not in config:
            response = (
                input(f"Experiment '{exp_name}' was already started. Continue? (yes/no/skip): ")
                .strip()
                .lower()
            )
            if response == "skip":
                logging.info(f"Skipping experiment: {exp_name}")
                continue
            elif response != "yes":
                logging.info("Aborting...")
                return

        logging.info(f"\n{'='*60}")
        logging.info(f"Launching experiment: {exp_name}")
        logging.info(f"{'='*60}\n")

        config["_start_time"] = time.strftime("%Y-%m-%d %H:%M:%S")
        save_experiments(experiments, json_path)

        try:
            success = launch_experiment(config)
            config["_stop_time"] = time.strftime("%Y-%m-%d %H:%M:%S")
            config["_done"] = success
        except Exception as e:
            logging.error(f"Experiment {exp_name} failed: {e}")
            traceback.print_exc()
            config["_error"] = str(e)

        save_experiments(experiments, json_path)

    logging.info("\nAll experiments completed.")


# ============================================================================
# Entry Point
# ============================================================================


def setup_logging():
    """Setup logging configuration"""
    log_directory = "./azure_batch_logs"
    os.makedirs(log_directory, exist_ok=True)
    log_filename = f"run_{datetime.now().strftime('%Y-%m-%d_%H-%M-%S')}.log"

    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s - %(levelname)s - %(message)s",
        handlers=[
            logging.StreamHandler(),
            logging.FileHandler(os.path.join(log_directory, log_filename)),
        ],
    )

    return os.path.join(log_directory, log_filename)


def main():
    log_file = setup_logging()
    logging.info(f"Logging to: {log_file}")

    args = load_args_as_dict()

    if args.get("experiments_json"):
        json_path = args["experiments_json"]
        experiments = load_experiments(json_path)

        if args.get("update_json"):
            # Update experiments with CLI args
            for exp_name, config in experiments.items():
                if not exp_name.startswith("_"):
                    for key, value in args.items():
                        if key not in ["experiments_json", "update_json"] and value is not None:
                            config[key] = value
            save_experiments(experiments, json_path)
            logging.info(f"Updated {json_path} with CLI arguments")
        else:
            launch_batch(experiments, json_path)
    else:
        launch_experiment(args)


if __name__ == "__main__":
    main()
