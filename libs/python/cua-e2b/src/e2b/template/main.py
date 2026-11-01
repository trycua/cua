"""The template builder: `Template().from_...().run_cmd(...)...`.

A template is a recipe: a base, a list of instructions (`RUN`, `COPY`, `ENV`,
`WORKDIR`, `USER`), a start command and a ready command. This module only
records the recipe, with the same builder methods, instruction format and
JSON as e2b's; `AsyncTemplate.build` / `Template.build` run it (see
`e2b.template._engine` for what the build does on Cua).

The e2b SDK is MIT-licensed; this module matches its public API.
"""

import json
import shlex
from pathlib import Path
from types import TracebackType
from typing import Dict, List, Literal, Optional, Union

from e2b.connection_config import ClientFactory
from e2b.exceptions import BuildException, InvalidArgumentException
from e2b.template.consts import RESOLVE_SYMLINKS
from e2b.template.dockerfile_parser import parse_dockerfile
from e2b.template.readycmd import ReadyCmd, wait_for_file
from e2b.template.types import (
    CopyItem,
    Instruction,
    InstructionType,
    RegistryConfig,
    TemplateType,
)
from e2b.template.utils import (
    calculate_files_hash,
    get_caller_directory,
    get_caller_frame,
    make_traceback,
    pad_octal,
    read_dockerignore,
    read_gcp_service_account_json,
    validate_relative_path,
)


class TemplateBuilder:
    """Adds instructions to a template. Every method returns the builder, so
    calls chain."""

    def __init__(self, template: "TemplateBase"):
        self._template = template

    def _push(self, instruction: Instruction) -> "TemplateBuilder":
        self._template._instructions.append(instruction)
        self._template._collect_stack_trace()
        return self

    def copy(
        self,
        src: Union[Union[str, Path], List[Union[str, Path]]],
        dest: Union[str, Path],
        force_upload: Optional[Literal[True]] = None,
        user: Optional[str] = None,
        mode: Optional[int] = None,
        resolve_symlinks: Optional[bool] = None,
        gzip: Optional[bool] = None,
    ) -> "TemplateBuilder":
        """Copies files or directories from the build context into the template.

        :param src: source path(s), relative to the context directory
        :param dest: destination path in the template
        :param force_upload: upload even if the files are cached
        :param user: owner, `user` or `user:group`
        :param mode: permissions, e.g. `0o755`
        :param resolve_symlinks: follow symlinks instead of copying them as links
        :param gzip: gzip the archive (default True)
        """
        srcs = [src] if isinstance(src, (str, Path)) else src
        stack_trace = make_traceback(get_caller_frame())

        for item in srcs:
            src_string = str(item)
            validate_relative_path(src_string, stack_trace)
            instruction: Instruction = {
                "type": InstructionType.COPY,
                "args": [
                    src_string,
                    str(dest),
                    user or "",
                    pad_octal(mode) if mode is not None else "",
                ],
                "force": force_upload or self._template._force_next_layer,
                "forceUpload": force_upload,
                "resolveSymlinks": resolve_symlinks,
                "gzip": gzip,
            }
            # One stack trace per pushed instruction keeps steps and traces aligned.
            self._push(instruction)
        return self

    def copy_items(self, items: List[CopyItem]) -> "TemplateBuilder":
        """Several `copy` calls from a list of `{"src", "dest", ...}` items."""
        stack_trace = make_traceback(get_caller_frame())
        for item in items:
            try:
                self.copy(
                    item["src"],
                    item["dest"],
                    item.get("forceUpload"),
                    item.get("user"),
                    item.get("mode"),
                    item.get("resolveSymlinks"),
                    item.get("gzip"),
                )
            except Exception as error:
                if stack_trace is not None:
                    raise error.with_traceback(stack_trace)
                raise
        return self

    def remove(
        self,
        path: Union[Union[str, Path], List[Union[str, Path]]],
        force: bool = False,
        recursive: bool = False,
        user: Optional[str] = None,
    ) -> "TemplateBuilder":
        """Removes files or directories in the template (`rm`)."""
        paths = [path] if isinstance(path, (str, Path)) else path
        args = ["rm"]
        if recursive:
            args.append("-r")
        if force:
            args.append("-f")
        args.extend(shlex.quote(str(p)) for p in paths)
        return self.run_cmd(" ".join(args), user=user)

    def rename(
        self,
        src: Union[str, Path],
        dest: Union[str, Path],
        force: bool = False,
        user: Optional[str] = None,
    ) -> "TemplateBuilder":
        """Renames or moves a file or directory in the template (`mv`)."""
        args = ["mv", shlex.quote(str(src)), shlex.quote(str(dest))]
        if force:
            args.append("-f")
        return self.run_cmd(" ".join(args), user=user)

    def make_dir(
        self,
        path: Union[Union[str, Path], List[Union[str, Path]]],
        mode: Optional[int] = None,
        user: Optional[str] = None,
    ) -> "TemplateBuilder":
        """Creates directories in the template (`mkdir -p`)."""
        paths = [path] if isinstance(path, (str, Path)) else path
        args = ["mkdir", "-p"]
        if mode is not None:
            args.append(f"-m {pad_octal(mode)}")
        args.extend(shlex.quote(str(p)) for p in paths)
        return self.run_cmd(" ".join(args), user=user)

    def make_symlink(
        self,
        src: Union[str, Path],
        dest: Union[str, Path],
        user: Optional[str] = None,
        force: bool = False,
    ) -> "TemplateBuilder":
        """Creates a symbolic link `dest` to `src` (`ln -s`)."""
        args = ["ln", "-s"]
        if force:
            args.append("-f")
        args.extend([shlex.quote(str(src)), shlex.quote(str(dest))])
        return self.run_cmd(" ".join(args), user=user)

    def run_cmd(
        self, command: Union[str, List[str]], user: Optional[str] = None
    ) -> "TemplateBuilder":
        """Runs a shell command during the build; a list is joined with `&&`."""
        commands = [command] if isinstance(command, str) else command
        args = [" && ".join(commands)]
        if user:
            args.append(user)
        return self._push(
            {
                "type": InstructionType.RUN,
                "args": args,
                "force": self._template._force_next_layer,
                "forceUpload": None,
            }
        )

    def set_workdir(self, workdir: Union[str, Path]) -> "TemplateBuilder":
        """Sets the working directory of the following commands."""
        return self._push(
            {
                "type": InstructionType.WORKDIR,
                "args": [str(workdir)],
                "force": self._template._force_next_layer,
                "forceUpload": None,
            }
        )

    def set_user(self, user: str) -> "TemplateBuilder":
        """Sets the user of the following commands."""
        return self._push(
            {
                "type": InstructionType.USER,
                "args": [user],
                "force": self._template._force_next_layer,
                "forceUpload": None,
            }
        )

    def pip_install(
        self, packages: Optional[Union[str, List[str]]] = None, g: bool = True
    ) -> "TemplateBuilder":
        """`pip install` (globally by default, `--user` with `g=False`; no
        packages installs the current directory)."""
        if isinstance(packages, str):
            packages = [packages]
        args = ["pip", "install"]
        if not g:
            args.append("--user")
        args.extend(packages if packages else ["."])
        return self.run_cmd(" ".join(args), user="root" if g else None)

    def npm_install(
        self,
        packages: Optional[Union[str, List[str]]] = None,
        g: Optional[bool] = False,
        dev: Optional[bool] = False,
    ) -> "TemplateBuilder":
        """`npm install` (no packages installs from package.json)."""
        if isinstance(packages, str):
            packages = [packages]
        args = ["npm", "install"]
        if g:
            args.append("-g")
        if dev:
            args.append("--save-dev")
        if packages:
            args.extend(packages)
        return self.run_cmd(" ".join(args), user="root" if g else None)

    def bun_install(
        self,
        packages: Optional[Union[str, List[str]]] = None,
        g: Optional[bool] = False,
        dev: Optional[bool] = False,
    ) -> "TemplateBuilder":
        """`bun install` (no packages installs from package.json)."""
        if isinstance(packages, str):
            packages = [packages]
        args = ["bun", "install"]
        if g:
            args.append("-g")
        if dev:
            args.append("--dev")
        if packages:
            args.extend(packages)
        return self.run_cmd(" ".join(args), user="root" if g else None)

    def apt_install(
        self,
        packages: Union[str, List[str]],
        no_install_recommends: bool = False,
        fix_missing: bool = False,
    ) -> "TemplateBuilder":
        """`apt-get install -y` as root, after an `apt-get update`."""
        if isinstance(packages, str):
            packages = [packages]
        flags = ("--no-install-recommends " if no_install_recommends else "") + (
            "--fix-missing " if fix_missing else ""
        )
        return self.run_cmd(
            [
                "apt-get update",
                "DEBIAN_FRONTEND=noninteractive DEBCONF_NOWARNINGS=yes "
                f"apt-get install -y {flags}{' '.join(packages)}",
            ],
            user="root",
        )

    def add_mcp_server(self, servers: Union[str, List[str]]) -> "TemplateBuilder":
        """Installs MCP servers with `mcp-gateway pull`; the template must
        start `from_template("mcp-gateway")`."""
        if self._template._base_template != "mcp-gateway":
            stack_trace = make_traceback(get_caller_frame())
            raise BuildException(
                "MCP servers can only be added to mcp-gateway template"
            ).with_traceback(stack_trace)
        if isinstance(servers, str):
            servers = [servers]
        return self.run_cmd(f"mcp-gateway pull {' '.join(servers)}", user="root")

    def git_clone(
        self,
        url: str,
        path: Optional[Union[str, Path]] = None,
        branch: Optional[str] = None,
        depth: Optional[int] = None,
        user: Optional[str] = None,
    ) -> "TemplateBuilder":
        """Clones a git repository into the template."""
        args = ["git", "clone", shlex.quote(url)]
        if branch:
            args.append(f"--branch {shlex.quote(branch)}")
            args.append("--single-branch")
        if depth:
            args.append(f"--depth {depth}")
        if path:
            args.append(shlex.quote(str(path)))
        return self.run_cmd(" ".join(args), user=user)

    def beta_dev_container_prebuild(
        self, devcontainer_directory: Union[str, Path]
    ) -> "TemplateBuilder":
        """Prebuilds a devcontainer; the template must start `from_template("devcontainer")`."""
        if self._template._base_template != "devcontainer":
            stack_trace = make_traceback(get_caller_frame())
            raise BuildException(
                "Devcontainers can only used in the devcontainer template"
            ).with_traceback(stack_trace)
        return self.run_cmd(
            f"devcontainer build --workspace-folder {shlex.quote(str(devcontainer_directory))}",
            user="root",
        )

    def beta_set_dev_container_start(
        self, devcontainer_directory: Union[str, Path]
    ) -> "TemplateFinal":
        """Starts a devcontainer as the start command (must be the last call)."""
        if self._template._base_template != "devcontainer":
            stack_trace = make_traceback(get_caller_frame())
            raise BuildException(
                "Devcontainers can only used in the devcontainer template"
            ).with_traceback(stack_trace)
        dir_ = shlex.quote(str(devcontainer_directory))
        return self.set_start_cmd(
            "sudo devcontainer up --workspace-folder "
            + dir_
            + " && sudo /prepare-exec.sh "
            + dir_
            + " | sudo tee /devcontainer.sh > /dev/null && sudo chmod +x /devcontainer.sh"
            " && sudo touch /devcontainer.up",
            wait_for_file("/devcontainer.up"),
        )

    def set_envs(self, envs: Dict[str, str]) -> "TemplateBuilder":
        """Sets environment variables for the following build steps and the start command."""
        if len(envs) == 0:
            return self
        return self._push(
            {
                "type": InstructionType.ENV,
                "args": [item for key, value in envs.items() for item in [key, value]],
                "force": self._template._force_next_layer,
                "forceUpload": None,
            }
        )

    def skip_cache(self) -> "TemplateBuilder":
        """Rebuilds every instruction from here on, ignoring cached layers."""
        self._template._force_next_layer = True
        return self

    def set_start_cmd(self, start_cmd: str, ready_cmd: Union[str, ReadyCmd]) -> "TemplateFinal":
        """Sets the command started in every sandbox, and the check that tells it is ready."""
        self._template._start_cmd = start_cmd
        if isinstance(ready_cmd, ReadyCmd):
            ready_cmd = ready_cmd.get_cmd()
        self._template._ready_cmd = ready_cmd
        self._template._collect_stack_trace()
        return TemplateFinal(self._template)

    def set_ready_cmd(self, ready_cmd: Union[str, ReadyCmd]) -> "TemplateFinal":
        """Sets the check that tells the sandbox is ready."""
        if isinstance(ready_cmd, ReadyCmd):
            ready_cmd = ready_cmd.get_cmd()
        self._template._ready_cmd = ready_cmd
        self._template._collect_stack_trace()
        return TemplateFinal(self._template)


class TemplateFinal:
    """A template whose start and ready commands are set: nothing can follow."""

    def __init__(self, template: "TemplateBase"):
        self._template = template


class TemplateBase(ClientFactory):
    """The start of a template: choose a base with a `from_*` method."""

    _logs_refresh_frequency = 0.2

    def __init__(
        self,
        file_context_path: Optional[Union[str, Path]] = None,
        file_ignore_patterns: Optional[List[str]] = None,
    ):
        """
        :param file_context_path: base path for the relative paths of `copy`
            (default: the directory of the calling file)
        :param file_ignore_patterns: `.dockerignore`-syntax patterns of files not
            to copy; applied after the context's `.dockerignore`. An invalid
            pattern raises `TemplateException`.
        """
        self._default_base_image: str = "e2bdev/base"
        self._base_image: Optional[str] = self._default_base_image
        self._base_template: Optional[str] = None
        self._registry_config: Optional[RegistryConfig] = None
        self._start_cmd: Optional[str] = None
        self._ready_cmd: Optional[str] = None
        # Rebuild the whole template / the next layer, ignoring caches.
        self._force: bool = False
        self._force_next_layer: bool = False
        self._instructions: List[Instruction] = []
        self._file_context_path = (
            file_context_path.as_posix()
            if isinstance(file_context_path, Path)
            else (file_context_path or get_caller_directory() or ".")
        )
        self._file_ignore_patterns: List[str] = file_ignore_patterns or []
        self._stack_traces: List[Union[TracebackType, None]] = []

    def skip_cache(self) -> "TemplateBase":
        """Rebuilds everything from here on, ignoring cached layers."""
        self._force_next_layer = True
        return self

    def _collect_stack_trace(self) -> "TemplateBase":
        """Remembers where user code added the instruction being pushed."""
        self._stack_traces.append(make_traceback(get_caller_frame()))
        return self

    def _base_chosen(self) -> TemplateBuilder:
        # A FROM under skip_cache() invalidates the whole template.
        if self._force_next_layer:
            self._force = True
        self._collect_stack_trace()
        return TemplateBuilder(self)

    # bases

    def from_debian_image(self, variant: str = "stable") -> TemplateBuilder:
        """Starts from a Debian image."""
        return self.from_image(f"debian:{variant}")

    def from_ubuntu_image(self, variant: str = "latest") -> TemplateBuilder:
        """Starts from an Ubuntu image."""
        return self.from_image(f"ubuntu:{variant}")

    def from_fedora_image(self, variant: str = "44") -> TemplateBuilder:
        """Starts from a Fedora image."""
        return self.from_image(f"fedora:{variant}")

    def from_alpine_image(self, variant: str = "3.24") -> TemplateBuilder:
        """Starts from an Alpine image."""
        return self.from_image(f"alpine:{variant}")

    def from_arch_image(self, variant: str = "latest") -> TemplateBuilder:
        """Starts from an Arch Linux image (a rolling release: `latest`)."""
        return self.from_image(f"archlinux:{variant}")

    def from_python_image(self, version: str = "3") -> TemplateBuilder:
        """Starts from a Python image."""
        return self.from_image(f"python:{version}")

    def from_node_image(self, variant: str = "lts") -> TemplateBuilder:
        """Starts from a Node.js image."""
        return self.from_image(f"node:{variant}")

    def from_bun_image(self, variant: str = "latest") -> TemplateBuilder:
        """Starts from a Bun image."""
        return self.from_image(f"oven/bun:{variant}")

    def from_base_image(self) -> TemplateBuilder:
        """Starts from the base image (`e2bdev/base`; on Cua, the Linux image)."""
        return self.from_image(self._default_base_image)

    def from_image(
        self,
        image: str,
        username: Optional[str] = None,
        password: Optional[str] = None,
    ) -> TemplateBuilder:
        """Starts from an image, with registry credentials if it is private.

        :raises InvalidArgumentException: when only one of username/password is given
        """
        if username is not None or password is not None:
            if not username or not password:
                stack_trace = make_traceback(get_caller_frame())
                raise InvalidArgumentException(
                    "Both username and password are required when providing registry credentials"
                ).with_traceback(stack_trace)
            self._registry_config = {"type": "registry", "username": username, "password": password}
        self._base_image = image
        self._base_template = None
        return self._base_chosen()

    def from_template(self, template: str) -> TemplateBuilder:
        """Starts from a template built before (`name` or `name:tag`)."""
        self._base_template = template
        self._base_image = None
        return self._base_chosen()

    def from_dockerfile(self, dockerfile_content_or_path: str) -> TemplateBuilder:
        """Starts from a Dockerfile (its text, or the path of the file)."""
        builder = TemplateBuilder(self)
        # Each parsed instruction collects its own stack trace (this call site).
        self._base_image = parse_dockerfile(dockerfile_content_or_path, builder)
        return self._base_chosen()

    def from_aws_registry(
        self, image: str, access_key_id: str, secret_access_key: str, region: str
    ) -> TemplateBuilder:
        """Starts from an image in AWS ECR."""
        self._base_image = image
        self._base_template = None
        self._registry_config = {
            "type": "aws",
            "awsAccessKeyId": access_key_id,
            "awsSecretAccessKey": secret_access_key,
            "awsRegion": region,
        }
        return self._base_chosen()

    def from_gcp_registry(
        self, image: str, service_account_json: Union[str, dict]
    ) -> TemplateBuilder:
        """Starts from an image in GCP Artifact Registry / Container Registry;
        the service account is JSON text, a dict or the path of a JSON file."""
        self._base_image = image
        self._base_template = None
        self._registry_config = {
            "type": "gcp",
            "serviceAccountJson": read_gcp_service_account_json(
                self._file_context_path, service_account_json
            ),
        }
        return self._base_chosen()

    # exports

    @staticmethod
    def to_json(template: "TemplateClass") -> str:
        """The template as the JSON a build receives (COPY steps carry the
        hash of their files, so the files must exist)."""
        return json.dumps(
            template._template._serialize(template._template._instructions_with_hashes()),
            indent=2,
        )

    @staticmethod
    def to_dockerfile(template: "TemplateClass") -> str:
        """The template as a Dockerfile.

        :raises ValueError: for a template built from another template, or with no base image
        """
        t = template._template
        if t._base_template is not None:
            raise ValueError(
                "Cannot convert template built from another template to Dockerfile. "
                "Templates based on other templates can only be built using the E2B API."
            )
        if t._base_image is None:
            raise ValueError("No base image specified for template")

        dockerfile = f"FROM {t._base_image}\n"
        for instruction in t._instructions:
            kind, args = instruction["type"], instruction["args"]
            if kind == InstructionType.RUN:
                dockerfile += f"RUN {args[0]}\n"
            elif kind == InstructionType.COPY:
                dockerfile += f"COPY {args[0]} {args[1]}\n"
            elif kind == InstructionType.ENV:
                values = [f"{args[i]}={args[i + 1]}" for i in range(0, len(args), 2)]
                dockerfile += f"ENV {' '.join(values)}\n"
            else:
                dockerfile += f"{kind.value} {' '.join(args)}\n"
        if t._start_cmd:
            dockerfile += f"ENTRYPOINT {t._start_cmd}\n"
        return dockerfile

    def _instructions_with_hashes(self) -> List[Instruction]:
        """The instructions, COPY steps with the hash of their files."""
        steps: List[Instruction] = []
        for index, instruction in enumerate(self._instructions):
            step: Instruction = {
                "type": instruction["type"],
                "args": instruction["args"],
                "force": instruction["force"],
                "forceUpload": instruction.get("forceUpload"),
                "resolveSymlinks": instruction.get("resolveSymlinks"),
                "gzip": instruction.get("gzip"),
            }
            if instruction["type"] == InstructionType.COPY:
                stack_trace = None
                if index + 1 < len(self._stack_traces):
                    stack_trace = self._stack_traces[index + 1]
                args = instruction.get("args", [])
                src = args[0] if len(args) > 0 else None
                dest = args[1] if len(args) > 1 else None
                if src is None or dest is None:
                    raise ValueError("Source path and destination path are required")
                resolve_symlinks = instruction.get("resolveSymlinks")
                step["filesHash"] = calculate_files_hash(
                    src,
                    dest,
                    self._file_context_path,
                    [*read_dockerignore(self._file_context_path), *self._file_ignore_patterns],
                    resolve_symlinks if resolve_symlinks is not None else RESOLVE_SYMLINKS,
                    stack_trace,
                )
            steps.append(step)
        return steps

    def _serialize(self, steps: List[Instruction]) -> TemplateType:
        """The template in the format a build receives."""
        _steps: List[Instruction] = []
        for instruction in steps:
            step: Instruction = {
                "type": instruction.get("type"),
                "args": instruction.get("args"),
                "force": instruction.get("force"),
            }
            files_hash = instruction.get("filesHash")
            if files_hash is not None:
                step["filesHash"] = files_hash
            force_upload = instruction.get("forceUpload")
            if force_upload is not None:
                step["forceUpload"] = force_upload
            _steps.append(step)

        data: TemplateType = {"steps": _steps, "force": self._force}
        if self._base_image is not None:
            data["fromImage"] = self._base_image
        if self._base_template is not None:
            data["fromTemplate"] = self._base_template
        if self._registry_config is not None:
            data["fromImageRegistry"] = self._registry_config
        if self._start_cmd is not None:
            data["startCmd"] = self._start_cmd
        if self._ready_cmd is not None:
            data["readyCmd"] = self._ready_cmd
        return data


TemplateClass = Union[TemplateFinal, TemplateBuilder]
