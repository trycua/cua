"""A small Dockerfile reader: instructions with their flags, arguments and
heredocs, and the shell-word processing Dockerfile values go through (quotes,
escapes, `$VAR` references).

It reads what `Template.from_dockerfile` needs, following Dockerfile syntax
(parser directives, line continuations, comments inside continued lines,
JSON-form arguments, `RUN`/`COPY`/`ADD` heredocs); it does not build anything.
This package's own implementation.
"""

from __future__ import annotations

import json
import re
from dataclasses import dataclass, field
from typing import Dict, List, Optional, Tuple


class DockerfileSyntaxError(ValueError):
    """The Dockerfile is malformed or uses something a template cannot."""

    def __init__(self, message: str, line: Optional[int] = None) -> None:
        self.line = line
        if line is None:
            super().__init__(f"Dockerfile parse error: {message}")
        else:
            super().__init__(f"Dockerfile parse error on line {line}: {message}")


@dataclass
class DockerfileHeredoc:
    name: str
    content: str = ""
    chomp: bool = False  # `<<-NAME`: leading tabs are stripped
    expand: bool = True  # an unquoted terminator: `$VAR` in the body expands
    file_descriptor: int = 0


@dataclass
class DockerfileInstruction:
    name: str
    args: List[str]
    flags: List[str] = field(default_factory=list)
    json: bool = False
    heredocs: List[DockerfileHeredoc] = field(default_factory=list)
    start_line: int = 1


@dataclass
class DockerfileWarning:
    message: str
    line: int


@dataclass
class DockerfileAst:
    instructions: List[DockerfileInstruction]
    escape_token: str = "\\"
    warnings: List[DockerfileWarning] = field(default_factory=list)


_INSTRUCTIONS = {
    "ADD", "ARG", "CMD", "COPY", "ENTRYPOINT", "ENV", "EXPOSE", "FROM", "HEALTHCHECK",
    "LABEL", "MAINTAINER", "ONBUILD", "RUN", "SHELL", "STOPSIGNAL", "USER", "VOLUME",
    "WORKDIR",
}  # fmt: skip
_HEREDOC_INSTRUCTIONS = {"ADD", "COPY", "RUN"}
_DIRECTIVE = re.compile(r"^#\s*([a-zA-Z][a-zA-Z0-9]*)\s*=\s*(.+?)\s*$")
_VALID_DIRECTIVES = {"syntax", "escape", "check"}
_NAME_CHARS = re.compile(r"\d+|[@*#?\-$!0]|\w+")
_HEREDOC_MARKER = re.compile(r"^(\d*)<<(-?)(.*)$", re.DOTALL)


def is_space(ch: str) -> bool:
    return ch in " \t\n\v\f\r"


# ── shell words ────────────────────────────────────────────────────────────


def split_words(text: str, escape: str = "\\") -> List[str]:
    """Splits on unquoted whitespace, keeping quotes and escapes in the words
    (they are resolved later by `process_word`)."""
    words: List[str] = []
    cur: List[str] = []
    i, n = 0, len(text)
    quote: Optional[str] = None
    started = False
    while i < n:
        ch = text[i]
        if quote:
            cur.append(ch)
            if ch == escape and quote == '"' and i + 1 < n:
                cur.append(text[i + 1])
                i += 1
            elif ch == quote:
                quote = None
        elif ch in "'\"":
            quote = ch
            started = True
            cur.append(ch)
        elif ch == escape and i + 1 < n:
            started = True
            cur.append(ch)
            cur.append(text[i + 1])
            i += 1
        elif is_space(ch):
            if started or cur:
                words.append("".join(cur))
                cur, started = [], False
        else:
            started = True
            cur.append(ch)
        i += 1
    if quote:
        what = "double" if quote == '"' else "single"
        raise ValueError(f"unexpected end of statement while looking for matching {what}-quote")
    if started or cur:
        words.append("".join(cur))
    return words


def _read_name(text: str, pos: int) -> Tuple[str, int]:
    m = _NAME_CHARS.match(text, pos)
    return (m.group(0), m.end()) if m else ("", pos)


def _brace_end(text: str, pos: int) -> int:
    """Index of the `}` that closes the `${` whose body starts at `pos`."""
    depth = 1
    while pos < len(text):
        if text[pos] == "{":
            depth += 1
        elif text[pos] == "}":
            depth -= 1
            if depth == 0:
                return pos
        pos += 1
    raise ValueError("syntax error: missing '}'")


def process_word(word: str, escape: str = "\\", env: Optional[Dict[str, str]] = None) -> str:
    """Resolves quotes and escapes in one word.

    `$VAR` and `${VAR...}` references stay as written when `env` is None (the
    shell that later runs the value expands them); with an `env` they are
    replaced from it (`${V:-d}`, `${V-d}`, `${V:+a}` and `${V+a}` supported;
    an unset variable is empty).
    """
    out: List[str] = []
    i, n = 0, len(word)
    quote: Optional[str] = None

    def reference() -> None:
        nonlocal i
        # word[i] == "$"
        if i + 1 < n and word[i + 1] == "{":
            end = _brace_end(word, i + 2)
            body = word[i + 2 : end]
            out.append(("${" + body + "}") if env is None else _expand_braced(body, env, escape))
            i = end + 1
            return
        name, nxt = _read_name(word, i + 1)
        if not name:
            out.append("$")
            i += 1
            return
        out.append(("$" + name) if env is None else env.get(name, ""))
        i = nxt

    while i < n:
        ch = word[i]
        if quote == "'":
            if ch == "'":
                quote = None
            else:
                out.append(ch)
            i += 1
        elif quote == '"':
            if ch == '"':
                quote = None
                i += 1
            elif ch == escape and i + 1 < n:
                nxt = word[i + 1]
                if nxt in ('"', "$", escape, "`"):
                    out.append(nxt)
                else:
                    out.append(ch + nxt)
                i += 2
            elif ch == "$":
                reference()
            else:
                out.append(ch)
                i += 1
        else:
            if ch in "'\"":
                quote = ch
                i += 1
            elif ch == escape:
                if i + 1 < n:
                    out.append(word[i + 1])
                i += 2
            elif ch == "$":
                reference()
            else:
                out.append(ch)
                i += 1
    if quote:
        what = "double" if quote == '"' else "single"
        raise ValueError(f"unexpected end of statement while looking for matching {what}-quote")
    return "".join(out)


def _expand_braced(body: str, env: Dict[str, str], escape: str) -> str:
    m = re.match(r"(\d+|[@*#?\-$!0]|\w+)(.*)$", body, re.DOTALL)
    if m is None:
        raise ValueError("syntax error: bad substitution")
    name, rest = m.group(1), m.group(2)
    value = env.get(name)
    if rest == "":
        return value or ""
    colon = rest.startswith(":")
    op = rest[1:] if colon else rest
    if not op or op[0] not in "+-":
        raise ValueError(f"unsupported modifier ({rest[:2]}) in substitution")
    word = process_word(op[1:], escape, env)
    present = value is not None and (value != "" or not colon)
    if op[0] == "-":
        return value if present else word
    return word if present else ""


# ── heredocs ───────────────────────────────────────────────────────────────


def parse_heredoc(word: str) -> Optional[DockerfileHeredoc]:
    """The heredoc a word such as `<<EOF`, `<<-'EOF'` or `3<<EOF` opens, or None."""
    m = _HEREDOC_MARKER.match(word)
    if m is None or not m.group(3):
        return None
    fd, chomp, rest = m.group(1), m.group(2) == "-", m.group(3)
    try:
        words = split_words(rest)
        if len(words) != 1:
            return None
        name = process_word(words[0])
    except ValueError:
        return None
    # A quoted terminator makes the body literal.
    expand = "'" not in rest and '"' not in rest and "\\" not in rest
    return DockerfileHeredoc(
        name=name, chomp=chomp, expand=expand, file_descriptor=int(fd) if fd else 0
    )


def chomp_heredoc_content(content: str) -> str:
    """Leading tabs removed from every line (`<<-` heredocs)."""
    return re.sub(r"^\t+", "", content, flags=re.MULTILINE)


# ── instructions ───────────────────────────────────────────────────────────


def _escape_directive(lines: List[str]) -> Tuple[str, int]:
    """The escape token set by a parser directive, and how many lines of
    directives open the file."""
    escape, used = "\\", 0
    for index, line in enumerate(lines):
        m = _DIRECTIVE.match(line.strip())
        if m is None or m.group(1).lower() not in _VALID_DIRECTIVES:
            break
        used = index + 1
        if m.group(1).lower() == "escape":
            value = m.group(2)
            if value not in ("\\", "`"):
                raise DockerfileSyntaxError(f"invalid ESCAPE '{value}'. Must be ` or \\", index + 1)
            escape = value
    return escape, used


def _split_flags(rest: str) -> Tuple[List[str], str]:
    flags: List[str] = []
    rest = rest.lstrip()
    while rest.startswith("--"):
        m = re.match(r"(\S+)\s*", rest)
        assert m is not None
        flags.append(m.group(1))
        rest = rest[m.end() :]
    return flags, rest


def _json_array(text: str) -> Optional[List[str]]:
    text = text.strip()
    if not text.startswith("["):
        return None
    try:
        value = json.loads(text)
    except ValueError:
        return None
    if isinstance(value, list) and all(isinstance(v, str) for v in value):
        return value
    return None


def _env_pairs(rest: str, escape: str, line: int) -> List[str]:
    """`KEY=value KEY2="v 2"` (or the legacy `KEY some value`) as a flat
    [key, value, ...] list of raw words."""
    try:
        words = split_words(rest, escape)
    except ValueError as e:
        raise DockerfileSyntaxError(str(e), line) from None
    if not words:
        return []
    first = words[0]
    eq = first.find("=")
    quote_at = min((first.find(q) for q in "'\"" if q in first), default=len(first))
    if eq < 0 or eq > quote_at:  # legacy form: the key, then the whole rest
        head = rest.strip().split(None, 1)
        if len(head) < 2:
            raise DockerfileSyntaxError("ENV must have two arguments", line)
        return [head[0], head[1].strip()]
    out: List[str] = []
    for word in words:
        key, sep, value = word.partition("=")
        if not sep:
            raise DockerfileSyntaxError(
                f'Syntax error - can\'t find = in "{word}". Must be of the form: name=value', line
            )
        out += [key, value]
    return out


def _heredoc_bodies(
    lines: List[str], index: int, markers: List[DockerfileHeredoc], first_line: int
) -> int:
    """Reads the body of every marker from `lines[index:]`; returns the index
    after the last terminator."""
    for heredoc in markers:
        body: List[str] = []
        while True:
            if index >= len(lines):
                raise DockerfileSyntaxError("unterminated heredoc", first_line)
            current = lines[index]
            index += 1
            probe = current.lstrip("\t") if heredoc.chomp else current
            if probe == heredoc.name:
                break
            body.append(current)
        heredoc.content = "".join(f"{b}\n" for b in body)
    return index


def parse_dockerfile_ast(content: str) -> DockerfileAst:
    """Reads Dockerfile text into instructions."""
    if content.startswith("﻿"):
        content = content[1:]
    lines = content.replace("\r\n", "\n").split("\n")
    escape, first = _escape_directive(lines)
    instructions: List[DockerfileInstruction] = []
    i = first
    while i < len(lines):
        stripped = lines[i].strip()
        if stripped == "" or stripped.startswith("#"):
            i += 1
            continue
        start = i + 1
        parts: List[str] = []
        while i < len(lines):
            line = lines[i]
            if parts and (line.strip() == "" or line.strip().startswith("#")):
                i += 1  # comments and blank lines inside a continued instruction
                continue
            trimmed = line.rstrip(" \t")
            i += 1
            if trimmed.endswith(escape):
                parts.append(trimmed[: -len(escape)])
                continue
            parts.append(line)
            break
        logical = "".join(parts).strip()
        name, _, rest = logical.partition(" ")
        if "\t" in name:  # a tab separates too
            name, _, tail = logical.partition("\t")
            rest = tail
        name = name.upper()
        if name not in _INSTRUCTIONS:
            raise DockerfileSyntaxError(f"unknown instruction: {name}", start)
        flags, rest = _split_flags(rest)  # leading --flags, whatever the instruction
        rest = rest.strip()
        instruction = DockerfileInstruction(name=name, args=[], flags=flags, start_line=start)
        if name in _HEREDOC_INSTRUCTIONS:
            try:
                words = split_words(rest, escape)
            except ValueError:
                words = []
            markers = [h for h in (parse_heredoc(w) for w in words) if h is not None]
            if markers:
                i = _heredoc_bodies(lines, i, markers, start)
                instruction.heredocs = markers
        if name in ("RUN", "CMD", "ENTRYPOINT", "SHELL", "HEALTHCHECK"):
            parsed = _json_array(rest)
            if parsed is not None:
                instruction.json, instruction.args = True, parsed
            else:
                instruction.args = [rest] if rest else []
        elif name in ("COPY", "ADD"):
            parsed = _json_array(rest)
            if parsed is not None:
                instruction.json, instruction.args = True, parsed
            else:
                try:
                    instruction.args = split_words(rest, escape)
                except ValueError:
                    instruction.args = [rest]  # an unbalanced quote: one word, too few arguments
        elif name in ("FROM", "ARG", "EXPOSE", "VOLUME", "STOPSIGNAL", "ONBUILD"):
            try:
                instruction.args = split_words(rest, escape)
            except ValueError as e:
                raise DockerfileSyntaxError(str(e), start) from None
        elif name in ("WORKDIR", "USER", "MAINTAINER"):
            instruction.args = [rest] if rest else []
        elif name == "ENV":
            instruction.args = _env_pairs(rest, escape, start)
        else:  # LABEL
            instruction.args = [rest] if rest else []
        instructions.append(instruction)
    return DockerfileAst(instructions=instructions, escape_token=escape)
