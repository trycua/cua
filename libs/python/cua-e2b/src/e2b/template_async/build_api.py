"""The calls `AsyncTemplate.build` makes to the build, in e2b's module and
with its signatures; here they run on Cua's build engine
(`e2b.template._engine`), which has no remote API behind them. `client` is
the engine's `BuildClient`, passed where e2b passes its API client.

The e2b SDK is MIT-licensed; this module matches its public API.
"""

import asyncio
import os
from types import TracebackType
from typing import Callable, Dict, List, Optional, Union

from e2b.exceptions import BuildException, FileUploadException
from e2b.template import _engine
from e2b.template._engine import BuildClient
from e2b.template.logger import LogEntry
from e2b.template.types import (
    TemplateBuildStatus,
    TemplateBuildStatusResponse,
    TemplateTag,
    TemplateTagInfo,
    TemplateType,
)
from e2b.template.utils import get_build_step_index, tar_file_stream


async def request_build(
    client: BuildClient,
    name: str,
    tags: Optional[List[str]],
    cpu_count: Optional[int],
    memory_mb: Optional[int],
    min_free_disk_mb: Optional[int] = None,
):
    """Registers a build of template `name` (`name` or `name:tag`) and returns
    its `template_id`, `build_id` and `tags`."""
    return await _engine.request_build(client, name, tags, cpu_count, memory_mb, min_free_disk_mb)


async def get_file_upload_link(
    client: BuildClient,
    template_id: str,
    files_hash: str,
    stack_trace: Optional[TracebackType] = None,
):
    """Where to upload the files of a COPY; `present` says they are cached
    (never, here: nothing is cached between builds)."""
    return await _engine.get_file_upload_link(client, template_id, files_hash)


async def upload_file(
    api_client: BuildClient,
    file_name: str,
    context_path: str,
    url: str,
    ignore_patterns: List[str],
    resolve_symlinks: bool,
    gzip: bool,
    stack_trace: Optional[TracebackType],
    request_timeout: Optional[float] = None,
    *,
    headers: Optional[Dict[str, str]] = None,
):
    """Archives what `file_name` selects in the context and hands the archive
    to the build."""
    del headers, request_timeout  # nothing is sent over a network
    try:
        tar_file = tar_file_stream(file_name, context_path, ignore_patterns, resolve_symlinks, gzip)
        try:
            size = os.fstat(tar_file.fileno()).st_size
            data = tar_file.read()
            assert len(data) == size
        finally:
            try:
                tar_file.close()
            except Exception:  # noqa: BLE001 - must not mask the result
                pass
        await _engine.store_upload(url, data)
    except Exception as e:
        raise FileUploadException(f"Failed to upload file: {e}").with_traceback(stack_trace)


async def trigger_build(
    client: BuildClient, template_id: str, build_id: str, template: TemplateType
) -> None:
    """Starts the build; it runs in the background."""
    await _engine.trigger_build(client, template_id, build_id, template)


async def get_build_status(
    client: BuildClient,
    template_id: str,
    build_id: str,
    logs_offset: Optional[int] = None,
) -> TemplateBuildStatusResponse:
    """The status of a build and its log entries from `logs_offset` (at most
    100 per call)."""
    return _engine.build_status(template_id, build_id, logs_offset)


async def wait_for_build_finish(
    client: BuildClient,
    template_id: str,
    build_id: str,
    on_build_logs: Optional[Callable[[LogEntry], None]] = None,
    logs_refresh_frequency: float = 0.2,
    stack_traces: List[Union[TracebackType, None]] = [],  # noqa: B006
):
    """Polls the build until it is ready, passing its log entries to
    `on_build_logs`.

    :raises BuildException: when the build fails
    """
    logs_offset = 0
    status = TemplateBuildStatus.BUILDING

    async def poll_status() -> TemplateBuildStatusResponse:
        nonlocal logs_offset
        build_status = await get_build_status(client, template_id, build_id, logs_offset)
        logs_offset += len(build_status.log_entries)
        for entry in build_status.log_entries:
            if on_build_logs:
                on_build_logs(entry)
        return build_status

    while status in [TemplateBuildStatus.BUILDING, TemplateBuildStatus.WAITING]:
        build_status = await poll_status()
        status = build_status.status
        if status in [TemplateBuildStatus.READY, TemplateBuildStatus.ERROR]:
            # At most 100 entries come per call: drain the rest.
            tail = build_status
            while len(tail.log_entries) > 0:
                tail = await poll_status()
            if status == TemplateBuildStatus.READY:
                return
            traceback = None
            if build_status.reason and build_status.reason.step:
                index = get_build_step_index(build_status.reason.step, len(stack_traces))
                if index < len(stack_traces):
                    traceback = stack_traces[index]
            raise BuildException(
                build_status.reason.message if build_status.reason else "Build failed"
            ).with_traceback(traceback)
        await asyncio.sleep(logs_refresh_frequency)

    raise BuildException("Unknown build error occurred.")


async def check_alias_exists(client: BuildClient, alias: str) -> bool:
    """Whether a template with this name (or `name:tag`) exists."""
    return _engine.alias_exists(alias)


async def assign_tags(client: BuildClient, target_name: str, tags: List[str]) -> TemplateTagInfo:
    """Points `tags` at the build of `target_name` (`name:tag`)."""
    return await _engine.assign_tags(client, target_name, tags)


async def remove_tags(client: BuildClient, name: str, tags: List[str]) -> None:
    """Removes tags from a template."""
    await _engine.remove_tags(client, name, tags)


async def get_template_tags(client: BuildClient, template_id_or_name: str) -> List[TemplateTag]:
    """All tags of a template."""
    return _engine.get_template_tags(client, template_id_or_name)
