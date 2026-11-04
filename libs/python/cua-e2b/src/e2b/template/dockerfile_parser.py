"""`Template.from_dockerfile`: a Dockerfile as calls on a template builder.

`parse_dockerfile(text_or_path, builder)` makes the same builder calls e2b's
parser makes for the same Dockerfile (so `to_json` of the result is the
same): `USER root` and `WORKDIR /` first (Docker's defaults), then each
instruction (`RUN`, `COPY`/`ADD`, `WORKDIR`, `USER`, `ENV`, `ARG`; `CMD` and
`ENTRYPOINT` combined into the start command), then `USER user` and
`WORKDIR /home/user` unless the Dockerfile set them. `FROM` is returned as
the base image. Multi-stage builds, `COPY --from`, remote `ADD` and unknown
flags are refused with a `DockerfileSyntaxError`; metadata instructions
(`EXPOSE`, `VOLUME`, `LABEL`, `MAINTAINER`) are skipped.

The e2b SDK is MIT-licensed; this module matches its public API.
"""

import os
import re
import shlex
from typing import Dict, List, Literal, Mapping, Optional, Protocol, Tuple, Union

from e2b.template._dockerfile import (
    DockerfileHeredoc,
    DockerfileInstruction,
    DockerfileSyntaxError,
    chomp_heredoc_content,
    parse_dockerfile_ast,
    parse_heredoc,
    process_word,
)
from e2b.template.readycmd import wait_for_timeout

__all__ = [
    "DockerfileParserInterface",
    "DockerfFileFinalParserInterface",
    "DockerfileSyntaxError",
    "parse_dockerfile",
]


class DockerfFileFinalParserInterface(Protocol):
    """What the builder returns once the start command is set."""


class DockerfileParserInterface(Protocol):
    """The builder methods the parser calls."""

    def run_cmd(
        self, command: Union[str, List[str]], user: Optional[str] = None
    ) -> "DockerfileParserInterface":
        """Handle RUN instruction."""
        ...

    def copy(
        self,
        src: str,
        dest: str,
        force_upload: Optional[Literal[True]] = None,
        user: Optional[str] = None,
        mode: Optional[int] = None,
        resolve_symlinks: Optional[bool] = None,
        gzip: Optional[bool] = None,
    ) -> "DockerfileParserInterface":
        """Handle COPY instruction."""
        ...

    def set_workdir(self, workdir: str) -> "DockerfileParserInterface":
        """Handle WORKDIR instruction."""
        ...

    def set_user(self, user: str) -> "DockerfileParserInterface":
        """Handle USER instruction."""
        ...

    def set_envs(self, envs: Dict[str, str]) -> "DockerfileParserInterface":
        """Handle ENV instruction."""
        ...

    def set_start_cmd(self, start_cmd: str, ready_cmd: str) -> "DockerfFileFinalParserInterface":
        """Handle CMD/ENTRYPOINT instruction."""
        ...


# flag name -> (type, ignored). Ignored flags are accepted with a notice.
FlagSpecs = Mapping[str, Tuple[str, bool]]

_RUN_FLAGS: FlagSpecs = {
    "mount": ("string", True),
    "network": ("string", True),
    "security": ("string", True),
    "device": ("string", True),
}
_COPY_FLAGS: FlagSpecs = {
    "chown": ("string", False),
    "chmod": ("string", False),
    "from": ("string", False),
    "link": ("bool", True),
    "exclude": ("string", True),
    "parents": ("bool", True),
}
_ADD_FLAGS: FlagSpecs = {
    **_COPY_FLAGS,
    "keep-git-dir": ("bool", True),
    "checksum": ("string", True),
    "unpack": ("bool", True),
}
_FROM_FLAGS: FlagSpecs = {"platform": ("string", True)}

# Metadata with no meaning in a template.
_IGNORED_INSTRUCTIONS = {"EXPOSE", "VOLUME", "LABEL", "MAINTAINER"}
_CHMOD = re.compile(r"^[0-7]{3,4}$")
_REMOTE_URL = re.compile(r"^[a-z][a-z0-9+.-]*://", re.IGNORECASE)
_VARIABLE_NAME = re.compile(r"\d+|[@*#?\-$!0]|\w+")


def _shell_join(words: List[str]) -> str:
    return " ".join(shlex.quote(word) for word in words)


def _shell_literal(ch: str, stop_char: Optional[str] = None) -> str:
    return "\\" + ch if ch in "\\`$" or ch == stop_char else ch


def _expandable_heredoc_body(content: str, line: int) -> str:
    """Rewrites an expandable heredoc body so the shell applies Docker's
    rules: `$VAR` / `${VAR...}` are substituted, a backslash escapes the next
    character, and everything else (`$(...)` and backticks included) is
    literal."""
    pos = 0

    def missing(stop: str) -> DockerfileSyntaxError:
        what = "'/' in ${}" if stop == "/" else "'}'"
        return DockerfileSyntaxError(f"syntax error: missing {what}", line)

    def name() -> str:
        nonlocal pos
        m = _VARIABLE_NAME.match(content, pos)
        if m is None:
            return ""
        pos = m.end()
        return m.group(0)

    def variable() -> str:
        nonlocal pos
        if pos >= len(content) or content[pos] != "{":
            var = name()
            return "\\$" if var == "" else "$" + var
        pos += 1
        if pos >= len(content):
            raise missing("}")
        if content[pos] in "{}:":
            raise DockerfileSyntaxError("syntax error: bad substitution", line)
        var = name()
        if pos >= len(content):
            raise missing("}")
        modifier = content[pos]
        pos += 1
        if modifier == "}":
            return "${" + var + "}"
        if modifier == "/":
            if pos < len(content) and content[pos] == "/":
                modifier += "/"
                pos += 1
            pattern = scan("/", True)
            return "${" + var + modifier + pattern + "/" + scan("}", True) + "}"
        if modifier == ":":
            if pos >= len(content):
                raise missing("}")
            modifier += content[pos]
            pos += 1
        op = modifier[-1]
        is_pattern = op in "#%"
        if op not in "+-?#%" or (len(modifier) == 2 and is_pattern):
            raise DockerfileSyntaxError(f"unsupported modifier ({modifier}) in substitution", line)
        return "${" + var + modifier + scan("}", is_pattern) + "}"

    def scan(stop: Optional[str], raw_escapes: bool = False) -> str:
        nonlocal pos
        out: List[str] = []
        while pos < len(content):
            ch = content[pos]
            pos += 1
            if ch == stop:
                return "".join(out)
            if ch == "\\":
                if pos < len(content):
                    nxt = content[pos]
                    pos += 1
                    out.append("\\" + nxt if raw_escapes else _shell_literal(nxt, stop))
            elif ch == "$":
                out.append(variable())
            elif ch == "`":
                out.append("\\`")
            else:
                out.append(ch)
        if stop is not None:
            raise missing(stop)
        return "".join(out)

    return scan(None)


def _heredoc_terminator(name: str, content: str) -> str:
    """A heredoc delimiter that does not occur in the content it wraps."""
    terminator = "E2B_HEREDOC_" + re.sub(r"[^A-Za-z0-9_]", "_", name)
    while terminator in content:
        terminator += "_"
    return terminator


def _with_trailing_newline(content: str) -> str:
    return content if content == "" or content.endswith("\n") else content + "\n"


class _StartCommand:
    def __init__(self, json: bool, args: List[str]) -> None:
        self.json = json
        self.args = args


class _DockerfileConverter:
    def __init__(self, builder: DockerfileParserInterface, escape: str) -> None:
        self._builder = builder
        self._escape = escape
        self._vars: Dict[str, str] = {}  # ARG / ENV values declared so far
        self._user_changed = False
        self._workdir_changed = False
        self._cmd: Optional[_StartCommand] = None
        self._entrypoint: Optional[_StartCommand] = None

    def convert(self, instructions: List[DockerfileInstruction]) -> str:
        froms = [i for i in instructions if i.name == "FROM"]
        if len(froms) > 1:
            raise DockerfileSyntaxError("Multi-stage Dockerfiles are not supported")
        if not froms:
            raise DockerfileSyntaxError("Dockerfile must contain a FROM instruction")
        base_image = self._from(froms[0])

        # Docker's defaults, then e2b's below if the Dockerfile left them.
        self._builder.set_user("root")
        self._builder.set_workdir("/")
        for ins in instructions:
            if ins.name == "FROM":
                continue
            if ins.start_line < froms[0].start_line and ins.name != "ARG":
                raise DockerfileSyntaxError(
                    f"{ins.name} must be preceded by a FROM instruction", ins.start_line
                )
            self._instruction(ins)
        self._apply_start_cmd()
        if not self._user_changed:
            self._builder.set_user("user")
        if not self._workdir_changed:
            self._builder.set_workdir("/home/user")
        return base_image

    def _instruction(self, ins: DockerfileInstruction) -> None:
        name = ins.name
        if name == "RUN":
            self._run(ins)
        elif name == "COPY":
            self._copy(ins, _COPY_FLAGS)
        elif name == "ADD":
            self._copy(ins, _ADD_FLAGS)
        elif name == "WORKDIR":
            self._builder.set_workdir(self._single(ins))
            self._workdir_changed = True
        elif name == "USER":
            self._builder.set_user(self._single(ins))
            self._user_changed = True
        elif name == "ENV":
            self._env(ins)
        elif name == "ARG":
            self._arg(ins)
        elif name == "CMD":
            self._flags(ins, {})
            self._cmd = _StartCommand(ins.json, ins.args)
        elif name == "ENTRYPOINT":
            self._flags(ins, {})
            self._entrypoint = _StartCommand(ins.json, ins.args)
        elif name not in _IGNORED_INSTRUCTIONS:
            print(f"Unsupported instruction: {name}")

    def _word(self, ins: DockerfileInstruction, word: str, env: Optional[Dict[str, str]] = None):
        """Quotes and escapes of `word` resolved; `$VAR` kept unless `env`."""
        try:
            return process_word(word, self._escape, env)
        except ValueError as err:
            raise DockerfileSyntaxError(str(err), ins.start_line) from None

    def _single(self, ins: DockerfileInstruction) -> str:
        self._flags(ins, {})
        if len(ins.args) != 1:
            raise DockerfileSyntaxError(f"{ins.name} requires exactly one argument", ins.start_line)
        return self._word(ins, ins.args[0])

    def _flags(self, ins: DockerfileInstruction, specs: FlagSpecs) -> Dict[str, str]:
        values: Dict[str, str] = {}
        for flag in ins.flags:
            if not flag.startswith("--"):
                raise DockerfileSyntaxError(f"arg should start with -- : {flag}", ins.start_line)
            name, sep, raw = flag[2:].partition("=")
            has_value = sep == "="
            spec = specs.get(name)
            if spec is None:
                raise DockerfileSyntaxError(f"unknown flag: {name}", ins.start_line)
            if name in values:
                raise DockerfileSyntaxError(f"duplicate flag specified: {name}", ins.start_line)
            kind, ignored = spec
            if kind == "bool":
                if not has_value or raw in ("", "true"):
                    value = "true"
                elif raw == "false":
                    value = "false"
                else:
                    raise DockerfileSyntaxError(
                        f"expecting boolean value for flag {name}, not: {raw}", ins.start_line
                    )
            else:
                if not has_value:
                    raise DockerfileSyntaxError(f"missing a value on flag: {name}", ins.start_line)
                value = raw
            if ignored:
                print(f"Ignoring unsupported {ins.name} flag --{name} (line {ins.start_line})")
                continue
            values[name] = value
        return values

    def _from(self, ins: DockerfileInstruction) -> str:
        self._flags(ins, _FROM_FLAGS)
        args = ins.args
        if len(args) == 3 and args[1].lower() == "as":
            pass  # the stage name means nothing in a single-stage build
        elif len(args) != 1:
            raise DockerfileSyntaxError(
                "FROM requires either one or three arguments", ins.start_line
            )
        return self._word(ins, args[0])

    def _run(self, ins: DockerfileInstruction) -> None:
        self._flags(ins, _RUN_FLAGS)
        args = ins.args
        if not args:
            raise DockerfileSyntaxError("RUN requires at least one argument", ins.start_line)
        if ins.json:
            command = _shell_join(args)
        elif ins.heredocs:
            command = self._run_with_heredocs(args[0], ins.heredocs)
        else:
            command = args[0]
        self._builder.run_cmd(command)

    @staticmethod
    def _run_with_heredocs(line: str, heredocs: List[DockerfileHeredoc]) -> str:
        def body(h: DockerfileHeredoc) -> str:
            return chomp_heredoc_content(h.content) if h.chomp else h.content

        # `RUN <<EOF` alone: the body is the script.
        if len(heredocs) == 1 and parse_heredoc(line.strip()) is not None:
            h = heredocs[0]
            content = body(h)
            if not content.startswith("#!"):
                return content
            # A shebang script is written out and executed.
            script = shlex.quote(f"/tmp/{h.name}")
            end = _heredoc_terminator(h.name, content)
            return (
                f"cat <<'{end}' >{script}\n"
                + _with_trailing_newline(content)
                + f"{end}\n"
                + f"chmod +x {script} && {script}\n"
                + f"E2B_EXIT=$?; rm -f {script}; exit $E2B_EXIT"
            )
        # A heredoc inside a larger command (`cat <<EOF > file`): rebuilt for the shell.
        full = line
        for h in heredocs:
            full += "\n" + h.content + h.name
        return full

    def _copy(self, ins: DockerfileInstruction, specs: FlagSpecs) -> None:
        flags = self._flags(ins, specs)
        if len(ins.args) < 2:
            raise DockerfileSyntaxError(
                f"{ins.name} requires at least two arguments", ins.start_line
            )
        if "from" in flags:
            raise DockerfileSyntaxError(
                f"{ins.name} --from is not supported (multi-stage builds are not supported)",
                ins.start_line,
            )
        user = self._word(ins, flags["chown"]) if "chown" in flags else None
        mode: Optional[int] = None
        if "chmod" in flags:
            chmod = self._word(ins, flags["chmod"])
            resolved = self._word(ins, flags["chmod"], self._vars)
            if not _CHMOD.match(resolved):
                detail = "" if resolved == chmod else f' (resolves to "{resolved}")'
                raise DockerfileSyntaxError(
                    f'invalid chmod value "{chmod}"{detail}, expected an octal mode such as 0755',
                    ins.start_line,
                )
            mode = int(resolved, 8)

        raw_dest = ins.args[-1]
        if parse_heredoc(raw_dest) is not None:
            raise DockerfileSyntaxError(
                f"{ins.name} cannot accept a heredoc as a destination", ins.start_line
            )
        dest = self._word(ins, raw_dest)
        by_name = {h.name: h for h in ins.heredocs}
        for raw_src in ins.args[:-1]:
            marker = parse_heredoc(raw_src)
            if marker is not None:
                heredoc = by_name.get(marker.name)
                if heredoc is None:
                    raise DockerfileSyntaxError(
                        f"missing heredoc content for {marker.name}", ins.start_line
                    )
                self._copy_heredoc(heredoc, dest, user, mode, ins.start_line)
                continue
            src = self._word(ins, raw_src)
            if ins.name == "ADD" and _REMOTE_URL.match(src):
                raise DockerfileSyntaxError(
                    f"ADD from a remote URL is not supported: {src}", ins.start_line
                )
            self._builder.copy(src, dest, user=user, mode=mode)

    def _copy_heredoc(
        self,
        heredoc: DockerfileHeredoc,
        dest: str,
        user: Optional[str],
        mode: Optional[int],
        line: int,
    ) -> None:
        """`COPY <<EOF /path` writes the body to a file in the template."""
        content = chomp_heredoc_content(heredoc.content) if heredoc.chomp else heredoc.content
        if heredoc.expand:
            content = _expandable_heredoc_body(content, line)
        plain = _heredoc_terminator(heredoc.name, content)
        terminator = plain if heredoc.expand else f"'{plain}'"
        target = '"$E2B_HEREDOC_DEST"'
        # As in Docker, a directory destination receives a file named after the heredoc.
        if dest.endswith("/"):
            command = f"E2B_HEREDOC_DEST={shlex.quote(dest + heredoc.name)}\n"
        else:
            command = (
                f"E2B_HEREDOC_DEST={shlex.quote(dest)}\n"
                f"if [ -d {target} ]; then E2B_HEREDOC_DEST={target}/"
                f"{shlex.quote(heredoc.name)}; fi\n"
            )
        command += (
            f'mkdir -p "$(dirname {target})" && cat <<{terminator} >{target}\n'
            + _with_trailing_newline(content)
            + plain
        )
        if user is not None:
            command += f"\nchown {shlex.quote(user)} {target}"
        if mode is not None:
            command += f"\nchmod {mode:o} {target}"
        # COPY writes as the builder (root), whatever USER is current.
        self._builder.run_cmd(command, user="root")

    def _env(self, ins: DockerfileInstruction) -> None:
        self._flags(ins, {})
        args = ins.args
        if not args:
            raise DockerfileSyntaxError("ENV requires at least one argument", ins.start_line)
        envs: Dict[str, str] = {}
        for i in range(0, len(args) - 1, 2):
            key = self._word(ins, args[i])
            if key == "":
                raise DockerfileSyntaxError("ENV names can not be blank", ins.start_line)
            envs[key] = self._word(ins, args[i + 1])
            self._vars[key] = self._word(ins, args[i + 1], self._vars)
        self._builder.set_envs(envs)

    def _arg(self, ins: DockerfileInstruction) -> None:
        self._flags(ins, {})
        if not ins.args:
            raise DockerfileSyntaxError("ARG requires at least one argument", ins.start_line)
        envs: Dict[str, str] = {}
        for arg in ins.args:
            name, sep, value = arg.partition("=")
            key = self._word(ins, name)
            if key == "":
                raise DockerfileSyntaxError("ARG names can not be blank", ins.start_line)
            envs[key] = self._word(ins, value) if sep else ""
            self._vars[key] = self._word(ins, value, self._vars) if sep else ""
        self._builder.set_envs(envs)

    def _apply_start_cmd(self) -> None:
        """ENTRYPOINT and CMD as Docker combines them: an exec-form
        ENTRYPOINT gets CMD appended (a shell-form CMD as `/bin/sh -c <cmd>`),
        a shell-form ENTRYPOINT ignores CMD, and CMD alone is used as is."""
        cmd, entry = self._cmd, self._entrypoint
        command: Optional[str] = None
        if entry is not None:
            if entry.json:
                words = list(entry.args)
                if cmd is not None:
                    if cmd.json:
                        words.extend(cmd.args)
                    elif cmd.args:
                        words.extend(["/bin/sh", "-c", cmd.args[0]])
                command = _shell_join(words) if words else None
            else:
                command = entry.args[0] if entry.args else None
        elif cmd is not None:
            if cmd.json:
                command = _shell_join(cmd.args) if cmd.args else None
            else:
                command = cmd.args[0] if cmd.args else None
        if command is not None and command.strip() != "":
            self._builder.set_start_cmd(command, wait_for_timeout(20_000))


def _read_dockerfile(dockerfile_content_or_path: str) -> str:
    try:
        if os.path.isfile(dockerfile_content_or_path):
            with open(dockerfile_content_or_path, "r", encoding="utf-8") as f:
                return f.read()
    except (OSError, ValueError):
        pass  # not a readable path: it is the content
    return dockerfile_content_or_path


def parse_dockerfile(
    dockerfile_content_or_path: str, template_builder: DockerfileParserInterface
) -> str:
    """Converts a Dockerfile into calls on `template_builder`.

    :param dockerfile_content_or_path: Dockerfile text, or the path of one
    :param template_builder: the builder to call
    :return: the base image of the Dockerfile's `FROM`
    :raises DockerfileSyntaxError: for an invalid or unsupported Dockerfile
    """
    ast = parse_dockerfile_ast(_read_dockerfile(dockerfile_content_or_path))
    for warning in ast.warnings:
        print(f"{warning.message} (line {warning.line})")
    return _DockerfileConverter(template_builder, ast.escape_token).convert(ast.instructions)
