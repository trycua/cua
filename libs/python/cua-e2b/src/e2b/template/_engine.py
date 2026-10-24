"""What a template build does on Cua.

e2b's `Template.build` talks to a build service in four calls
(`request_build`, an upload link per COPY, `trigger_build`, polling
`get_build_status`), and tags and aliases are further calls to it. Here those
calls are made on this module, which runs the build in a Linux VM Space and
keeps the result as a pinned Cua snapshot that `Sandbox.create(template=name)`
forks:

- the base is a Cua image: `from_base_image()` (e2b's `e2bdev/base`),
  `from_ubuntu_image()` / `ubuntu:*`, `linux`, `macos`, `windows` or one of
  Cua's own images (`ghcr.io/trycua/...`), or `from_template(name)` (another
  template: `name` or `name:tag`);
- every instruction runs in the VM (`RUN`, `ENV`, `WORKDIR`, `USER`, `COPY`),
  the start command is started and waited for with the ready command, and the
  VM is saved with its memory, so the started service already runs in every
  sandbox created from the template;
- a base that is a bare OCI image (`from_python_image`, `from_image("node:22")`,
  a Dockerfile's other `FROM`) or any registry credentials are refused with a
  `BuildException`: Cua Spaces boot Cua images (VM disks that carry
  cua-spacesd), not OCI root filesystems;
- `COPY` archives are made by the same code as e2b's (`tar_file_stream`),
  "uploaded" to this module, and unpacked in the VM with Docker's rules
  (a directory source copies its contents; several sources or a destination
  ending in `/` make the destination a directory);
- there is no layer cache: every build runs all its instructions, so
  `skip_cache` and `force_upload` hold by construction;
- `cpu_count` and `memory_mb` size the builder VM, and so every sandbox
  forked from the template; `min_free_disk_mb` is checked after the steps
  (a VM disk is sized when it is created: `CUA_E2B_DISK_GB`) and fails the
  build when the VM has less free space;
- a template is a record kept next to the Spaces registry under its name, and
  every tag `name:tag` is a record of its own pointing at the snapshot of one
  build, so `create(template="name:tag")` resolves it like any template. Every
  build also moves the `default` tag, which is what plain `name` means.
  A snapshot is deleted when no tag points at its build any more.

Builds run in the process that started them: `get_build_status` follows
builds of this process.
"""

from __future__ import annotations

import asyncio
import io
import os
import posixpath
import secrets
import shlex
import tarfile
from contextlib import asynccontextmanager
from dataclasses import dataclass, field
from datetime import datetime, timezone
from typing import Any, Dict, List, Optional, Tuple

from e2b._backend import get_backend, vm_size
from e2b.exceptions import BuildException, FileUploadException, TemplateException
from e2b.template.logger import LogEntry
from e2b.template.types import (
    BuildStatusReason,
    InstructionType,
    TemplateBuildStatus,
    TemplateBuildStatusResponse,
    TemplateTag,
    TemplateTagInfo,
    TemplateType,
)

PROFILE = "/etc/profile.d/cua-e2b-template.sh"
READY_TIMEOUT_S = 300
DEFAULT_TAG = "default"
UPLOAD_SCHEME = "cua-build://"

_OCI_WHY = (
    "Cua Spaces boot Cua images (VM disk images that carry cua-spacesd), not bare OCI "
    "images, so {ref!r} cannot be a template base. Start from `from_base_image()` (Ubuntu) "
    "and install what you need with apt_install/pip_install/run_cmd, or build a Cua image "
    "with `cua images build` and pass it to `from_image(...)`"
)
_REGISTRY_WHY = (
    "registry credentials ({kind}) are for pulling OCI images, which Cua Spaces do not boot "
    "(they boot Cua images that carry cua-spacesd), so {ref!r} cannot be a template base. "
    "Start from `from_base_image()` and install what you need, or build a Cua image with "
    "`cua images build`"
)

# Images Cua boots as VMs.
_VM_WORDS = ("linux", "macos", "windows")
_BASE_PREFIXES = ("e2bdev/base", "ubuntu", "ghcr.io/trycua/")


def vm_image_for(image: str) -> Optional[str]:
    """The Cua image a `from_image(image)` base means, or None for a bare
    OCI image."""
    if image in _VM_WORDS or image.startswith("ghcr.io/trycua/"):
        return image
    name = image.split(":", 1)[0]
    if name in ("e2bdev/base", "ubuntu"):
        return "linux"  # Cua's Linux image is Ubuntu
    return None


@dataclass
class BuildClient:
    """Stands where e2b passes its authenticated API client. There is no
    remote API here; the connection options are kept for reference."""

    api_params: Dict[str, Any] = field(default_factory=dict)
    _follow_redirects: bool = True
    _proxy: Any = None


@dataclass
class BuildRequestResponse:
    """What `request_build` answers (e2b's `TemplateRequestResponseV3`)."""

    template_id: str
    build_id: str
    tags: List[str]
    names: List[str]
    public: bool = False


@dataclass
class FileUploadInfo:
    """What `get_file_upload_link` answers (e2b's `TemplateBuildFileUpload`)."""

    present: bool
    url: Optional[str] = None
    headers: Optional[Dict[str, str]] = None


class _Build:
    def __init__(self, template_id: str, build_id: str, tags: List[str], resources: dict):
        self.template_id = template_id
        self.build_id = build_id
        self.tags = tags
        self.resources = resources
        self.status = TemplateBuildStatus.WAITING
        self.entries: List[LogEntry] = []
        self.reason: Optional[BuildStatusReason] = None
        self.task: Optional["asyncio.Future[None]"] = None

    def log(self, level: str, message: str) -> None:
        self.entries.append(LogEntry(datetime.now(timezone.utc), level, message))  # type: ignore[arg-type]


_BUILDS: Dict[str, _Build] = {}
_UPLOADS: Dict[str, bytes] = {}


# ── names, tags and records ────────────────────────────────────────────────


def split_name(name: str) -> Tuple[str, str]:
    """`name:tag` as (name, tag); a bare name has the `default` tag."""
    base, sep, tag = name.rpartition(":")
    if not sep or not base or "/" in tag:
        return name, DEFAULT_TAG
    return base, tag or DEFAULT_TAG


def _now() -> str:
    return datetime.now(timezone.utc).isoformat()


def _key(base: str, tag: str) -> str:
    return base if tag == DEFAULT_TAG else f"{base}:{tag}"


def _root(base: str) -> Optional[dict]:
    rec = get_backend().template(base)
    if not rec:
        return None
    if rec.get("builds") is None:
        if rec.get("kind") != "snapshot":
            return None
        # A template saved before tags existed: one build, the default tag.
        rec = {
            **rec,
            "builds": {"legacy": {"snapshot": rec["snapshot"], "created_at": _now()}},
            "tags": {DEFAULT_TAG: {"build_id": "legacy", "created_at": _now()}},
        }
    return rec


async def _put(key: str, record: Optional[dict]) -> None:
    """`None` removes the record (the snapshot it names stays)."""
    await get_backend().save_template(key, record)


async def _write_root(base: str, root: dict) -> None:
    """Saves the record of `base` and one record per tag, so that every
    `name:tag` resolves to its build's snapshot."""
    builds = root["builds"]
    tags = root["tags"]
    default = tags.get(DEFAULT_TAG)
    if default is not None:
        root["snapshot"] = builds[default["build_id"]]["snapshot"]
    root["kind"] = "snapshot"
    root["template_id"] = base
    await _put(base, root)
    for tag, info in tags.items():
        await _put(
            f"{base}:{tag}",
            {
                "kind": "snapshot",
                "snapshot": builds[info["build_id"]]["snapshot"],
                "template_id": base,
                "tag": tag,
                "build_id": info["build_id"],
            },
        )


async def _collect(base: str, root: dict) -> None:
    """Deletes the snapshots of builds that no tag points at."""
    from e2b.sandbox_async.main import AsyncSandbox

    used = {t["build_id"] for t in root["tags"].values()}
    for build_id in [b for b in root["builds"] if b not in used]:
        snap = root["builds"].pop(build_id)["snapshot"]
        try:
            await AsyncSandbox.delete_snapshot(snap)
        except Exception:  # noqa: BLE001 - already gone
            pass


def _not_found(what: str) -> TemplateException:
    err = TemplateException(f"404: {what} not found")
    err.status_code = 404
    return err


def _root_or_404(name: str) -> Tuple[str, dict]:
    base, _ = split_name(name)
    root = _root(base)
    if root is None:
        raise _not_found(f"template {base!r}")
    return base, root


# ── the calls of a build ───────────────────────────────────────────────────


async def request_build(
    client: BuildClient,
    name: str,
    tags: Optional[List[str]],
    cpu_count: Optional[int],
    memory_mb: Optional[int],
    min_free_disk_mb: Optional[int],
) -> BuildRequestResponse:
    base, tag = split_name(name)
    if not base:
        raise BuildException("Name must be provided")
    assigned = list(dict.fromkeys([*([tag] if ":" in name else []), *(tags or [])]))
    for t in assigned:
        if not t or "/" in t or ":" in t:
            raise BuildException(f"400: invalid tag {t!r}: a tag is one word without ':' or '/'")
    for label, value in (
        ("cpu_count", cpu_count),
        ("memory_mb", memory_mb),
        ("min_free_disk_mb", min_free_disk_mb),
    ):
        if value is not None and (not isinstance(value, int) or value < 0):
            raise BuildException(f"400: {label} must be a non-negative integer")
    for label, value in (("cpu_count", cpu_count), ("memory_mb", memory_mb)):
        if value is not None and value < 1:
            raise BuildException(f"400: {label} must be at least 1")
    build_id = secrets.token_hex(8)
    _BUILDS[build_id] = _Build(
        base,
        build_id,
        assigned or [DEFAULT_TAG],
        {"cpu_count": cpu_count, "memory_mb": memory_mb, "min_free_disk_mb": min_free_disk_mb},
    )
    return BuildRequestResponse(
        template_id=base,
        build_id=build_id,
        tags=assigned or [DEFAULT_TAG],
        names=[name],
    )


async def get_file_upload_link(client: BuildClient, template_id: str, files_hash: str):
    # Nothing is cached between builds, so a file is never "present".
    return FileUploadInfo(present=False, url=f"{UPLOAD_SCHEME}{template_id}/{files_hash}")


async def store_upload(url: str, data: bytes) -> None:
    if not url.startswith(UPLOAD_SCHEME):
        raise FileUploadException(
            f"Failed to upload file: {url!r} is not a Cua build upload link "
            f"(expected {UPLOAD_SCHEME}...)"
        )
    _UPLOADS[url] = data


async def trigger_build(
    client: BuildClient, template_id: str, build_id: str, template: TemplateType
) -> None:
    build = _BUILDS.get(build_id)
    if build is None or build.template_id != template_id:
        err = BuildException(f"404: build {build_id!r} of template {template_id!r} not found")
        raise err
    build.status = TemplateBuildStatus.BUILDING
    build.task = asyncio.ensure_future(_run(build, template))


def build_status(
    template_id: str, build_id: str, logs_offset: Optional[int] = None
) -> TemplateBuildStatusResponse:
    build = _BUILDS.get(build_id)
    if build is None or build.template_id != template_id:
        raise BuildException(f"404: build {build_id!r} of template {template_id!r} not found")
    entries = build.entries[logs_offset or 0 :][:100]  # e2b returns at most 100 per call
    return TemplateBuildStatusResponse(
        build_id=build_id,
        template_id=template_id,
        status=build.status,
        log_entries=entries,
        logs=[],
        reason=build.reason if build.status == TemplateBuildStatus.ERROR else None,
    )


def alias_exists(alias: str) -> bool:
    base, tag = split_name(alias)
    root = _root(base)
    if root is None:
        return False
    return tag in root["tags"]


async def assign_tags(client: BuildClient, target_name: str, tags: List[str]) -> TemplateTagInfo:
    base, root = _root_or_404(target_name)
    _, target_tag = split_name(target_name)
    source = root["tags"].get(target_tag)
    if source is None:
        raise _not_found(f"template {target_name!r}")
    if not tags:
        raise TemplateException("400: at least one tag is required")
    for t in tags:
        if not t or "/" in t or ":" in t:
            raise TemplateException(f"400: invalid tag {t!r}: a tag is one word without ':' or '/'")
    for t in dict.fromkeys(tags):
        root["tags"][t] = {"build_id": source["build_id"], "created_at": _now()}
    await _collect(base, root)
    await _write_root(base, root)
    return TemplateTagInfo(build_id=source["build_id"], tags=list(dict.fromkeys(tags)))


async def remove_tags(client: BuildClient, name: str, tags: List[str]) -> None:
    base, root = _root_or_404(name)
    for t in tags:
        if t == DEFAULT_TAG:
            raise TemplateException(
                f"400: the {DEFAULT_TAG!r} tag of {base!r} cannot be removed; delete the template"
            )
        if t not in root["tags"]:
            raise _not_found(f"tag {t!r} of template {base!r}")
    for t in tags:
        root["tags"].pop(t, None)
        await _put(f"{base}:{t}", None)
    await _collect(base, root)
    await _write_root(base, root)


def get_template_tags(client: BuildClient, template_id_or_name: str) -> List[TemplateTag]:
    base, root = _root_or_404(template_id_or_name)
    out = [
        TemplateTag(
            tag=tag,
            build_id=info["build_id"],
            created_at=datetime.fromisoformat(info["created_at"]),
        )
        for tag, info in root["tags"].items()
    ]
    return sorted(out, key=lambda t: (t.created_at, t.tag))


async def delete_template(name: str) -> bool:
    """Deletes a template with all its tags and builds, or one tag
    (`name:tag`). `False`: there was none."""
    from e2b.sandbox_async.main import AsyncSandbox

    base, tag = split_name(name)
    root = _root(base)
    if root is None:
        return False
    if ":" in name and tag != DEFAULT_TAG:
        if tag not in root["tags"]:
            return False
        await remove_tags(BuildClient(), name, [tag])
        return True
    for t in list(root["tags"]):
        await _put(f"{base}:{t}", None)
    for build in root["builds"].values():
        try:
            await AsyncSandbox.delete_snapshot(build["snapshot"])
        except Exception:  # noqa: BLE001 - already gone
            pass
    await _put(base, None)
    return True


# ── running a build ────────────────────────────────────────────────────────


@asynccontextmanager
async def _builder_resources(resources: dict):
    """cpu and memory of the builder VM, for the VMs created in this task."""
    with vm_size(resources.get("cpu_count"), resources.get("memory_mb")):
        yield


class _StepFailed(Exception):
    def __init__(self, message: str, step: str):
        super().__init__(message)
        self.step = step


def _archive_placements(
    data: bytes, dest: str
) -> List[Tuple[tarfile.TarInfo, str, Optional[bytes]]]:
    """(member, destination path, content) for every member of a COPY
    archive, with Docker's placement rules."""
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:*") as tar:
        members = tar.getmembers()
        names = {m.name.rstrip("/") for m in members if m.isdir()}
        tops = [m for m in members if posixpath.dirname(m.name.rstrip("/")) not in names]
        as_dir = dest.endswith("/") or len(tops) > 1 or any(m.isdir() for m in tops)
        root = dest.rstrip("/") or "/"
        out = []
        for top in tops:
            top_name = top.name.rstrip("/")
            under = [m for m in members if m.name == top.name or m.name.startswith(top_name + "/")]
            for m in under:
                rel = m.name[len(top_name) :].lstrip("/")
                if top.isdir():
                    target = root if not rel else posixpath.join(root, rel)
                elif as_dir:
                    target = posixpath.join(root, posixpath.basename(top_name))
                else:
                    target = root
                content = tar.extractfile(m).read() if m.isreg() else None  # type: ignore[union-attr]
                out.append((m, target, content))
    return out


async def _copy_step(sb, step: dict, template_id: str, user: Optional[str], log) -> None:
    args = step["args"]
    src, dest = args[0], args[1]
    owner = (args[2] if len(args) > 2 and args[2] else None) or user
    mode = int(args[3], 8) if len(args) > 3 and args[3] else None
    url = f"{UPLOAD_SCHEME}{template_id}/{step.get('filesHash')}"
    data = _UPLOADS.get(url)
    if data is None:
        raise FileUploadException(f"the files of COPY {src!r} were not uploaded")
    roots = []
    for member, target, content in _archive_placements(data, dest):
        if member.isdir():
            await sb.commands.run(f"mkdir -p {shlex.quote(target)}", user="root", timeout=60)
        elif member.issym():
            parent = posixpath.dirname(target)
            await sb.commands.run(
                f"mkdir -p {shlex.quote(parent)} && ln -sfn {shlex.quote(member.linkname)} "
                f"{shlex.quote(target)}",
                user="root",
                timeout=60,
            )
        elif member.isreg():
            await sb.files.write(target, content or b"", user="root")
            want = mode if mode is not None else member.mode & 0o7777
            if want != 0o644:
                await sb.commands.run(f"chmod {want:o} {shlex.quote(target)}", user="root")
        else:
            log("warn", f"COPY {src}: skipped {member.name!r} (not a file, directory or link)")
            continue
        if target not in roots:
            roots.append(target)
    if owner:
        for target in roots:
            await sb.commands.run(
                f"chown {shlex.quote(owner)} {shlex.quote(target)}", user="root", timeout=60
            )


async def _vm_build(build: _Build, spec: TemplateType, log) -> str:
    from e2b.sandbox_async.main import AsyncSandbox

    if spec.get("fromImageRegistry") is not None:
        kind = spec["fromImageRegistry"]["type"]
        ref = spec.get("fromImage") or spec.get("fromTemplate")
        raise _StepFailed(_REGISTRY_WHY.format(kind=kind, ref=ref), "base")
    if spec.get("fromTemplate") is not None:
        ref = spec["fromTemplate"]
        log("info", f"base: template {ref}")
    else:
        image = spec.get("fromImage") or "e2bdev/base"
        ref = vm_image_for(image)
        if ref is None:
            raise _StepFailed(_OCI_WHY.format(ref=image), "base")
        log("info", f"base: {image}" + ("" if ref == image else f" (Cua image {ref})"))

    steps = spec["steps"]
    resources = build.resources
    try:
        async with _builder_resources(resources):
            sb = await AsyncSandbox.create(template=ref, timeout=7200)
    except Exception as e:  # noqa: BLE001
        raise _StepFailed(f"could not start the builder VM from {ref!r}: {e}", "base") from e

    snapshot = f"{build.template_id}-{build.build_id}"
    try:
        envs: Dict[str, str] = {}
        cwd: Optional[str] = None
        user: Optional[str] = None
        await sb.commands.run(f"sudo rm -f {PROFILE}", timeout=60)
        for i, step in enumerate(steps, 1):
            kind = InstructionType(step["type"])
            args = step["args"]
            log("info", f"step {i}/{len(steps)}: {kind.value}")
            try:
                if kind == InstructionType.ENV:
                    pairs = dict(zip(args[0::2], args[1::2]))
                    envs.update(pairs)
                    lines = "".join(f"export {k}={shlex.quote(v)}\n" for k, v in pairs.items())
                    await sb.commands.run(
                        f"printf %s {shlex.quote(lines)} | sudo tee -a {PROFILE} >/dev/null",
                        timeout=60,
                    )
                elif kind == InstructionType.WORKDIR:
                    cwd = args[0]
                    await sb.commands.run(f"sudo mkdir -p {shlex.quote(cwd)}", timeout=60)
                elif kind == InstructionType.USER:
                    user = args[0]
                elif kind == InstructionType.RUN:
                    await sb.commands.run(
                        args[0],
                        user=(args[1] if len(args) > 1 else None) or user,
                        cwd=cwd,
                        envs=envs,
                        timeout=3600,
                        on_stdout=lambda o: log("info", o),
                        on_stderr=lambda o: log("info", o),
                    )
                elif kind == InstructionType.COPY:
                    await _copy_step(sb, step, build.template_id, user, log)
            except _StepFailed:
                raise
            except Exception as e:  # noqa: BLE001
                raise _StepFailed(str(e) or type(e).__name__, str(i)) from e

        if cwd:
            rule = f'[ "$PWD" = "$HOME" ] && cd {shlex.quote(cwd)}\n'
            await sb.commands.run(
                f"printf %s {shlex.quote(rule)} | sudo tee -a {PROFILE} >/dev/null", timeout=60
            )
        try:
            if spec.get("startCmd"):
                log("info", f"start: {spec['startCmd']}")
                await sb.commands.run(
                    spec["startCmd"], background=True, user=user, cwd=cwd, envs=envs, timeout=0
                )
            if spec.get("readyCmd"):
                log("info", f"waiting for: {spec['readyCmd']}")
                loop = asyncio.get_running_loop()
                deadline = loop.time() + READY_TIMEOUT_S
                while True:
                    try:
                        await sb.commands.run(
                            spec["readyCmd"], user=user, cwd=cwd, envs=envs, timeout=30
                        )
                        break
                    except Exception:  # noqa: BLE001 - not ready yet
                        if loop.time() > deadline:
                            raise _StepFailed(
                                "the ready command never succeeded", "finalize"
                            ) from None
                        await asyncio.sleep(1)
            wanted = resources.get("min_free_disk_mb")
            if wanted:
                out = await sb.commands.run("df -Pm / | awk 'NR==2{print $4}'", timeout=60)
                free = int((out.stdout or "0").strip() or 0)
                if free < wanted:
                    raise _StepFailed(
                        f"the builder VM has {free} MiB free after the steps, less than "
                        f"min_free_disk_mb={wanted}; a VM disk is sized when it is created, "
                        "set CUA_E2B_DISK_GB for the builder",
                        "finalize",
                    )
        except _StepFailed:
            raise
        except Exception as e:  # noqa: BLE001
            raise _StepFailed(str(e) or type(e).__name__, "finalize") from e
        try:
            await sb.create_snapshot(snapshot)
        except Exception as e:  # noqa: BLE001
            raise _StepFailed(f"could not save the template: {e}", "finalize") from e
    finally:
        await sb.kill()
    return snapshot


async def _run(build: _Build, spec: TemplateType) -> None:
    log = build.log
    try:
        snapshot = await _vm_build(build, spec, log)
        base = build.template_id
        root = _root(base) or {"builds": {}, "tags": {}}
        created = _now()
        root["builds"][build.build_id] = {"snapshot": snapshot, "created_at": created}
        for tag in dict.fromkeys([*build.tags, DEFAULT_TAG]):
            root["tags"][tag] = {"build_id": build.build_id, "created_at": created}
        await _collect(base, root)
        await _write_root(base, root)
    except _StepFailed as e:
        _fail(build, str(e), e.step)
    except Exception as e:  # noqa: BLE001
        _fail(build, f"template {build.template_id!r} failed to build: {e}", "finalize")
    else:
        log("info", f"template {build.template_id!r} is ready")
        build.status = TemplateBuildStatus.READY


def _fail(build: _Build, message: str, step: str) -> None:
    build.log("error", message)
    build.reason = BuildStatusReason(message=message, step=step, log_entries=list(build.entries))
    build.status = TemplateBuildStatus.ERROR
