from __future__ import annotations

import asyncio
import os
import re
import subprocess
from typing import Iterable

import aiofiles
import aiohttp
from gcloud.aio.storage import Storage
from google.cloud import batch_v1, logging, storage
from tqdm import tqdm

# Configuration
IMAGE_URI = os.environ.get(
    "GCP_IMAGE_URI",
    "us-central1-docker.pkg.dev/enhanced-kiln-453008-s4/bench/py311-playwright-bench:latest",
)
PROJECT_ID = os.environ.get("GCP_PROJECT_ID", "enhanced-kiln-453008-s4")
REGION = os.environ.get("GCP_REGION", "us-central1")


# Utility: sanitize a string to be a valid GCP Batch job_id
# Must match: ^[a-z]([a-z0-9-]{0,61}[a-z0-9])?$  (1-63 chars, start with letter, end with letter/digit)
def sanitize_job_id(name: str) -> str:
    s = (name or "").lower()
    # Replace invalid chars with '-'
    s = re.sub(r"[^a-z0-9-]", "-", s)
    # Collapse consecutive '-'
    s = re.sub(r"-+", "-", s)
    # Remove leading non-letters
    s = re.sub(r"^[^a-z]+", "", s)
    # Ensure starts with a letter
    if not s or not re.match(r"^[a-z]", s):
        s = "cb" + s
    # Trim to max length 63
    s = s[:63]
    # Ensure ends with letter or digit (not '-')
    s = re.sub(r"[^a-z0-9]+$", "", s)
    # Edge case: could become empty after trimming
    if not s:
        s = "cb-worker"
    return s


# region Batch Job Abstraction


async def execute(
    job_name: str,
    print_logs: bool = True,
    mapped_folders: list[tuple[str, str]] | None = None,
    gcs_volumes: list[dict] | None = None,
    container_script: str | None = None,
    auto_cleanup: bool = True,
    task_count: int | None = None,
    task_parallelism: int | None = None,
    image_uri: str | None = None,
    run_local: bool = False,
):
    """Execute a job and attach to it.

    Args:
        job_name: Name of the job.
        print_logs: Whether to print logs after completion.
        mapped_folders: Optional list of (localPath, mountPath). Each local path
            is uploaded to a temporary bucket and mounted into the container.
        gcs_volumes: Optional list of dicts with keys similar to sample JSON:
            {"gcs": {"remotePath": "gs://bucket/path"}, "mountPath": "/mnt", "mountOptions": ["ro"]}
    """
    image_uri = image_uri or IMAGE_URI
    assert job_name, "Job name must be provided"
    # Sanitize to meet GCP Batch job_id requirements
    job_name = sanitize_job_id(job_name)
    pbar = None

    # Prepare optional temp bucket & derived gcs volumes from mapped_folders
    bucket_name: str | None = None
    derived_gcs_vols: list[dict] = []
    if mapped_folders:
        bucket_name = f"{job_name}-tmp"
        # 0a) Create temp bucket
        await create_bucket(PROJECT_ID, bucket_name, REGION)
        # 0b) Upload each mapped folder and construct volume specs
        for local_path, mount_path in mapped_folders:
            if not os.path.exists(local_path):
                raise FileNotFoundError(f"mapped folder does not exist: {local_path}")
            prefix = os.path.basename(os.path.abspath(local_path)) or "data"
            remote_uri = f"gs://{bucket_name}/{prefix}"
            print(f"Uploading {local_path} to {remote_uri}")
            await upload_directory(bucket_name, local_path, prefix)
            print(f"Uploaded {local_path} to {remote_uri}")
            derived_gcs_vols.append(
                {
                    "gcs": {"remotePath": remote_uri},
                    "mountPath": mount_path,
                }
            )

    # Merge provided gcs_volumes with derived ones
    final_gcs_vols: list[dict] | None = None
    if gcs_volumes or derived_gcs_vols:
        final_gcs_vols = []
        if gcs_volumes:
            final_gcs_vols.extend(gcs_volumes)
        if derived_gcs_vols:
            final_gcs_vols.extend(derived_gcs_vols)

    # If running locally, execute container via Docker and return
    if run_local:
        image = image_uri or IMAGE_URI
        shell_cmd = container_script or ("echo Hello world! This is a local container run.")
        docker_cmd: list[str] = ["docker", "run", "--rm"]
        # Mount folders if provided
        if mapped_folders:
            for local_path, mount_path in mapped_folders:
                docker_cmd.extend(["-v", f"{os.path.abspath(local_path)}:{mount_path}:ro"])
        docker_cmd.append(image)
        docker_cmd.extend(["/bin/sh", "-c", shell_cmd])
        print(f"Running locally via Docker: {' '.join(docker_cmd)}")
        result = subprocess.run(docker_cmd, capture_output=True, text=True)
        if result.returncode != 0:
            raise RuntimeError(f"Local Docker run failed with code {result.returncode}")
        # Return stdout lines to allow parsing by caller
        return result.stdout.splitlines()

    # 0) If the job already exists, cancel
    if await job_exists(PROJECT_ID, REGION, job_name):
        if pbar:
            pbar.close()
        print(
            f"Failed! Job '{job_name}' already exists in {REGION}.\n"
            f"Consider deleting it, then re-run.\n\n $ gcloud batch jobs delete {job_name} --location={REGION}\n"
        )
        return

    result_logs: list[str] = []
    try:
        # 1) Create the job
        await create_container_job(
            PROJECT_ID,
            REGION,
            job_name,
            gcs_volumes=final_gcs_vols,
            container_script=container_script,
            task_count=task_count,
            task_parallelism=task_parallelism,
            image_uri=image_uri,
        )

        # 2) Track job progress by tasks completed
        j = await get_job(PROJECT_ID, REGION, job_name)
        S = batch_v1.JobStatus.State

        def state_str(state) -> str:
            if state == S.QUEUED:
                return "QUEUED"
            if state == S.SCHEDULED:
                return "SCHEDULED"
            if state == S.RUNNING:
                return "RUNNING"
            if state == S.SUCCEEDED:
                return "SUCCEEDED"
            if state == S.FAILED:
                return "FAILED"
            return "UNKNOWN"

        # Determine total task count
        total_tasks = None
        try:
            groups = getattr(j, "task_groups", None)
            if groups and len(groups) > 0:
                total_tasks = getattr(groups[0], "task_count", None) or None
        except Exception:
            total_tasks = None
        if total_tasks is None:
            total_tasks = task_count if task_count else 1

        # Initialize progress bar with total tasks and description "{job_name} ({job_status})"
        pbar = tqdm(total=total_tasks, desc=f"{job_name} ({state_str(j.status.state)})")

        prev_completed = 0
        # Poll until job is complete
        while True:
            j = await get_job(PROJECT_ID, REGION, job_name)
            # Update description with current job status
            pbar.set_description(f"{job_name} ({state_str(j.status.state)})")

            # Count tasks completed (SUCCEEDED or FAILED)
            completed = await count_completed_tasks(PROJECT_ID, REGION, job_name)
            if completed > prev_completed:
                pbar.update(completed - prev_completed)
                prev_completed = completed

            # Stop when job is terminal or we've completed all tasks
            if j.status.state in (S.SUCCEEDED, S.FAILED) or completed >= total_tasks:
                break
            await asyncio.sleep(2)
        pbar.close()

        # 3) If FAILED, print status events, logs, and raise
        if j.status.state == batch_v1.JobStatus.State.FAILED:
            print("Job failed. Status events:")
            try:
                for ev in j.status.status_events:
                    etype = getattr(ev, "type", None) or ""
                    desc = getattr(ev, "description", None) or ""
                    exec_info = getattr(ev, "task_execution", None)
                    stderr_snip = None
                    if exec_info is not None:
                        stderr_snip = getattr(exec_info, "stderr_snippet", None)
                    if exec_info and getattr(exec_info, "exit_code", None) is not None:
                        print(f"- {etype}: {desc} (exit_code={exec_info.exit_code})")
                    else:
                        print(f"- {etype}: {desc}")
                    if stderr_snip:
                        print("  stderr_snippet:\n" + stderr_snip)
            except Exception as _:
                pass
            # Attempt to surface container output prior to raising
            try:
                lines = await get_job_logs(PROJECT_ID, j)
                for ln in lines:
                    print(ln)
            except Exception as _:
                pass
            raise RuntimeError(f"Batch job '{job_name}' failed")

        # 4) Fetch logs and optionally print
        lines = await get_job_logs(PROJECT_ID, j)
        if print_logs:
            for ln in lines:
                print(ln)
        result_logs = lines
    finally:
        if pbar:
            pbar.close()
        print("Cleaning up cloud resources...")
        # 4) Clean up job resources
        if auto_cleanup:
            # Delete the job if it exists
            if await job_exists(PROJECT_ID, REGION, job_name):
                await delete_job(PROJECT_ID, REGION, job_name)
            # Delete temp bucket if created
            if bucket_name:
                try:
                    await delete_bucket(bucket_name)
                except Exception as e:
                    print(f"Warning: failed to delete bucket {bucket_name}: {e}")
        else:
            print("Auto cleanup is disabled. To delete resources manually:")
            print(f"  gcloud batch jobs delete {job_name} --location={REGION}")
            if bucket_name:
                print(f"  gcloud storage rm -r gs://{bucket_name}")
            print("Tip: set auto_cleanup=True to delete these automatically on completion.")

    return result_logs


# region GCP Batch Helpers


async def list_jobs(project_id: str, region: str) -> list[batch_v1.Job]:
    client = batch_v1.BatchServiceAsyncClient()
    req = batch_v1.ListJobsRequest(parent=f"projects/{project_id}/locations/{region}")
    pager = await client.list_jobs(request=req)
    return [job async for job in pager]


async def job_exists(project_id: str, region: str, job_name: str) -> bool:
    jobs = await list_jobs(project_id, region)
    return any(
        job.name == f"projects/{project_id}/locations/{region}/jobs/{job_name}" for job in jobs
    )


async def get_job(project_id: str, region: str, job_name: str) -> batch_v1.Job:
    client = batch_v1.BatchServiceAsyncClient()
    return await client.get_job(name=f"projects/{project_id}/locations/{region}/jobs/{job_name}")


async def count_completed_tasks(
    project_id: str, region: str, job_name: str, task_group_id: str = "group0"
) -> int:
    """Count tasks that have reached a terminal state (SUCCEEDED or FAILED)."""
    client = batch_v1.BatchServiceAsyncClient()
    parent = f"projects/{project_id}/locations/{region}/jobs/{job_name}/taskGroups/{task_group_id}"
    completed = 0
    try:
        pager = await client.list_tasks(parent=parent)
        async for task in pager:
            status = getattr(task, "status", None)
            state = getattr(status, "state", None)
            if state in (batch_v1.TaskStatus.State.SUCCEEDED, batch_v1.TaskStatus.State.FAILED):
                completed += 1
    except Exception:
        pass
    return completed


async def create_container_job(
    project_id: str,
    region: str,
    job_name: str,
    gcs_volumes: list[dict] | None = None,
    container_script: str | None = None,
    task_count: int | None = None,
    task_parallelism: int | None = None,
    image_uri: str | None = None,
) -> batch_v1.Job:
    """
    This method shows how to create a sample Batch Job that will run
    a simple command inside a container on Cloud Compute instances.

    Args:
        project_id: project ID or project number of the Cloud project you want to use.
        region: name of the region you want to use to run the job. Regions that are
            available for Batch are listed on: https://cloud.google.com/batch/docs/get-started#locations
        job_name: the name of the job that will be created.
            It needs to be unique for each project and region pair.

    Returns:
        A job object representing the job created.
    """
    client = batch_v1.BatchServiceAsyncClient()

    # Define what will be done as part of the job.
    runnable = batch_v1.Runnable()
    runnable.container = batch_v1.Runnable.Container()
    # runnable.container.image_uri = "gcr.io/google-containers/busybox"
    runnable.container.enable_image_streaming = True
    runnable.container.image_uri = image_uri or IMAGE_URI
    runnable.container.entrypoint = "/bin/sh"
    if container_script:
        runnable.container.commands = ["-c", container_script]
    else:
        runnable.container.commands = [
            "-c",
            "echo Hello world! This is task ${BATCH_TASK_INDEX}. This job has a total of ${BATCH_TASK_COUNT} tasks.",
        ]

    # Jobs can be divided into tasks. In this case, we have only one task.
    task = batch_v1.TaskSpec()
    task.runnables = [runnable]

    # Attach GCS volumes if provided
    if gcs_volumes:
        vols: list[batch_v1.Volume] = []
        for v in gcs_volumes:
            remote_path = v.get("gcs", {}).get("remotePath")
            mount_path = v.get("mountPath")
            mount_options: Iterable[str] | None = v.get("mountOptions")
            if not (remote_path and mount_path):
                continue
            vol = batch_v1.Volume()
            vol.gcs = batch_v1.GCS()
            vol.gcs.remote_path = remote_path.strip("gs://")
            vol.mount_path = mount_path
            if mount_options:
                vol.mount_options.extend(list(mount_options))
            vols.append(vol)
        if vols:
            task.volumes = vols

    # We can specify what resources are requested by each task.
    resources = batch_v1.ComputeResource()
    resources.cpu_milli = (
        2000  # in milliseconds per cpu-second. This means the task requires 2 whole CPUs.
    )
    resources.memory_mib = 64  # in MiB
    task.compute_resource = resources

    task.max_retry_count = 2
    task.max_run_duration = "3600s"

    # Tasks are grouped inside a job using TaskGroups.
    # Currently, it's possible to have only one task group.
    group = batch_v1.TaskGroup()
    if task_count is not None and task_count > 0:
        group.task_count = task_count
    else:
        group.task_count = 4
    if task_parallelism is not None and task_parallelism > 0:
        # Run up to this many tasks concurrently
        group.parallelism = task_parallelism
    else:
        group.parallelism = 4
    group.task_spec = task

    # Policies are used to define on what kind of virtual machines the tasks will run on.
    # In this case, we tell the system to use "e2-standard-4" machine type.
    # Read more about machine types here: https://cloud.google.com/compute/docs/machine-types
    policy = batch_v1.AllocationPolicy.InstancePolicy()
    # policy.machine_type = "e2-standard-4" # 4 vCPUs, 16 GB RAM
    policy.machine_type = "e2-highcpu-16"  # 16 vCPUs, 16 GB RAM
    instances = batch_v1.AllocationPolicy.InstancePolicyOrTemplate()
    instances.policy = policy
    allocation_policy = batch_v1.AllocationPolicy()
    allocation_policy.instances = [instances]

    job = batch_v1.Job()
    job.task_groups = [group]
    job.allocation_policy = allocation_policy
    job.labels = {"env": "testing", "type": "container"}
    # We use Cloud Logging as it's an out of the box available option
    job.logs_policy = batch_v1.LogsPolicy()
    job.logs_policy.destination = batch_v1.LogsPolicy.Destination.CLOUD_LOGGING

    create_request = batch_v1.CreateJobRequest()
    create_request.job = job
    create_request.job_id = job_name
    # The job's parent is the region in which the job will run
    create_request.parent = f"projects/{project_id}/locations/{region}"

    return await client.create_job(create_request)


async def delete_job(project_id: str, region: str, job_name: str):
    client = batch_v1.BatchServiceAsyncClient()
    operation = client.delete_job(name=f"projects/{project_id}/locations/{region}/jobs/{job_name}")
    return await (await operation).result()


async def get_job_logs(project_id: str, job: batch_v1.Job) -> list[str]:
    """
    Prints the log messages created by given job.

    Args:
        project_id: name of the project hosting the job.
        job: the job which logs you want to print.
    """
    # Initialize sync client (no official async client yet); use within async context.
    log_client = logging.Client(project=project_id)
    logger = log_client.logger("batch_task_logs")
    lines: list[str] = []
    for log_entry in logger.list_entries(filter_=f"labels.job_uid={job.uid}"):
        payload = str(log_entry.payload)
        lines.append(payload)
    return lines


# region GCS Bucket Helpers
async def create_bucket(project_id: str, bucket_name: str, location: str) -> storage.Bucket:
    """Create a GCS bucket in the given location."""
    client = storage.Client(project=project_id)
    bucket = storage.Bucket(client, name=bucket_name)
    bucket.location = location
    return client.create_bucket(bucket, project=project_id)


async def upload_directory(bucket_name: str, local_dir: str, dest_prefix: str = "") -> None:
    """Upload an entire directory to gs://bucket/dest_prefix preserving structure.

    Skips Python virtual environment directories named 'venv' or '.venv'.
    """
    local_dir = os.path.abspath(local_dir)
    async with aiohttp.ClientSession() as session:
        client = Storage(session=session)
        for root, dirs, files in tqdm(os.walk(local_dir)):
            # Skip virtual environment directories
            dirs[:] = [d for d in dirs if d not in ("venv", ".venv", "__pycache__")]
            for fname in tqdm(files):
                lpath = os.path.join(root, fname)
                rel = os.path.relpath(lpath, start=local_dir)
                object_name = os.path.join(dest_prefix, rel).replace("\\", "/")
                async with aiofiles.open(lpath, mode="rb") as f:
                    data = await f.read()
                await client.upload(bucket_name, object_name, data)


async def delete_bucket(bucket_name: str) -> None:
    """Delete a bucket and all its contents."""
    client = storage.Client()
    bucket = client.bucket(bucket_name)
    # Delete all objects (including in subpaths)
    blobs = list(client.list_blobs(bucket))
    for blob in blobs:
        blob.delete()
    bucket.delete()


if __name__ == "__main__":
    JOB_NAME = "test-job"

    # asyncio.run(execute(JOB_NAME))

    # print(job_exists(
    #     project_id=PROJECT_ID,
    #     region=REGION,
    #     job_name=JOB_NAME,
    # ))

    # jobs = list_jobs(
    #     project_id=PROJECT_ID,
    #     region=REGION,
    # )
    # print(jobs)

    # job = get_job(
    #     project_id=PROJECT_ID,
    #     region=REGION,
    #     job_name=JOB_NAME,
    # )
    # print(job)
    # print_job_logs(
    #     project_id=PROJECT_ID,
    #     job=job,
    # )

    # job = create_container_job(
    #     project_id=PROJECT_ID,
    #     region=REGION,
    #     job_name=JOB_NAME,
    # )
    # print(job)

    # result = delete_job(
    #     project_id=PROJECT_ID,
    #     region=REGION,
    #     job_name=JOB_NAME,
    # )
    # print(result)
