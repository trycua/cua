"""The blocking twin of `e2b.template_async.build_api`: the same calls with
e2b's synchronous signatures, run on the cua-e2b event loop.

The e2b SDK is MIT-licensed; this module matches its public API.
"""

import time
from types import TracebackType
from typing import Callable, Dict, List, Optional, Union

from e2b.exceptions import BuildException
from e2b.sandbox_sync.main import _Loop
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
from e2b.template.utils import get_build_step_index
from e2b.template_async import build_api as _a


def request_build(
    client: BuildClient,
    name: str,
    tags: Optional[List[str]],
    cpu_count: Optional[int],
    memory_mb: Optional[int],
    min_free_disk_mb: Optional[int] = None,
):
    """Registers a build of template `name` (`name` or `name:tag`)."""
    return _Loop.run(_a.request_build(client, name, tags, cpu_count, memory_mb, min_free_disk_mb))


def get_file_upload_link(
    client: BuildClient,
    template_id: str,
    files_hash: str,
    stack_trace: Optional[TracebackType] = None,
):
    """Where to upload the files of a COPY."""
    return _Loop.run(_a.get_file_upload_link(client, template_id, files_hash, stack_trace))


def upload_file(
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
    """Archives what `file_name` selects in the context and hands it to the build."""
    return _Loop.run(
        _a.upload_file(
            api_client,
            file_name,
            context_path,
            url,
            ignore_patterns,
            resolve_symlinks,
            gzip,
            stack_trace,
            headers=headers,
            request_timeout=request_timeout,
        )
    )


def trigger_build(
    client: BuildClient, template_id: str, build_id: str, template: TemplateType
) -> None:
    """Starts the build; it runs in the background."""
    return _Loop.run(_a.trigger_build(client, template_id, build_id, template))


def get_build_status(
    client: BuildClient,
    template_id: str,
    build_id: str,
    logs_offset: Optional[int] = None,
) -> TemplateBuildStatusResponse:
    """The status of a build and its log entries from `logs_offset`."""
    return _engine.build_status(template_id, build_id, logs_offset)


def wait_for_build_finish(
    client: BuildClient,
    template_id: str,
    build_id: str,
    on_build_logs: Optional[Callable[[LogEntry], None]] = None,
    logs_refresh_frequency: float = 0.2,
    stack_traces: List[Union[TracebackType, None]] = [],  # noqa: B006
):
    """Polls the build until it is ready.

    :raises BuildException: when the build fails
    """
    logs_offset = 0
    status = TemplateBuildStatus.BUILDING

    def poll_status() -> TemplateBuildStatusResponse:
        nonlocal logs_offset
        build_status = get_build_status(client, template_id, build_id, logs_offset)
        logs_offset += len(build_status.log_entries)
        for entry in build_status.log_entries:
            if on_build_logs:
                on_build_logs(entry)
        return build_status

    while status in [TemplateBuildStatus.BUILDING, TemplateBuildStatus.WAITING]:
        build_status = poll_status()
        status = build_status.status
        if status in [TemplateBuildStatus.READY, TemplateBuildStatus.ERROR]:
            tail = build_status
            while len(tail.log_entries) > 0:
                tail = poll_status()
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
        time.sleep(logs_refresh_frequency)

    raise BuildException("Unknown build error occurred.")


def check_alias_exists(client: BuildClient, alias: str) -> bool:
    """Whether a template with this name (or `name:tag`) exists."""
    return _engine.alias_exists(alias)


def assign_tags(client: BuildClient, target_name: str, tags: List[str]) -> TemplateTagInfo:
    """Points `tags` at the build of `target_name` (`name:tag`)."""
    return _Loop.run(_engine.assign_tags(client, target_name, tags))


def remove_tags(client: BuildClient, name: str, tags: List[str]) -> None:
    """Removes tags from a template."""
    return _Loop.run(_engine.remove_tags(client, name, tags))


def get_template_tags(client: BuildClient, template_id_or_name: str) -> List[TemplateTag]:
    """All tags of a template."""
    return _engine.get_template_tags(client, template_id_or_name)
