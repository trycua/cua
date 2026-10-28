"""`AsyncTemplate`: builds, looks up and tags templates (awaitable).

`build` follows e2b's flow call for call: `request_build`, an upload link and
upload per COPY, `trigger_build`, then `wait_for_build_finish`
(`e2b.template_async.build_api`), which here run on Cua's build engine; see
`e2b.template._engine` for what a build does and keeps.

The e2b SDK is MIT-licensed; this module matches its public API.
"""

from datetime import datetime
from typing import Callable, List, Optional, Union

from e2b.template import _engine
from e2b.template._engine import BuildClient
from e2b.template.consts import GZIP, RESOLVE_SYMLINKS
from e2b.template.logger import LogEntry, LogEntryEnd, LogEntryStart
from e2b.template.main import TemplateBase, TemplateClass
from e2b.template.types import BuildInfo, InstructionType, TemplateTag, TemplateTagInfo
from e2b.template.utils import normalize_build_arguments, read_dockerignore

from .build_api import (
    assign_tags,
    check_alias_exists,
    get_build_status,
    get_file_upload_link,
    get_template_tags,
    remove_tags,
    request_build,
    trigger_build,
    upload_file,
    wait_for_build_finish,
)


class AsyncTemplate(TemplateBase):
    """The template builder whose `build` and lookups are awaited."""

    @staticmethod
    async def _build(
        api_client: BuildClient,
        template: TemplateClass,
        name: str,
        tags: Optional[List[str]] = None,
        cpu_count: Optional[int] = None,
        memory_mb: Optional[int] = None,
        min_free_disk_mb: Optional[int] = None,
        skip_cache: bool = False,
        on_build_logs: Optional[Callable[[LogEntry], None]] = None,
        request_timeout: Optional[float] = None,
    ) -> BuildInfo:
        if skip_cache:
            template._template._force = True

        def say(message: str) -> None:
            if on_build_logs:
                on_build_logs(LogEntry(timestamp=datetime.now(), level="info", message=message))

        tags_msg = f" with tags: {', '.join(tags)}" if tags else ""
        say(f"Requesting build for template: {name}{tags_msg}")
        response = await request_build(
            api_client,
            name=name,
            cpu_count=cpu_count,
            memory_mb=memory_mb,
            min_free_disk_mb=min_free_disk_mb,
            tags=tags,
        )
        template_id = response.template_id
        build_id = response.build_id
        say(f"Template created with ID: {template_id}, Build ID: {build_id}")

        steps = template._template._instructions_with_hashes()
        for index, step in enumerate(steps):
            if step["type"] != InstructionType.COPY:
                continue
            args = step.get("args", [])
            src = args[0] if len(args) > 0 else None
            force_upload = step.get("forceUpload")
            files_hash = step.get("filesHash", None)
            resolve_symlinks = step.get("resolveSymlinks")
            if resolve_symlinks is None:
                resolve_symlinks = RESOLVE_SYMLINKS
            gzip = step.get("gzip")
            if gzip is None:
                gzip = GZIP
            if src is None or files_hash is None:
                raise ValueError("Source path and files hash are required")

            stack_trace = None
            if index + 1 < len(template._template._stack_traces):
                stack_trace = template._template._stack_traces[index + 1]

            file_info = await get_file_upload_link(api_client, template_id, files_hash, stack_trace)
            if (force_upload and file_info.url) or (file_info.present is False and file_info.url):
                await upload_file(
                    api_client,
                    src,
                    template._template._file_context_path,
                    file_info.url,
                    [
                        *read_dockerignore(template._template._file_context_path),
                        *template._template._file_ignore_patterns,
                    ],
                    resolve_symlinks,
                    gzip,
                    stack_trace,
                    headers=file_info.headers,
                    request_timeout=request_timeout,
                )
                say(f"Uploaded '{src}'")
            else:
                say(f"Skipping upload of '{src}', already cached")
        say("All file uploads completed")

        say("Starting building...")
        await trigger_build(api_client, template_id, build_id, template._template._serialize(steps))
        return BuildInfo(
            template_id=template_id,
            build_id=build_id,
            alias=name,
            name=name,
            tags=response.tags,
        )

    @classmethod
    async def build(
        cls,
        template: TemplateClass,
        name: Optional[str] = None,
        *,
        alias: Optional[str] = None,
        tags: Optional[List[str]] = None,
        cpu_count: Optional[int] = None,
        memory_mb: Optional[int] = None,
        min_free_disk_mb: Optional[int] = None,
        skip_cache: bool = False,
        on_build_logs: Optional[Callable[[LogEntry], None]] = None,
        **opts,
    ) -> BuildInfo:
        """Builds a template and keeps it under `name` (`name` or `name:tag`).

        :param template: the template to build
        :param name: template name, optionally with a tag
        :param alias: deprecated spelling of `name`
        :param tags: further tags for this build
        :param cpu_count: CPUs of the sandboxes made from the template
        :param memory_mb: memory (MiB) of those sandboxes
        :param min_free_disk_mb: free disk (MiB) the build must leave
        :param skip_cache: rebuild everything (always the case here: no cache)
        :param on_build_logs: receives the build's `LogEntry` objects
        :raises BuildException: when the build fails
        """
        name = normalize_build_arguments(name, alias)
        try:
            if on_build_logs:
                on_build_logs(LogEntryStart(timestamp=datetime.now(), message="Build started"))
            client = BuildClient(api_params=cls._resolve_api_params(**opts))
            data = await cls._build(
                client,
                template,
                name,
                tags=tags,
                cpu_count=cpu_count,
                memory_mb=memory_mb,
                min_free_disk_mb=min_free_disk_mb,
                skip_cache=skip_cache,
                on_build_logs=on_build_logs,
                request_timeout=client.api_params.get("request_timeout"),
            )
            if on_build_logs:
                on_build_logs(
                    LogEntry(timestamp=datetime.now(), level="info", message="Waiting for logs...")
                )
            await wait_for_build_finish(
                client,
                data.template_id,
                data.build_id,
                on_build_logs,
                logs_refresh_frequency=cls._logs_refresh_frequency,
                stack_traces=template._template._stack_traces,
            )
            return data
        finally:
            if on_build_logs:
                on_build_logs(LogEntryEnd(timestamp=datetime.now(), message="Build finished"))

    @classmethod
    async def build_in_background(
        cls,
        template: TemplateClass,
        name: Optional[str] = None,
        *,
        alias: Optional[str] = None,
        tags: Optional[List[str]] = None,
        cpu_count: Optional[int] = None,
        memory_mb: Optional[int] = None,
        min_free_disk_mb: Optional[int] = None,
        skip_cache: bool = False,
        on_build_logs: Optional[Callable[[LogEntry], None]] = None,
        **opts,
    ) -> BuildInfo:
        """Starts a build and returns at once; follow it with `get_build_status`.

        :return: the build's `template_id` and `build_id`
        """
        name = normalize_build_arguments(name, alias)
        client = BuildClient(api_params=cls._resolve_api_params(**opts))
        return await cls._build(
            client,
            template,
            name,
            tags=tags,
            cpu_count=cpu_count,
            memory_mb=memory_mb,
            min_free_disk_mb=min_free_disk_mb,
            skip_cache=skip_cache,
            on_build_logs=on_build_logs,
            request_timeout=client.api_params.get("request_timeout"),
        )

    @classmethod
    async def get_build_status(
        cls, build_info: BuildInfo, logs_offset: Optional[int] = None, **opts
    ):
        """The status and log entries (from `logs_offset`) of a build."""
        client = BuildClient(api_params=cls._resolve_api_params(**opts))
        return await get_build_status(
            client, build_info.template_id, build_info.build_id, logs_offset
        )

    @classmethod
    async def exists(cls, name: str, **opts) -> bool:
        """Whether a template named `name` (or `name:tag`) exists."""
        return await cls.alias_exists(name, **opts)

    @classmethod
    async def alias_exists(cls, alias: str, **opts) -> bool:
        """Deprecated: use `exists`."""
        client = BuildClient(api_params=cls._resolve_api_params(**opts))
        return await check_alias_exists(client, alias)

    @classmethod
    async def assign_tags(
        cls, target_name: str, tags: Union[str, List[str]], **opts
    ) -> TemplateTagInfo:
        """Points tag(s) at the build `target_name` (`name:tag`) names.

        :return: the build id and the assigned tags
        """
        client = BuildClient(api_params=cls._resolve_api_params(**opts))
        return await assign_tags(client, target_name, [tags] if isinstance(tags, str) else tags)

    @classmethod
    async def remove_tags(cls, name: str, tags: Union[str, List[str]], **opts) -> None:
        """Removes tag(s) from a template."""
        client = BuildClient(api_params=cls._resolve_api_params(**opts))
        await remove_tags(client, name, [tags] if isinstance(tags, str) else tags)

    @classmethod
    async def get_tags(cls, template_id: str, **opts) -> List[TemplateTag]:
        """All tags of a template, with the build each points at."""
        client = BuildClient(api_params=cls._resolve_api_params(**opts))
        return await get_template_tags(client, template_id)

    @classmethod
    async def delete(cls, name: str, **opts) -> bool:
        """Deletes a template with its tags and builds (`name`), or one tag
        (`name:tag`). `False`: there was none."""
        cls._resolve_api_params(**opts)
        return await _engine.delete_template(name)


__all__ = ["AsyncTemplate"]
