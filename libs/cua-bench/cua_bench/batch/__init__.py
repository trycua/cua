"""Batch integration for cua-bench."""

import asyncio
from pathlib import Path
from typing import List, Optional

# Import GCP batch utilities from the batch_example
# We'll adapt the execute function for cua-bench use
try:
    from . import gcp as _  # noqa: F401

    GCP_AVAILABLE = True
except ImportError:
    GCP_AVAILABLE = False


async def execute_batch(
    job_name: str,
    env_path: Path,
    container_script: str,
    task_count: int = 4,
    task_parallelism: int = 4,
    run_local: bool = False,
    image_uri: Optional[str] = None,
    auto_cleanup: bool = True,
    output_dir: Optional[str] = None,
) -> List[str]:
    """Execute a batch job for cua-bench environment.

    Args:
        job_name: Name of the batch job
        env_path: Path to the environment directory
        container_script: Script to run in the container
        task_count: Number of tasks to run
        task_parallelism: Max concurrent tasks
        run_local: Run locally using Docker instead of GCP
        image_uri: Custom container image
        auto_cleanup: Clean up resources after completion

    Returns:
        List of log lines from the job
    """
    if not GCP_AVAILABLE and not run_local:
        raise RuntimeError(
            "GCP dependencies not installed. Install with: pip install google-cloud-batch google-cloud-logging google-cloud-storage gcloud-aio-storage"
        )

    # For local runs, use Docker
    if run_local:
        return await run_local_docker(
            env_path=env_path,
            container_script=container_script.format(env_path="/app/env"),
            image_uri=image_uri,
            output_dir=output_dir,
            task_count=task_count,
            parallelism=task_parallelism,
        )

    # Import the batch execution from batch_example
    from . import gcp

    # Upload environment to GCS and run batch job
    mapped_folders = [
        (str(env_path.absolute()), "/mnt/disks/env"),
    ]

    # If an output_dir is provided for a GCP run, ensure a GCS bucket exists and
    # mount it into the container at /tmp/td_output to collect results.
    gcs_volumes = None
    if output_dir and not run_local:
        bucket_name = "cua-bench-data"
        try:
            # Best-effort create; ignore if it already exists
            await gcp.create_bucket(gcp.PROJECT_ID, bucket_name, gcp.REGION)
        except Exception as e:
            print(f"Failed to create output bucket: {e}")
        gcs_volumes = [
            {
                "gcs": {"remotePath": f"gs://{bucket_name}"},
                "mountPath": "/tmp/td_output",
            }
        ]

    result = await gcp.execute(
        job_name=job_name,
        mapped_folders=mapped_folders,
        container_script=container_script.format(env_path="/mnt/disks/env"),
        task_count=task_count,
        task_parallelism=task_parallelism,
        image_uri=image_uri,
        auto_cleanup=auto_cleanup,
        print_logs=True,
        gcs_volumes=gcs_volumes,
    )
    # After the job, if output_dir was requested, copy results locally from GCS.
    if output_dir and not run_local:
        out_path = Path(output_dir).absolute()
        out_path.mkdir(parents=True, exist_ok=True)
        # Use gsutil to copy everything from the bucket to the local output dir
        import asyncio as _asyncio

        proc = await _asyncio.create_subprocess_exec(
            "gsutil",
            "cp",
            "-r",
            "gs://cua-bench-data/",
            str(out_path),
            stdout=_asyncio.subprocess.PIPE,
            stderr=_asyncio.subprocess.PIPE,
        )
        stdout, stderr = await proc.communicate()
        if stdout:
            for line in stdout.decode().splitlines():
                print(line)
        if stderr:
            for line in stderr.decode().splitlines():
                print(line)
        if proc.returncode != 0:
            raise RuntimeError(f"gsutil copy failed with code {proc.returncode}")

    return result or []


async def run_local_docker(
    env_path: Path,
    container_script: str,
    image_uri: Optional[str] = None,
    output_dir: Optional[str] = None,
    task_count: int = 1,
    parallelism: int = 1,
) -> List[str]:
    """Run the batch job locally using Docker.

    Args:
        env_path: Path to environment directory
        container_script: Script to run
        image_uri: Docker image to use
        output_dir: Local directory to mount as /tmp/td_output for results
        task_count: Total number of tasks to run
        parallelism: Maximum number of concurrent containers

    Returns:
        List of output lines
    """
    image = image_uri or "cua-bench:latest"

    # Prepare output directory
    if output_dir:
        output_path = Path(output_dir).absolute()
        output_path.mkdir(parents=True, exist_ok=True)
        print(f"Output directory: {output_path} -> /tmp/td_output")

    print(f"Running {task_count} tasks with parallelism {parallelism}\n")

    all_output = []

    async def run_task(task_index: int) -> List[str]:
        """Run a single task in a Docker container."""
        # Build docker command for this specific task
        docker_cmd = [
            "docker",
            "run",
            "--rm",
            "-e",
            f"BATCH_TASK_INDEX={task_index}",
            "-e",
            f"BATCH_TASK_COUNT={task_count}",
            "-v",
            f"{env_path.absolute()}:/app/env:ro",
        ]

        # Add output directory mount if specified
        if output_dir:
            docker_cmd.extend(["-v", f"{output_path}:/tmp/td_output"])

        docker_cmd.append(image)
        docker_cmd.extend(["/bin/sh", "-c", container_script])

        print(f"[Task {task_index}] Starting...")

        # Run container and capture output
        process = await asyncio.create_subprocess_exec(
            *docker_cmd,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )

        stdout, stderr = await process.communicate()

        output_lines = []
        if stdout:
            for line in stdout.decode().splitlines():
                print(f"[Task {task_index}] {line}")
                output_lines.append(line)

        if stderr:
            for line in stderr.decode().splitlines():
                print(f"[Task {task_index}] [stderr] {line}")

        if process.returncode != 0:
            raise RuntimeError(f"Task {task_index} failed with code {process.returncode}")

        print(f"[Task {task_index}] ✓ Completed\n")
        return output_lines

    # Run tasks with limited parallelism
    semaphore = asyncio.Semaphore(parallelism)

    async def run_with_semaphore(task_index: int):
        async with semaphore:
            return await run_task(task_index)

    # Create tasks for all indices
    tasks = [run_with_semaphore(i) for i in range(task_count)]

    # Run all tasks and collect results
    results = await asyncio.gather(*tasks, return_exceptions=True)

    # Check for errors and collect output
    for i, result in enumerate(results):
        if isinstance(result, Exception):
            print(f"✗ Task {i} failed: {result}")
            raise result
        else:
            all_output.extend(result)

    return all_output
