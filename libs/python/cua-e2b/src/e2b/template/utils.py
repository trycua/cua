"""Pure helpers of the template builder: paths, ignore patterns, hashes,
archives, log clean-up and stack traces. Same names, arguments and results as
e2b's; the implementation (the `.dockerignore` matcher and the glob in
particular) is this package's own.

The e2b SDK is MIT-licensed; this module matches its public API.
"""

import fnmatch
import hashlib
import inspect
import json
import os
import posixpath
import re
import stat
import tarfile
import tempfile
from types import FrameType, TracebackType
from typing import IO, List, Optional, Tuple, Union

from e2b.exceptions import TemplateException
from e2b.template.consts import BASE_STEP_NAME, FINALIZE_STEP_NAME


def make_traceback(caller_frame: Optional[FrameType]) -> Optional[TracebackType]:
    """A traceback that points at `caller_frame` (user code), for errors
    raised later from SDK code; `None` without a frame."""
    if caller_frame is None:
        return None
    return TracebackType(
        tb_next=None,
        tb_frame=caller_frame,
        tb_lasti=caller_frame.f_lasti,
        tb_lineno=caller_frame.f_lineno,
    )


def validate_relative_path(src: str, stack_trace: Optional[TracebackType]) -> None:
    """Checks that a COPY source is relative and stays inside the context
    directory.

    :raises TemplateException: for an absolute path or one that escapes the context
    """
    if os.path.isabs(src):
        raise TemplateException(
            f'Invalid source path "{src}": absolute paths are not allowed. '
            "Use a relative path within the context directory."
        ).with_traceback(stack_trace)

    normalized = os.path.normpath(src)
    # '..myconfig' is a file name; only '..' and '../x' escape.
    if normalized == ".." or normalized.startswith(".." + os.sep):
        raise TemplateException(
            f'Invalid source path "{src}": path escapes the context directory. '
            "The path must stay within the context directory."
        ).with_traceback(stack_trace)


def normalize_build_arguments(name: Optional[str] = None, alias: Optional[str] = None) -> str:
    """The template name of `build(template, name)` or the legacy `alias=`.

    :raises TemplateException: when neither is given
    """
    if name and len(name) > 0:
        return name
    if alias and len(alias) > 0:
        return alias
    raise TemplateException("Name must be provided")


def read_dockerignore(context_path: str) -> List[str]:
    """The patterns of the `.dockerignore` in `context_path` (comments and
    blank lines dropped); empty without the file."""
    path = os.path.join(context_path, ".dockerignore")
    if not os.path.exists(path):
        return []
    with open(path, "r", encoding="utf-8-sig") as f:
        content = f.read()
    return [
        line.strip() for line in content.split("\n") if not line.startswith("#") and line.strip()
    ]


def normalize_path(path: str) -> str:
    """Path separators as forward slashes (what glob patterns expect)."""
    return path.replace(os.sep, "/")


# ── .dockerignore matching ─────────────────────────────────────────────────


def _pattern_regex(pattern: str) -> "re.Pattern[str]":
    """One cleaned `.dockerignore` pattern as a regular expression over a
    slash-separated relative path: `*` stays inside a segment, `**` crosses
    segments, `?` is one non-slash character, `[...]` a character class and
    `\\x` a literal. Brace expansion is not supported, as in Docker."""
    out = []
    i, n = 0, len(pattern)
    while i < n:
        c = pattern[i]
        if c == "*":
            if pattern.startswith("**", i):
                i += 2
                while i < n and pattern[i] == "*":
                    i += 1
                if i < n and pattern[i] == "/":
                    i += 1
                    out.append("(?:.*/)?")  # zero or more directories
                else:
                    out.append(".*")
                continue
            out.append("[^/]*")
        elif c == "?":
            out.append("[^/]")
        elif c == "[":
            j = i + 1
            if j < n and pattern[j] in "^!":
                j += 1
            if j < n and pattern[j] == "]":
                j += 1
            while j < n and pattern[j] != "]":
                j += 2 if pattern[j] == "\\" and j + 1 < n else 1
            if j >= n:
                raise ValueError(f"syntax error in pattern: {pattern!r} (missing ']')")
            body = pattern[i + 1 : j]
            neg = body[:1] in ("^", "!")
            if neg:
                body = body[1:]
            out.append("[" + ("^" if neg else "") + body.replace("[", "\\[") + "]")
            i = j
        elif c == "\\" and i + 1 < n:
            i += 1
            out.append(re.escape(pattern[i]))
        else:
            out.append(re.escape(c))
        i += 1
    try:
        return re.compile("".join(out) + r"\Z", re.DOTALL)
    except re.error as e:  # a malformed class such as [z-a]
        raise ValueError(f"syntax error in pattern: {pattern!r} ({e})") from None


class _IgnoreMatcher:
    """`.dockerignore` semantics: patterns are relative to the context root, a
    leading `/` is ignored, a pattern that matches a directory matches
    everything under it, and a `!` pattern re-includes (the last pattern that
    matches decides)."""

    def __init__(self, patterns: List[str]):
        self._rules: List[Tuple["re.Pattern[str]", bool]] = []
        for raw in patterns:
            pattern = raw.strip()
            if not pattern or pattern.startswith("#"):
                continue
            negated = pattern.startswith("!")
            if negated:
                pattern = pattern[1:].strip()
            pattern = posixpath.normpath(pattern.lstrip("/")) if pattern.strip("/") else ""
            if pattern in ("", "."):
                continue
            self._rules.append((_pattern_regex(pattern), negated))

    def matches(self, rel: str) -> bool:
        parents = []
        head = rel
        while "/" in head:
            head = head.rsplit("/", 1)[0]
            parents.append(head)
        matched = False
        for regex, negated in self._rules:
            hit = regex.match(rel) is not None or any(regex.match(p) for p in parents)
            if hit:
                matched = not negated
        return matched

    def may_match_under(self, rel: str) -> bool:
        """Whether a re-including pattern could rescue something below `rel`
        (so an ignored directory still has to be walked)."""
        return any(negated for _, negated in self._rules)


def _normalize_ignore_pattern(pattern: str, context_path: str) -> str:
    """An absolute pattern that points into the context, made relative to
    it; other patterns are anchored at the context root."""
    pattern = pattern.strip()
    negated = pattern.startswith("!")
    if negated:
        pattern = pattern[1:].strip()
    if os.path.isabs(pattern):
        try:
            relative = os.path.relpath(pattern, context_path)
        except ValueError:
            relative = None
        if (
            relative is not None
            and relative != "."
            and relative != ".."
            and not relative.startswith(".." + os.sep)
        ):
            pattern = normalize_path(relative)
    return ("!" if negated else "") + pattern


# ── globbing the COPY source ───────────────────────────────────────────────


def _has_magic(segment: str) -> bool:
    return any(ch in segment for ch in "*?[")


def _glob(pattern: str, root: str) -> List[str]:
    """Paths (relative to `root`, `/`-separated) that match `pattern`:
    `*`, `?`, `[...]` within a segment, `**` across directories, dot files
    included."""
    segments = [s for s in normalize_path(pattern).split("/") if s not in ("", ".")]
    results: List[str] = []

    def children(rel: str):
        try:
            with os.scandir(os.path.join(root, rel) if rel else root) as it:
                return sorted(e.name for e in it)
        except OSError:
            return []

    def isdir(rel: str) -> bool:
        return os.path.isdir(os.path.join(root, rel)) and not os.path.islink(
            os.path.join(root, rel)
        )

    def walk(rel: str, idx: int) -> None:
        if idx == len(segments):
            if rel:  # the root itself is selected only by "." (below)
                results.append(rel)
            return
        seg = segments[idx]
        if seg == "**" and idx == len(segments) - 1:
            # A trailing ** selects the directory itself and everything below it.
            if rel:
                results.append(rel)
            for name in children(rel):
                sub = f"{rel}/{name}" if rel else name
                results.append(sub)
                if isdir(sub):
                    walk(sub, idx)
            return
        if seg == "**":
            walk(rel, idx + 1)
            for name in children(rel):
                sub = f"{rel}/{name}" if rel else name
                if isdir(sub):
                    walk(sub, idx)
            return
        if seg == "..":
            walk(posixpath.normpath(f"{rel}/..") if rel else "..", idx + 1)
            return
        if _has_magic(seg):
            for name in children(rel):
                if fnmatch.fnmatchcase(name, seg):
                    walk(f"{rel}/{name}" if rel else name, idx + 1)
        else:
            sub = f"{rel}/{seg}" if rel else seg
            if os.path.lexists(os.path.join(root, sub)):
                walk(sub, idx + 1)

    if not segments:
        return ["."]
    walk("", 0)
    return sorted(set(results))


def get_all_files_in_path(
    src: str,
    context_path: str,
    ignore_patterns: List[str],
    include_directories: bool = True,
) -> List[str]:
    """All files (and directories) that `src` selects below `context_path`,
    with the ignore patterns applied.

    Patterns follow `.dockerignore` semantics: relative to the context root,
    a pattern that matches a directory excludes everything under it, and `!`
    patterns re-include (the last matching pattern wins). The context root is
    never excluded. Symlinks are not followed: the link itself is copied.

    :return: sorted absolute paths
    """
    files = set()
    abs_context = os.path.abspath(context_path)
    try:
        matcher = _IgnoreMatcher(
            [_normalize_ignore_pattern(p, abs_context) for p in ignore_patterns]
        )
    except ValueError as e:
        raise TemplateException(str(e)) from e

    def walk(dir_path: str, dir_rel: str) -> None:
        with os.scandir(dir_path) as entries:
            for entry in entries:
                rel = entry.name if dir_rel == "." else f"{dir_rel}/{entry.name}"
                ignored = matcher.matches(rel)
                if not ignored:
                    files.add(entry.path)
                if entry.is_dir(follow_symlinks=False) and (
                    not ignored or matcher.may_match_under(rel)
                ):
                    walk(entry.path, rel)

    matches = []
    for hit in _glob(src, abs_context):
        file_path = os.path.normpath(os.path.join(abs_context, hit))
        rel = normalize_path(os.path.relpath(file_path, abs_context))
        matches.append((file_path, rel))

    # Parents before their children, so paths under a walked directory are
    # not walked again.
    matches.sort(key=lambda m: 0 if m[1] == "." else m[1].count("/") + 1)
    walked = set()

    def under_walked(rel: str) -> bool:
        if "." in walked:
            return True
        parent = posixpath.dirname(rel)
        while parent:
            if parent in walked:
                return True
            parent = posixpath.dirname(parent)
        return False

    for file_path, rel in matches:
        if under_walked(rel):
            continue
        if rel != "." and matcher.matches(rel):
            continue
        if os.path.isdir(file_path) and not os.path.islink(file_path):
            if include_directories:
                files.add(file_path)
            walk(file_path, rel)
            walked.add(rel)
        else:
            files.add(file_path)

    return sorted(files)


def calculate_files_hash(
    src: str,
    dest: str,
    context_path: str,
    ignore_patterns: List[str],
    resolve_symlinks: bool,
    stack_trace: Optional[TracebackType],
) -> str:
    """A hash of what a COPY copies: its text, every relative path, file
    mode, size and content (symlinks: their target). uid, gid and mtime are
    left out so the hash is the same on every machine.

    :raises ValueError: when no file matches `src`
    """
    src_path = os.path.join(context_path, src)
    digest = hashlib.sha256()
    digest.update(f"COPY {src} {dest}".encode())

    files = get_all_files_in_path(src, context_path, ignore_patterns, True)
    if len(files) == 0:
        raise ValueError(f"No files found in {src_path}").with_traceback(stack_trace)

    def hash_stats(info: os.stat_result) -> None:
        digest.update(str(info.st_mode).encode())
        # A directory's size belongs to the filesystem, not to the copied files.
        if not stat.S_ISDIR(info.st_mode):
            digest.update(str(info.st_size).encode())

    for file in files:
        digest.update(os.path.relpath(file, context_path).encode())
        if os.path.islink(file):
            follow = resolve_symlinks and (os.path.isfile(file) or os.path.isdir(file))
            if not follow:
                hash_stats(os.lstat(file))
                digest.update(os.readlink(file).encode())
                continue
        info = os.stat(file)
        hash_stats(info)
        if stat.S_ISREG(info.st_mode):
            with open(file, "rb") as f:
                digest.update(f.read())
    return digest.hexdigest()


def tar_file_stream(
    file_name: str,
    file_context_path: str,
    ignore_patterns: List[str],
    resolve_symlinks: bool,
    gzip: bool,
) -> IO[bytes]:
    """A tar archive (gzipped when `gzip`) of what `file_name` selects, in a
    temporary file positioned at its start; closing the file deletes it."""
    tar_file = tempfile.TemporaryFile()
    try:
        with tarfile.open(
            fileobj=tar_file, mode="w:gz" if gzip else "w", dereference=resolve_symlinks
        ) as tar:
            for file in get_all_files_in_path(file_name, file_context_path, ignore_patterns, True):
                tar.add(file, arcname=os.path.relpath(file, file_context_path), recursive=False)
        tar_file.seek(0)
        return tar_file
    except Exception:
        try:
            tar_file.close()
        except Exception:  # noqa: BLE001 - the archive error is the one to report
            pass
        raise


_ST = r"(?:\u0007|\u001B\|\u009C)"
_STRINGS = rf"(?:\u001B[\]PX^_][\s\S]*?{_ST})"
_CSI = r"[\u001B\u009B][\[\]()#;?]*(?:\d{1,4}(?:[;:]\d{0,4})*)?[\dA-PR-TZcf-nq-uy=><~]"
_ANSI = re.compile(f"{_STRINGS}|{_CSI}", re.ASCII)


def strip_ansi_escape_codes(text: str) -> str:
    """`text` without ANSI escape sequences (colours, cursor moves, OSC/DCS
    strings)."""
    return _ANSI.sub("", text)


# The package directory: the boundary between SDK frames and user frames.
_SDK_ROOT_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def _is_user_file(filename: str) -> bool:
    try:
        return os.path.commonpath([_SDK_ROOT_DIR, os.path.abspath(filename)]) != _SDK_ROOT_DIR
    except ValueError:  # another drive on Windows
        return True


def get_caller_frame() -> Optional[FrameType]:
    """The innermost frame outside the SDK: where user code called a template
    method, however many SDK frames sit in between."""
    frame = inspect.currentframe()
    while frame is not None:
        if _is_user_file(frame.f_code.co_filename):
            return frame
        frame = frame.f_back
    return None


def get_caller_directory() -> Optional[str]:
    """The directory of the file that called into the SDK: the default
    context for a template's relative file paths."""
    try:
        frame = get_caller_frame()
        if frame is None:
            return None
        return os.path.dirname(os.path.abspath(frame.f_code.co_filename))
    except Exception:  # noqa: BLE001
        return None


def pad_octal(mode: int) -> str:
    """A file mode as four octal digits: `0o755` is `"0755"`."""
    return f"{mode:04o}"


def get_build_step_index(step: str, stack_traces_length: int) -> int:
    """The index of a build step: `"base"` is 0, `"finalize"` the last one,
    a number is itself."""
    if step == BASE_STEP_NAME:
        return 0
    if step == FINALIZE_STEP_NAME:
        return stack_traces_length - 1
    return int(step)


def read_gcp_service_account_json(context_path: str, path_or_content: Union[str, dict]) -> str:
    """A GCP service account as JSON text: read from a file (relative to
    `context_path`) or serialized from a dict."""
    if isinstance(path_or_content, str):
        with open(os.path.join(context_path, path_or_content), "r", encoding="utf-8") as f:
            return f.read()
    return json.dumps(path_or_content)
