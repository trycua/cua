"""`sandbox.git`: e2b's git helper, as `git` commands in the sandbox (run
with `commands.run`, so failures raise `CommandExitException`)."""

from __future__ import annotations

import shlex
from dataclasses import dataclass, field
from typing import Dict, List, Literal, Optional
from urllib.parse import quote, urlsplit, urlunsplit

GitResetMode = Literal["soft", "mixed", "hard", "merge", "keep"]

_STATUS = {
    "M": "modified",
    "A": "added",
    "D": "deleted",
    "R": "renamed",
    "C": "copied",
    "T": "typechange",
    "U": "conflict",
    "?": "untracked",
}


@dataclass
class GitFileStatus:
    name: str
    status: str
    index_status: str
    working_tree_status: str
    staged: bool
    renamed_from: Optional[str] = None


@dataclass
class GitStatus:
    current_branch: Optional[str]
    upstream: Optional[str]
    ahead: int
    behind: int
    detached: bool
    file_status: List[GitFileStatus] = field(default_factory=list)

    @property
    def is_clean(self) -> bool:
        return not self.file_status

    @property
    def has_changes(self) -> bool:
        return bool(self.file_status)

    @property
    def has_staged(self) -> bool:
        return any(f.staged for f in self.file_status)

    @property
    def has_untracked(self) -> bool:
        return any(f.status == "untracked" for f in self.file_status)

    @property
    def has_conflicts(self) -> bool:
        return any(f.status == "conflict" for f in self.file_status)

    @property
    def total_count(self) -> int:
        return len(self.file_status)

    @property
    def staged_count(self) -> int:
        return sum(1 for f in self.file_status if f.staged)

    @property
    def unstaged_count(self) -> int:
        return sum(1 for f in self.file_status if f.working_tree_status not in (" ", "?", ""))

    @property
    def untracked_count(self) -> int:
        return sum(1 for f in self.file_status if f.status == "untracked")

    @property
    def conflict_count(self) -> int:
        return sum(1 for f in self.file_status if f.status == "conflict")


@dataclass
class GitBranches:
    branches: List[str]
    current_branch: Optional[str]


def parse_status(porcelain: str) -> GitStatus:
    """`git status --porcelain=v1 --branch` output."""
    st = GitStatus(None, None, 0, 0, False, [])
    for line in porcelain.splitlines():
        if line.startswith("## "):
            head = line[3:]
            if head.startswith("HEAD (no branch)"):
                st.detached = True
                continue
            if head.startswith("No commits yet on "):
                st.current_branch = head[len("No commits yet on ") :].strip()
                continue
            track = ""
            if " [" in head:
                head, track = head.split(" [", 1)
            branch, _, upstream = head.partition("...")
            st.current_branch = branch or None
            st.upstream = upstream or None
            for part in track.rstrip("]").split(","):
                part = part.strip()
                if part.startswith("ahead "):
                    st.ahead = int(part[6:])
                elif part.startswith("behind "):
                    st.behind = int(part[7:])
            continue
        if len(line) < 4:
            continue
        x, y, rest = line[0], line[1], line[3:]
        renamed_from = None
        if " -> " in rest:
            renamed_from, rest = rest.split(" -> ", 1)
        if "U" in (x, y) or (x, y) in (("A", "A"), ("D", "D")):
            status = "conflict"
        elif x == "?":
            status = "untracked"
        else:
            status = _STATUS.get(x if x != " " else y, "modified")
        st.file_status.append(
            GitFileStatus(
                name=rest,
                status=status,
                index_status=x,
                working_tree_status=y,
                staged=x not in (" ", "?"),
                renamed_from=renamed_from,
            )
        )
    return st


def _with_credentials(url: str, username: Optional[str], password: Optional[str]) -> str:
    if not username and not password:
        return url
    parts = urlsplit(url)
    if parts.scheme not in ("http", "https"):
        raise ValueError("git credentials need an http(s) URL")
    auth = quote(username or "", safe="") + (":" + quote(password, safe="") if password else "")
    return urlunsplit(
        (
            parts.scheme,
            f"{auth}@{parts.hostname}" + (f":{parts.port}" if parts.port else ""),
            parts.path,
            parts.query,
            parts.fragment,
        )
    )


class Git:
    """Git operations in the sandbox."""

    def __init__(self, commands):
        self._commands = commands

    async def _git(
        self,
        args: List[str],
        path=None,
        envs=None,
        user=None,
        cwd=None,
        timeout=None,
        request_timeout=None,
    ):
        cmd = shlex.join(["git", *(["-C", path] if path else []), *args])
        env = {"GIT_TERMINAL_PROMPT": "0", **(envs or {})}
        return await self._commands.run(
            cmd,
            envs=env,
            user=user,
            cwd=cwd,
            timeout=timeout if timeout is not None else 60,
            request_timeout=request_timeout,
        )

    async def clone(
        self,
        url: str,
        path: Optional[str] = None,
        branch: Optional[str] = None,
        depth: Optional[int] = None,
        username: Optional[str] = None,
        password: Optional[str] = None,
        envs: Optional[Dict[str, str]] = None,
        user=None,
        cwd=None,
        timeout=None,
        request_timeout=None,
        dangerously_store_credentials: bool = False,
    ):
        args = ["clone", _with_credentials(url, username, password)]
        if branch:
            args += ["--branch", branch, "--single-branch"]
        if depth:
            args += ["--depth", str(depth)]
        if path:
            args.append(path)
        r = await self._git(args, None, envs, user, cwd, timeout, request_timeout)
        if (username or password) and not dangerously_store_credentials:
            repo = path or url.rstrip("/").rsplit("/", 1)[-1].removesuffix(".git")
            await self._git(
                ["remote", "set-url", "origin", url],
                repo,
                envs,
                user,
                cwd,
                timeout,
                request_timeout,
            )
        return r

    async def init(
        self,
        path: str,
        bare: bool = False,
        initial_branch: Optional[str] = None,
        envs=None,
        user=None,
        cwd=None,
        timeout=None,
        request_timeout=None,
    ):
        args = [
            "init",
            *(["--bare"] if bare else []),
            *(["--initial-branch", initial_branch] if initial_branch else []),
            path,
        ]
        return await self._git(args, None, envs, user, cwd, timeout, request_timeout)

    async def remote_add(
        self,
        path: str,
        name: str,
        url: str,
        fetch: bool = False,
        overwrite: bool = False,
        envs=None,
        user=None,
        cwd=None,
        timeout=None,
        request_timeout=None,
    ):
        if (
            overwrite
            and await self.remote_get(path, name, envs, user, cwd, timeout, request_timeout)
            is not None
        ):
            r = await self._git(
                ["remote", "set-url", name, url], path, envs, user, cwd, timeout, request_timeout
            )
        else:
            r = await self._git(
                ["remote", "add", name, url], path, envs, user, cwd, timeout, request_timeout
            )
        if fetch:
            r = await self._git(["fetch", name], path, envs, user, cwd, timeout, request_timeout)
        return r

    async def remote_get(
        self,
        path: str,
        name: str,
        envs=None,
        user=None,
        cwd=None,
        timeout=None,
        request_timeout=None,
    ) -> Optional[str]:
        r = await self._commands.run(
            shlex.join(["git", "-C", path, "remote", "get-url", name]) + " 2>/dev/null || true",
            envs=envs,
            user=user,
            cwd=cwd,
            timeout=timeout if timeout is not None else 60,
            request_timeout=request_timeout,
        )
        return r.stdout.strip() or None

    async def status(
        self, path: str, envs=None, user=None, cwd=None, timeout=None, request_timeout=None
    ) -> GitStatus:
        r = await self._git(
            ["status", "--porcelain=v1", "--branch"],
            path,
            envs,
            user,
            cwd,
            timeout,
            request_timeout,
        )
        return parse_status(r.stdout)

    async def branches(
        self, path: str, envs=None, user=None, cwd=None, timeout=None, request_timeout=None
    ) -> GitBranches:
        r = await self._git(
            ["branch", "--format=%(HEAD) %(refname:short)"],
            path,
            envs,
            user,
            cwd,
            timeout,
            request_timeout,
        )
        names, current = [], None
        for line in r.stdout.splitlines():
            if not line.strip():
                continue
            name = line[2:].strip()
            if name.startswith("("):  # detached HEAD
                continue
            names.append(name)
            if line.startswith("*"):
                current = name
        return GitBranches(branches=names, current_branch=current)

    async def create_branch(
        self,
        path: str,
        branch: str,
        envs=None,
        user=None,
        cwd=None,
        timeout=None,
        request_timeout=None,
    ):
        return await self._git(
            ["checkout", "-b", branch], path, envs, user, cwd, timeout, request_timeout
        )

    async def checkout_branch(
        self,
        path: str,
        branch: str,
        envs=None,
        user=None,
        cwd=None,
        timeout=None,
        request_timeout=None,
    ):
        return await self._git(
            ["checkout", branch], path, envs, user, cwd, timeout, request_timeout
        )

    async def delete_branch(
        self,
        path: str,
        branch: str,
        force: bool = False,
        envs=None,
        user=None,
        cwd=None,
        timeout=None,
        request_timeout=None,
    ):
        return await self._git(
            ["branch", "-D" if force else "-d", branch],
            path,
            envs,
            user,
            cwd,
            timeout,
            request_timeout,
        )

    async def add(
        self,
        path: str,
        files: Optional[List[str]] = None,
        all: bool = True,
        envs=None,
        user=None,
        cwd=None,
        timeout=None,
        request_timeout=None,
    ):
        args = ["add", *(["--", *files] if files else (["-A"] if all else ["."]))]
        return await self._git(args, path, envs, user, cwd, timeout, request_timeout)

    async def commit(
        self,
        path: str,
        message: str,
        author_name: Optional[str] = None,
        author_email: Optional[str] = None,
        allow_empty: bool = False,
        envs=None,
        user=None,
        cwd=None,
        timeout=None,
        request_timeout=None,
    ):
        args = ["commit", "-m", message, *(["--allow-empty"] if allow_empty else [])]
        if author_name or author_email:
            args += ["--author", f"{author_name or ''} <{author_email or ''}>"]
        return await self._git(args, path, envs, user, cwd, timeout, request_timeout)

    async def reset(
        self,
        path: str,
        mode: Optional[GitResetMode] = None,
        target: Optional[str] = None,
        paths: Optional[List[str]] = None,
        envs=None,
        user=None,
        cwd=None,
        timeout=None,
        request_timeout=None,
    ):
        args = [
            "reset",
            *([f"--{mode}"] if mode else []),
            *([target] if target else []),
            *(["--", *paths] if paths else []),
        ]
        return await self._git(args, path, envs, user, cwd, timeout, request_timeout)

    async def restore(
        self,
        path: str,
        paths: List[str],
        staged: Optional[bool] = None,
        worktree: Optional[bool] = None,
        source: Optional[str] = None,
        envs=None,
        user=None,
        cwd=None,
        timeout=None,
        request_timeout=None,
    ):
        args = [
            "restore",
            *(["--staged"] if staged else []),
            *(["--worktree"] if worktree else []),
            *([f"--source={source}"] if source else []),
            "--",
            *paths,
        ]
        return await self._git(args, path, envs, user, cwd, timeout, request_timeout)

    async def _with_remote_auth(
        self, op, path, remote, username, password, envs, user, cwd, timeout, request_timeout, extra
    ):
        remote = remote or "origin"
        if not (username or password):
            return await self._git(
                [op, remote, *extra], path, envs, user, cwd, timeout, request_timeout
            )
        url = await self.remote_get(path, remote, envs, user, cwd, timeout, request_timeout)
        if not url:
            raise ValueError(f"remote {remote!r} not found")
        return await self._git(
            [op, _with_credentials(url, username, password), *extra],
            path,
            envs,
            user,
            cwd,
            timeout,
            request_timeout,
        )

    async def push(
        self,
        path: str,
        remote: Optional[str] = None,
        branch: Optional[str] = None,
        set_upstream: bool = True,
        username=None,
        password=None,
        envs=None,
        user=None,
        cwd=None,
        timeout=None,
        request_timeout=None,
    ):
        extra = [*([branch] if branch else [])]
        if set_upstream and branch and not (username or password):
            return await self._git(
                ["push", "-u", remote or "origin", branch],
                path,
                envs,
                user,
                cwd,
                timeout,
                request_timeout,
            )
        return await self._with_remote_auth(
            "push",
            path,
            remote,
            username,
            password,
            envs,
            user,
            cwd,
            timeout,
            request_timeout,
            extra,
        )

    async def pull(
        self,
        path: str,
        remote: Optional[str] = None,
        branch: Optional[str] = None,
        username=None,
        password=None,
        envs=None,
        user=None,
        cwd=None,
        timeout=None,
        request_timeout=None,
    ):
        extra = [*([branch] if branch else [])]
        if not remote and not branch and not (username or password):
            return await self._git(["pull"], path, envs, user, cwd, timeout, request_timeout)
        return await self._with_remote_auth(
            "pull",
            path,
            remote,
            username,
            password,
            envs,
            user,
            cwd,
            timeout,
            request_timeout,
            extra,
        )

    def _scope(self, scope: str, path: Optional[str]):
        return (
            ["--local"] if scope == "local" else ["--system"] if scope == "system" else ["--global"]
        )

    async def set_config(
        self,
        key: str,
        value: str,
        scope: str = "global",
        path: Optional[str] = None,
        envs=None,
        user=None,
        cwd=None,
        timeout=None,
        request_timeout=None,
    ):
        return await self._git(
            ["config", *self._scope(scope, path), key, value],
            path if scope == "local" else None,
            envs,
            user,
            cwd,
            timeout,
            request_timeout,
        )

    async def get_config(
        self,
        key: str,
        scope: str = "global",
        path: Optional[str] = None,
        envs=None,
        user=None,
        cwd=None,
        timeout=None,
        request_timeout=None,
    ) -> Optional[str]:
        loc = ["-C", path] if (path and scope == "local") else []
        r = await self._commands.run(
            shlex.join(["git", *loc, "config", *self._scope(scope, path), "--get", key])
            + " || true",
            envs=envs,
            user=user,
            cwd=cwd,
            timeout=timeout if timeout is not None else 60,
            request_timeout=request_timeout,
        )
        return r.stdout.strip() or None

    async def dangerously_authenticate(
        self,
        username: str,
        password: str,
        host: str = "github.com",
        protocol: str = "https",
        envs=None,
        user=None,
        cwd=None,
        timeout=None,
        request_timeout=None,
    ):
        await self.set_config(
            "credential.helper", "store", "global", None, envs, user, cwd, timeout, request_timeout
        )
        line = f"protocol={protocol}\nhost={host}\nusername={username}\npassword={password}\n\n"
        return await self._commands.run(
            "git credential approve <<'CUA_EOF'\n" + line + "CUA_EOF",
            envs=envs,
            user=user,
            cwd=cwd,
            timeout=timeout if timeout is not None else 60,
            request_timeout=request_timeout,
        )

    async def configure_user(
        self,
        name: str,
        email: str,
        scope: str = "global",
        path: Optional[str] = None,
        envs=None,
        user=None,
        cwd=None,
        timeout=None,
        request_timeout=None,
    ):
        await self.set_config(
            "user.name", name, scope, path, envs, user, cwd, timeout, request_timeout
        )
        await self.set_config(
            "user.email", email, scope, path, envs, user, cwd, timeout, request_timeout
        )
