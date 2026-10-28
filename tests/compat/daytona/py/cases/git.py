"""Git toolbox: `sandbox.git.*` against a real git in the guest.

Remotes are bare repositories created in the guest (plus a smart-HTTP git server
with basic auth for the credential cases), so no internet is needed. Expected
behaviour is derived from the SDK: request fields and defaults in
daytona/_sync/git.py, models in daytona_toolbox_api_client/models/git_*.py,
exception classes in daytona/common/errors.py (CODE_TO_ERROR).
"""

from harness import case, err_obs

REF = "daytona/_sync/git.py; common/errors.py CODE_TO_ERROR"

# A smart-HTTP git server that demands basic auth user:secret (GET /info/refs
# answers 401 first, like a hosted git service).
HTTPD = r"""
import base64, http.server, os, subprocess, sys
ROOT, PORT = sys.argv[1], int(sys.argv[2])
WANT = "Basic " + base64.b64encode(b"user:secret").decode()
BACKEND = subprocess.check_output(["git", "--exec-path"], text=True).strip() + "/git-http-backend"
class H(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.0"
    def log_message(self, *a): pass
    def serve(self):
        if self.headers.get("Authorization") != WANT:
            self.send_response(401); self.send_header("WWW-Authenticate", 'Basic realm="git"'); self.end_headers(); return
        path, _, q = self.path.partition("?")
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n) if n else b""
        env = {"GIT_PROJECT_ROOT": ROOT, "GIT_HTTP_EXPORT_ALL": "1", "REQUEST_METHOD": self.command, "PATH_INFO": path,
               "QUERY_STRING": q, "CONTENT_TYPE": self.headers.get("Content-Type", ""), "CONTENT_LENGTH": str(len(body)),
               "REMOTE_USER": "user", "REMOTE_ADDR": "127.0.0.1", "PATH": os.environ["PATH"], "HOME": os.environ.get("HOME", "/tmp"),
               "HTTP_CONTENT_ENCODING": self.headers.get("Content-Encoding", ""), "GIT_COMMITTER_NAME": "srv", "GIT_COMMITTER_EMAIL": "srv@x"}
        out = subprocess.run([BACKEND], input=body, env=env, capture_output=True).stdout
        head, _, rest = out.partition(b"\r\n\r\n")
        status, headers = 200, []
        for line in head.decode().split("\r\n"):
            k, _, v = line.partition(": ")
            if k.lower() == "status": status = int(v.split()[0])
            elif k: headers.append((k, v))
        self.send_response(status)
        for k, v in headers: self.send_header(k, v)
        self.end_headers(); self.wfile.write(rest)
    do_GET = do_POST = serve
http.server.ThreadingHTTPServer(("127.0.0.1", PORT), H).serve_forever()
"""

AUTHOR = ("Ann Author", "ann@example.test")


def norm(st):
    def v(x):
        return str(getattr(x, "value", x))

    return {
        "branch": st.current_branch,
        "detached": bool(st.detached),
        "ahead": st.ahead,
        "behind": st.behind,
        "published": bool(st.branch_published),
        "upstream": st.upstream or "",
        "files": sorted([f.name, v(f.staging), v(f.worktree), f.extra] for f in st.file_status),
    }


async def sh(c, sb, cmd, ok=True):
    r = await c.w(sb.process.exec(cmd))
    if ok:
        assert r.exit_code == 0, (cmd, r.exit_code, r.result)
    return r.result.strip()


async def write(c, sb, path, text):
    await c.w(sb.fs.upload_file(text.encode(), path))


async def commit(c, sb, repo, msg, files=(".",)):
    await c.w(sb.git.add(repo, list(files)))
    r = await c.w(sb.git.commit(repo, msg, *AUTHOR))
    assert len(r.sha) == 40, r
    return r.sha


async def base(c, sb, name):
    d = f"/tmp/gt-{c.run_id}-{name}"
    await sh(c, sb, f"rm -rf {d} && mkdir -p {d}")
    return d


async def seeded(c, sb, name):
    """A bare remote with one commit on main, and a working clone `a` of it."""
    d = await base(c, sb, name)
    remote, a = f"{d}/remote.git", f"{d}/a"
    await c.w(sb.git.init(remote, bare=True, initial_branch="main"))
    await c.w(sb.git.clone(remote, a))
    await c.w(sb.git.configure_user(*AUTHOR, scope="local", path=a))
    await write(c, sb, f"{a}/f.txt", "one\n")
    await commit(c, sb, a, "first")
    await c.w(sb.git.push(a, set_upstream=True))
    return d, remote, a


@case("git.init_branches", ref=REF)
async def init_branches(c):
    sb = await c.main()
    d = await base(c, sb, "init")
    await c.w(sb.git.init(f"{d}/r", initial_branch="trunk"))
    empty = await c.w(sb.git.branches(f"{d}/r"))
    await c.w(sb.git.configure_user(*AUTHOR, scope="local", path=f"{d}/r"))
    await write(c, sb, f"{d}/r/a.txt", "a\n")
    await commit(c, sb, f"{d}/r", "init")
    await c.w(sb.git.create_branch(f"{d}/r", "feature/x"))
    after = await c.w(sb.git.branches(f"{d}/r"))
    bare = await c.w(sb.git.init(f"{d}/bare.git", bare=True))
    is_bare = await sh(c, sb, f"git -C {d}/bare.git rev-parse --is-bare-repository")
    return {
        "empty": [sorted(empty.branches), empty.current],
        "after": [sorted(after.branches), after.current],
        "bare": is_bare,
    }


@case("git.commit_status_flow", ref=REF)
async def commit_status_flow(c):
    sb = await c.main()
    d = await base(c, sb, "flow")
    r = f"{d}/r"
    await c.w(sb.git.init(r, initial_branch="main"))
    await c.w(sb.git.configure_user(*AUTHOR, scope="local", path=r))
    await write(c, sb, f"{r}/a.txt", "a\n")
    await write(c, sb, f"{r}/b b.txt", "b\n")
    s1 = norm(await c.w(sb.git.status(r)))
    await c.w(sb.git.add(r, ["a.txt"]))
    s2 = norm(await c.w(sb.git.status(r)))
    sha = await commit(c, sb, r, "first")
    await write(c, sb, f"{r}/a.txt", "a2\n")
    s3 = norm(await c.w(sb.git.status(r)))
    empty = await c.w(sb.git.commit(r, "empty", *AUTHOR, allow_empty=True))
    log = await sh(c, sb, f"git -C {r} log --format='%an|%ae|%cn|%s' -n 2")
    head = await sh(c, sb, f"git -C {r} rev-parse HEAD~1")
    nothing = err_obs(await c.raises(sb.git.commit, r, "nothing", *AUTHOR))
    return {
        "untracked": s1,
        "staged": s2,
        "modified": s3,
        "sha_is_head": head == sha,
        "empty_len": len(empty.sha),
        "log": log.split("\n"),
        "nothing_to_commit": nothing,
    }


@case("git.branch_lifecycle", ref=REF)
async def branch_lifecycle(c):
    sb = await c.main()
    d, remote, a = await seeded(c, sb, "br")
    await c.w(sb.git.create_branch(a, "dev"))
    exists = err_obs(await c.raises(sb.git.create_branch, a, "dev"))
    await c.w(sb.git.checkout_branch(a, "dev"))
    on_dev = (await c.w(sb.git.branches(a))).current
    missing = err_obs(await c.raises(sb.git.checkout_branch, a, "nope"))
    await c.w(sb.git.checkout_branch(a, "main"))
    await c.w(sb.git.delete_branch(a, "dev"))
    gone = err_obs(await c.raises(sb.git.delete_branch, a, "dev"))
    final = sorted((await c.w(sb.git.branches(a))).branches)
    return {"exists": exists, "on_dev": on_dev, "missing": missing, "gone": gone, "final": final}


@case("git.push_pull_roundtrip", ref=REF)
async def push_pull_roundtrip(c):
    sb = await c.main()
    d, remote, a = await seeded(c, sb, "pp")
    b = f"{d}/b"
    await c.w(sb.git.clone(remote, b))
    await c.w(sb.git.configure_user("Bo", "bo@example.test", scope="local", path=b))
    s0 = norm(await c.w(sb.git.status(b)))
    await write(c, sb, f"{a}/g.txt", "g\n")
    await commit(c, sb, a, "second")
    ahead = norm(await c.w(sb.git.status(a)))
    await c.w(sb.git.push(a))
    pushed = norm(await c.w(sb.git.status(a)))
    await sh(c, sb, f"git -C {b} fetch --quiet origin")
    behind = norm(await c.w(sb.git.status(b)))
    await c.w(sb.git.pull(b))
    pulled = await c.w(sb.fs.download_file(f"{b}/g.txt"))
    again = norm(await c.w(sb.git.status(b)))
    # A new branch is not published until it is pushed.
    await c.w(sb.git.create_branch(a, "topic"))
    await c.w(sb.git.checkout_branch(a, "topic"))
    unpub = norm(await c.w(sb.git.status(a)))
    await c.w(sb.git.push(a, branch="topic", set_upstream=True))
    pub = norm(await c.w(sb.git.status(a)))
    return {
        "clone": s0,
        "ahead": ahead,
        "pushed": pushed,
        "behind": behind,
        "pulled": pulled.decode(),
        "after_pull": again,
        "unpublished": unpub,
        "published": pub,
    }


@case("git.push_rejected_and_conflict", ref=REF)
async def push_rejected_and_conflict(c):
    sb = await c.main()
    d, remote, a = await seeded(c, sb, "rej")
    b = f"{d}/b"
    await c.w(sb.git.clone(remote, b))
    await c.w(sb.git.configure_user("Bo", "bo@example.test", scope="local", path=b))
    await write(c, sb, f"{a}/f.txt", "from a\n")
    await commit(c, sb, a, "a edits")
    await c.w(sb.git.push(a))
    await write(c, sb, f"{b}/f.txt", "from b\n")
    await commit(c, sb, b, "b edits")
    rejected = err_obs(await c.raises(sb.git.push, b))
    conflict = err_obs(await c.raises(sb.git.pull, b))
    st = norm(await c.w(sb.git.status(b)))
    return {
        "rejected": rejected,
        "conflict": conflict,
        "conflicted": [f for f in st["files"] if f[1] == "Updated but unmerged"],
    }


@case("git.dirty_checkout", ref=REF)
async def dirty_checkout(c):
    sb = await c.main()
    d, remote, a = await seeded(c, sb, "dirty")
    await c.w(sb.git.create_branch(a, "other"))
    await c.w(sb.git.checkout_branch(a, "other"))
    await write(c, sb, f"{a}/f.txt", "other\n")
    await commit(c, sb, a, "other edit")
    await c.w(sb.git.checkout_branch(a, "main"))
    await c.w(sb.git.checkout_branch(a, "other"))
    await c.w(sb.git.checkout_branch(a, "main"))
    await write(c, sb, f"{a}/f.txt", "dirty\n")
    dirty = err_obs(await c.raises(sb.git.checkout_branch, a, "other"))
    await c.w(sb.git.restore(a, ["f.txt"]))
    restored = await c.w(sb.fs.download_file(f"{a}/f.txt"))
    return {"dirty": dirty, "restored": restored.decode()}


@case("git.repo_errors", ref=REF)
async def repo_errors(c):
    sb = await c.main()
    d, remote, a = await seeded(c, sb, "err")
    plain = await base(c, sb, "err2")
    return {
        "status_not_repo": err_obs(await c.raises(sb.git.status, plain)),
        "branches_not_repo": err_obs(await c.raises(sb.git.branches, f"{plain}/nope")),
        "clone_missing_repo": err_obs(
            await c.raises(sb.git.clone, f"{plain}/missing.git", f"{plain}/c1")
        ),
        "clone_missing_branch": err_obs(
            await c.raises(sb.git.clone, remote, f"{plain}/c2", branch="nope")
        ),
        "clone_ok_branch": (await c.w(sb.git.clone(remote, f"{plain}/c3", branch="main"))) is None,
        "add_nothing_matches": err_obs(await c.raises(sb.git.add, a, ["does-not-exist"])),
        "reset_bad_mode": (await c.raises(sb.git.reset, a, mode="bogus")) is not None,
        "unreachable": err_obs(
            await c.raises(sb.git.clone, "http://127.0.0.1:9/x.git", f"{plain}/c4")
        ),
    }


@case("git.reset_restore", ref=REF)
async def reset_restore(c):
    sb = await c.main()
    d, remote, a = await seeded(c, sb, "rr")
    await write(c, sb, f"{a}/f.txt", "two\n")
    sha2 = await commit(c, sb, a, "second")
    await write(c, sb, f"{a}/f.txt", "three\n")
    await c.w(sb.git.add(a, ["f.txt"]))
    staged = norm(await c.w(sb.git.status(a)))["files"]
    await c.w(sb.git.restore(a, ["f.txt"], staged=True))
    unstaged = norm(await c.w(sb.git.status(a)))["files"]
    await c.w(sb.git.restore(a, ["f.txt"], source="HEAD~1"))
    from_old = (await c.w(sb.fs.download_file(f"{a}/f.txt"))).decode()
    await c.w(sb.git.add(a, ["f.txt"]))
    await c.w(sb.git.reset(a))
    mixed = norm(await c.w(sb.git.status(a)))["files"]
    await c.w(sb.git.reset(a, mode="soft", target="HEAD~1"))
    soft_head = await sh(c, sb, f"git -C {a} log --format=%s -n 1")
    await c.w(sb.git.reset(a, mode="hard", target=sha2))
    hard = (await c.w(sb.fs.download_file(f"{a}/f.txt"))).decode()
    await write(c, sb, f"{a}/f.txt", "scratch\n")
    await c.w(sb.git.reset(a, files=["f.txt"]))
    return {
        "staged": staged,
        "unstaged": unstaged,
        "from_old": from_old,
        "mixed": mixed,
        "soft_head": soft_head,
        "hard": hard,
    }


@case("git.remotes", ref=REF)
async def remotes(c):
    sb = await c.main()
    d, remote, a = await seeded(c, sb, "rem")
    other = await base(c, sb, "rem2")
    await c.w(sb.git.init(f"{other}/o.git", bare=True))
    first = [(r.name, r.url == remote) for r in (await c.w(sb.git.remotes(a))).remotes]
    await c.w(sb.git.remote_add(a, "backup", f"{other}/o.git"))
    dup = err_obs(await c.raises(sb.git.remote_add, a, "backup", f"{other}/o.git"))
    await c.w(sb.git.remote_add(a, "backup", remote, overwrite=True))
    got = await c.w(sb.git.remote_get(a, "backup"))
    missing = await c.w(sb.git.remote_get(a, "nope"))
    await c.w(sb.git.remote_add(a, "fetched", remote, fetch=True))
    names = sorted(r.name for r in (await c.w(sb.git.remotes(a))).remotes)
    await c.w(sb.git.push(a, remote="backup", branch="main"))
    return {
        "first": first,
        "dup": dup,
        "overwritten": got == remote,
        "missing": missing,
        "names": names,
    }


@case("git.config", ref=REF)
async def config(c):
    sb = await c.main()
    d, remote, a = await seeded(c, sb, "cfg")
    await c.w(sb.git.set_config("cua.test", "local-value", scope="local", path=a))
    local = await c.w(sb.git.get_config("cua.test", scope="local", path=a))
    missing = await c.w(sb.git.get_config("cua.nothing", scope="local", path=a))
    await c.w(sb.git.configure_user("Zed", "zed@example.test", scope="local", path=a))
    user = [
        await c.w(sb.git.get_config("user.name", scope="local", path=a)),
        await c.w(sb.git.get_config("user.email", scope="local", path=a)),
    ]
    await c.w(sb.git.set_config(f"cua.g{c.run_id}", "global-value"))
    glob = await c.w(sb.git.get_config(f"cua.g{c.run_id}"))
    await sh(c, sb, f"git config --global --unset cua.g{c.run_id}")
    bad_scope = err_obs(await c.raises(sb.git.get_config, "x.y", scope="worktree"))
    no_path = err_obs(await c.raises(sb.git.set_config, "x.y", "v", scope="local"))
    return {
        "local": local,
        "missing": missing,
        "user": user,
        "global": glob,
        "bad_scope": bad_scope,
        "no_path": no_path,
    }


@case("git.clone_depth_commit", ref=REF)
async def clone_depth_commit(c):
    sb = await c.main()
    d, remote, a = await seeded(c, sb, "depth")
    first = await sh(c, sb, f"git -C {a} rev-parse HEAD")
    for i in range(2):
        await write(c, sb, f"{a}/f.txt", f"v{i}\n")
        await commit(c, sb, a, f"c{i}")
    await c.w(sb.git.push(a))
    shallow = f"{d}/shallow"
    await c.w(sb.git.clone(f"file://{remote}", shallow, depth=1))
    count = await sh(c, sb, f"git -C {shallow} rev-list --count HEAD")
    pinned = f"{d}/pinned"
    await c.w(sb.git.clone(remote, pinned, commit_id=first))
    st = norm(await c.w(sb.git.status(pinned)))
    content = (await c.w(sb.fs.download_file(f"{pinned}/f.txt"))).decode()
    bad = err_obs(await c.raises(sb.git.clone, remote, f"{d}/badc", commit_id="0" * 40))
    left = await sh(c, sb, f"test -e {d}/badc && echo yes || echo no")
    return {
        "shallow_commits": count,
        "detached": st["detached"],
        "pinned": content,
        "bad_commit": bad,
        "bad_commit_left": left,
    }


@case("git.http_auth", ref=REF + "; GitCloneRequest username/password; dangerously_authenticate")
async def http_auth(c):
    sb = await c.main()
    d = await base(c, sb, "auth")
    srv = f"{d}/srv"
    await sh(c, sb, f"mkdir -p {srv}")
    await c.w(sb.git.init(f"{srv}/r.git", bare=True, initial_branch="main"))
    await sh(c, sb, f"git -C {srv}/r.git config http.receivepack true")
    await write(c, sb, f"{d}/httpd.py", HTTPD)
    port = 20000 + int(c.run_id) % 20000
    await sh(
        c,
        sb,
        f"nohup python3 {d}/httpd.py {srv} {port} >{d}/httpd.log 2>&1 & echo $! >{d}/httpd.pid",
    )
    await sh(
        c,
        sb,
        "python3 -c \"import socket,time,sys\nfor _ in range(50):\n try: socket.create_connection(('127.0.0.1', %d), 1).close(); sys.exit(0)\n except OSError: time.sleep(.2)\nsys.exit(1)\""
        % port,
    )
    url = f"http://127.0.0.1:{port}/r.git"
    try:
        no_creds = err_obs(await c.raises(sb.git.clone, url, f"{d}/c0"))
        wrong = await c.raises(
            sb.git.clone, url, f"{d}/c1", username="user", password="wrong-password-xyz"
        )
        leaked = wrong is not None and "wrong-password-xyz" in wrong["message"]
        await c.w(sb.git.clone(url, f"{d}/c2", username="user", password="secret"))
        await c.w(sb.git.configure_user(*AUTHOR, scope="local", path=f"{d}/c2"))
        await write(c, sb, f"{d}/c2/x.txt", "x\n")
        await commit(c, sb, f"{d}/c2", "x")
        push_no = err_obs(await c.raises(sb.git.push, f"{d}/c2"))
        await c.w(sb.git.push(f"{d}/c2", username="user", password="secret"))
        # Nothing keeps the password: not in the repo config, not in the process list.
        stored = await sh(c, sb, f"grep -rl secret {d}/c2/.git/config; true")
        await c.w(
            sb.git.dangerously_authenticate(
                "user", "secret", host=f"127.0.0.1:{port}", protocol="http"
            )
        )
        await c.w(sb.git.clone(url, f"{d}/c3"))
        helper = await sh(c, sb, "git config --global credential.helper")
        await sh(c, sb, "rm -f ~/.git-credentials; git config --global --unset credential.helper")
        return {
            "no_creds": no_creds,
            "wrong_password": err_obs(wrong),
            "wrong_leaked": leaked,
            "push_no_creds": push_no,
            "config_has_password": stored,
            "helper": helper,
        }
    finally:
        await sh(c, sb, f"[ -f {d}/httpd.pid ] && kill $(cat {d}/httpd.pid); true")


@case(
    "git.history_route",
    ref="daytona_toolbox_api_client/api/git_api.py get_history (not exposed by the SDK classes)",
)
async def history_route(c):
    import json
    import os
    import urllib.request

    sb = await c.main()
    d, remote, a = await seeded(c, sb, "hist")
    url = f"{os.environ['DAYTONA_API_URL']}/toolbox/{sb.id}/git/history?path={a}"
    req = urllib.request.Request(
        url, headers={"Authorization": f"Bearer {os.environ['DAYTONA_API_KEY']}"}
    )
    with urllib.request.urlopen(req, timeout=30) as r:
        items = json.loads(r.read())
    return {
        "count": len(items),
        "first": [items[0]["author"], items[0]["email"], items[0]["message"]],
        "hash_len": len(items[0]["hash"]),
        "keys": sorted(items[0]),
    }
