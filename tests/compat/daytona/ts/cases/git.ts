import { assert, errObs, testCase, type Ctx } from '../harness.js'

// Derived from the SDK: Git.js (request fields), common/errors (CODE_TO_ERROR). See py/cases/git.py.
const HTTPD = String.raw`import base64, http.server, os, subprocess, sys
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
`
const AUTHOR: [string, string] = ['Ann Author', 'ann@example.test']

const norm = (st: any) => ({
  branch: st.currentBranch,
  detached: !!st.detached,
  ahead: st.ahead,
  behind: st.behind,
  published: !!st.branchPublished,
  upstream: st.upstream || '',
  files: st.fileStatus.map((f: any) => [f.name, String(f.staging), String(f.worktree), f.extra]).sort((a: any, b: any) => JSON.stringify(a).localeCompare(JSON.stringify(b))),
})
// Python sorts lists of lists lexicographically by element; do the same.
const lex = (a: any[], b: any[]): number => {
  for (let i = 0; i < Math.min(a.length, b.length); i++) {
    if (a[i] < b[i]) return -1
    if (a[i] > b[i]) return 1
  }
  return a.length - b.length
}
const normS = (st: any) => {
  const n = norm(st)
  n.files.sort(lex)
  return n
}

async function sh(sb: any, cmd: string, ok = true): Promise<string> {
  const r = await sb.process.executeCommand(cmd)
  if (ok) assert(r.exitCode === 0, `${cmd}: ${r.exitCode} ${r.result}`)
  return r.result.trim()
}
const write = (sb: any, path: string, text: string) => sb.fs.uploadFile(Buffer.from(text), path)
async function commit(sb: any, repo: string, msg: string, files: string[] = ['.']) {
  await sb.git.add(repo, files)
  const r = await sb.git.commit(repo, msg, ...AUTHOR)
  assert(r.sha.length === 40, 'sha')
  return r.sha as string
}
async function base(c: Ctx, sb: any, name: string) {
  const d = `/tmp/gt-${c.runId}-${name}`
  await sh(sb, `rm -rf ${d} && mkdir -p ${d}`)
  return d
}
async function seeded(c: Ctx, sb: any, name: string) {
  const d = await base(c, sb, name)
  const remote = `${d}/remote.git`
  const a = `${d}/a`
  await sb.git.init(remote, true, 'main')
  await sb.git.clone(remote, a)
  await sb.git.configureUser(...AUTHOR, 'local', a)
  await write(sb, `${a}/f.txt`, 'one\n')
  await commit(sb, a, 'first')
  await sb.git.push(a, undefined, undefined, undefined, undefined, true)
  return { d, remote, a }
}
const e = async (c: Ctx, fn: () => Promise<unknown>) => errObs(await c.raises(fn))
const text = async (sb: any, p: string) => (await sb.fs.downloadFile(p)).toString()

testCase('git.init_branches', 'space', async (c) => {
  const sb = await c.main()
  const d = await base(c, sb, 'init')
  await sb.git.init(`${d}/r`, false, 'trunk')
  const empty = await sb.git.branches(`${d}/r`)
  await sb.git.configureUser(...AUTHOR, 'local', `${d}/r`)
  await write(sb, `${d}/r/a.txt`, 'a\n')
  await commit(sb, `${d}/r`, 'init')
  await sb.git.createBranch(`${d}/r`, 'feature/x')
  const after = await sb.git.branches(`${d}/r`)
  await sb.git.init(`${d}/bare.git`, true)
  const bare = await sh(sb, `git -C ${d}/bare.git rev-parse --is-bare-repository`)
  return { empty: [[...empty.branches].sort(), empty.current], after: [[...after.branches].sort(), after.current], bare }
})

testCase('git.commit_status_flow', 'space', async (c) => {
  const sb = await c.main()
  const d = await base(c, sb, 'flow')
  const r = `${d}/r`
  await sb.git.init(r, false, 'main')
  await sb.git.configureUser(...AUTHOR, 'local', r)
  await write(sb, `${r}/a.txt`, 'a\n')
  await write(sb, `${r}/b b.txt`, 'b\n')
  const s1 = normS(await sb.git.status(r))
  await sb.git.add(r, ['a.txt'])
  const s2 = normS(await sb.git.status(r))
  const sha = await commit(sb, r, 'first')
  await write(sb, `${r}/a.txt`, 'a2\n')
  const s3 = normS(await sb.git.status(r))
  const empty = await sb.git.commit(r, 'empty', ...AUTHOR, true)
  const log = await sh(sb, `git -C ${r} log --format='%an|%ae|%cn|%s' -n 2`)
  const head = await sh(sb, `git -C ${r} rev-parse HEAD~1`)
  const nothing = await e(c, () => sb.git.commit(r, 'nothing', ...AUTHOR))
  return { untracked: s1, staged: s2, modified: s3, sha_is_head: head === sha, empty_len: empty.sha.length, log: log.split('\n'), nothing_to_commit: nothing }
})

testCase('git.branch_lifecycle', 'space', async (c) => {
  const sb = await c.main()
  const { a } = await seeded(c, sb, 'br')
  await sb.git.createBranch(a, 'dev')
  const exists = await e(c, () => sb.git.createBranch(a, 'dev'))
  await sb.git.checkoutBranch(a, 'dev')
  const on_dev = (await sb.git.branches(a)).current
  const missing = await e(c, () => sb.git.checkoutBranch(a, 'nope'))
  await sb.git.checkoutBranch(a, 'main')
  await sb.git.deleteBranch(a, 'dev')
  const gone = await e(c, () => sb.git.deleteBranch(a, 'dev'))
  const final = [...(await sb.git.branches(a)).branches].sort()
  return { exists, on_dev, missing, gone, final }
})

testCase('git.push_pull_roundtrip', 'space', async (c) => {
  const sb = await c.main()
  const { d, remote, a } = await seeded(c, sb, 'pp')
  const b = `${d}/b`
  await sb.git.clone(remote, b)
  await sb.git.configureUser('Bo', 'bo@example.test', 'local', b)
  const s0 = normS(await sb.git.status(b))
  await write(sb, `${a}/g.txt`, 'g\n')
  await commit(sb, a, 'second')
  const ahead = normS(await sb.git.status(a))
  await sb.git.push(a)
  const pushed = normS(await sb.git.status(a))
  await sh(sb, `git -C ${b} fetch --quiet origin`)
  const behind = normS(await sb.git.status(b))
  await sb.git.pull(b)
  const pulled = await text(sb, `${b}/g.txt`)
  const again = normS(await sb.git.status(b))
  await sb.git.createBranch(a, 'topic')
  await sb.git.checkoutBranch(a, 'topic')
  const unpub = normS(await sb.git.status(a))
  await sb.git.push(a, undefined, undefined, 'topic', undefined, true)
  const pub = normS(await sb.git.status(a))
  return { clone: s0, ahead, pushed, behind, pulled, after_pull: again, unpublished: unpub, published: pub }
})

testCase('git.push_rejected_and_conflict', 'space', async (c) => {
  const sb = await c.main()
  const { d, remote, a } = await seeded(c, sb, 'rej')
  const b = `${d}/b`
  await sb.git.clone(remote, b)
  await sb.git.configureUser('Bo', 'bo@example.test', 'local', b)
  await write(sb, `${a}/f.txt`, 'from a\n')
  await commit(sb, a, 'a edits')
  await sb.git.push(a)
  await write(sb, `${b}/f.txt`, 'from b\n')
  await commit(sb, b, 'b edits')
  const rejected = await e(c, () => sb.git.push(b))
  const conflict = await e(c, () => sb.git.pull(b))
  const st = normS(await sb.git.status(b))
  return { rejected, conflict, conflicted: st.files.filter((f: any) => f[1] === 'Updated but unmerged') }
})

testCase('git.dirty_checkout', 'space', async (c) => {
  const sb = await c.main()
  const { a } = await seeded(c, sb, 'dirty')
  await sb.git.createBranch(a, 'other')
  await sb.git.checkoutBranch(a, 'other')
  await write(sb, `${a}/f.txt`, 'other\n')
  await commit(sb, a, 'other edit')
  await sb.git.checkoutBranch(a, 'main')
  await sb.git.checkoutBranch(a, 'other')
  await sb.git.checkoutBranch(a, 'main')
  await write(sb, `${a}/f.txt`, 'dirty\n')
  const dirty = await e(c, () => sb.git.checkoutBranch(a, 'other'))
  await sb.git.restore(a, ['f.txt'])
  const restored = await text(sb, `${a}/f.txt`)
  return { dirty, restored }
})

testCase('git.repo_errors', 'space', async (c) => {
  const sb = await c.main()
  const { remote, a } = await seeded(c, sb, 'err')
  const plain = await base(c, sb, 'err2')
  return {
    status_not_repo: await e(c, () => sb.git.status(plain)),
    branches_not_repo: await e(c, () => sb.git.branches(`${plain}/nope`)),
    clone_missing_repo: await e(c, () => sb.git.clone(`${plain}/missing.git`, `${plain}/c1`)),
    clone_missing_branch: await e(c, () => sb.git.clone(remote, `${plain}/c2`, 'nope')),
    clone_ok_branch: (await sb.git.clone(remote, `${plain}/c3`, 'main')) == null,
    add_nothing_matches: await e(c, () => sb.git.add(a, ['does-not-exist'])),
    reset_bad_mode: (await c.raises(() => sb.git.reset(a, 'bogus'))) !== null,
    unreachable: await e(c, () => sb.git.clone('http://127.0.0.1:9/x.git', `${plain}/c4`)),
  }
})

testCase('git.reset_restore', 'space', async (c) => {
  const sb = await c.main()
  const { a } = await seeded(c, sb, 'rr')
  await write(sb, `${a}/f.txt`, 'two\n')
  const sha2 = await commit(sb, a, 'second')
  await write(sb, `${a}/f.txt`, 'three\n')
  await sb.git.add(a, ['f.txt'])
  const staged = normS(await sb.git.status(a)).files
  await sb.git.restore(a, ['f.txt'], true)
  const unstaged = normS(await sb.git.status(a)).files
  await sb.git.restore(a, ['f.txt'], undefined, undefined, 'HEAD~1')
  const from_old = await text(sb, `${a}/f.txt`)
  await sb.git.add(a, ['f.txt'])
  await sb.git.reset(a)
  const mixed = normS(await sb.git.status(a)).files
  await sb.git.reset(a, 'soft', 'HEAD~1')
  const soft_head = await sh(sb, `git -C ${a} log --format=%s -n 1`)
  await sb.git.reset(a, 'hard', sha2)
  const hard = await text(sb, `${a}/f.txt`)
  await write(sb, `${a}/f.txt`, 'scratch\n')
  await sb.git.reset(a, undefined, undefined, ['f.txt'])
  return { staged, unstaged, from_old, mixed, soft_head, hard }
})

testCase('git.remotes', 'space', async (c) => {
  const sb = await c.main()
  const { remote, a } = await seeded(c, sb, 'rem')
  const other = await base(c, sb, 'rem2')
  await sb.git.init(`${other}/o.git`, true)
  const first = (await sb.git.remotes(a)).remotes.map((r: any) => [r.name, r.url === remote])
  await sb.git.remoteAdd(a, 'backup', `${other}/o.git`)
  const dup = await e(c, () => sb.git.remoteAdd(a, 'backup', `${other}/o.git`))
  await sb.git.remoteAdd(a, 'backup', remote, undefined, true)
  const got = await sb.git.remoteGet(a, 'backup')
  const missing = (await sb.git.remoteGet(a, 'nope')) ?? null
  await sb.git.remoteAdd(a, 'fetched', remote, true)
  const names = (await sb.git.remotes(a)).remotes.map((r: any) => r.name).sort()
  await sb.git.push(a, undefined, undefined, 'main', 'backup')
  return { first, dup, overwritten: got === remote, missing, names }
})

testCase('git.config', 'space', async (c) => {
  const sb = await c.main()
  const { a } = await seeded(c, sb, 'cfg')
  await sb.git.setConfig('cua.test', 'local-value', 'local', a)
  const local = await sb.git.getConfig('cua.test', 'local', a)
  const missing = (await sb.git.getConfig('cua.nothing', 'local', a)) ?? null
  await sb.git.configureUser('Zed', 'zed@example.test', 'local', a)
  const user = [await sb.git.getConfig('user.name', 'local', a), await sb.git.getConfig('user.email', 'local', a)]
  await sb.git.setConfig(`cua.g${c.runId}`, 'global-value')
  const glob = await sb.git.getConfig(`cua.g${c.runId}`)
  await sh(sb, `git config --global --unset cua.g${c.runId}`)
  const bad_scope = await e(c, () => sb.git.getConfig('x.y', 'worktree'))
  const no_path = await e(c, () => sb.git.setConfig('x.y', 'v', 'local'))
  return { local, missing, user, global: glob, bad_scope, no_path }
})

testCase('git.clone_depth_commit', 'space', async (c) => {
  const sb = await c.main()
  const { d, remote, a } = await seeded(c, sb, 'depth')
  const first = await sh(sb, `git -C ${a} rev-parse HEAD`)
  for (let i = 0; i < 2; i++) {
    await write(sb, `${a}/f.txt`, `v${i}\n`)
    await commit(sb, a, `c${i}`)
  }
  await sb.git.push(a)
  const shallow = `${d}/shallow`
  await sb.git.clone(`file://${remote}`, shallow, undefined, undefined, undefined, undefined, undefined, 1)
  const count = await sh(sb, `git -C ${shallow} rev-list --count HEAD`)
  const pinned = `${d}/pinned`
  await sb.git.clone(remote, pinned, undefined, first)
  const st = normS(await sb.git.status(pinned))
  const content = await text(sb, `${pinned}/f.txt`)
  const bad = await e(c, () => sb.git.clone(remote, `${d}/badc`, undefined, '0'.repeat(40)))
  const left = await sh(sb, `test -e ${d}/badc && echo yes || echo no`)
  return { shallow_commits: count, detached: st.detached, pinned: content, bad_commit: bad, bad_commit_left: left }
})

testCase('git.http_auth', 'space', async (c) => {
  const sb = await c.main()
  const d = await base(c, sb, 'auth')
  const srv = `${d}/srv`
  await sh(sb, `mkdir -p ${srv}`)
  await sb.git.init(`${srv}/r.git`, true, 'main')
  await sh(sb, `git -C ${srv}/r.git config http.receivepack true`)
  await write(sb, `${d}/httpd.py`, HTTPD)
  const port = 20000 + (Number(c.runId) % 20000)
  await sh(sb, `nohup python3 ${d}/httpd.py ${srv} ${port} >${d}/httpd.log 2>&1 & echo $! >${d}/httpd.pid`)
  await sh(sb, `python3 -c "import socket,time,sys\nfor _ in range(50):\n try: socket.create_connection(('127.0.0.1', ${port}), 1).close(); sys.exit(0)\n except OSError: time.sleep(.2)\nsys.exit(1)"`)
  const url = `http://127.0.0.1:${port}/r.git`
  try {
    const no_creds = await e(c, () => sb.git.clone(url, `${d}/c0`))
    const wrong = await c.raises(() => sb.git.clone(url, `${d}/c1`, undefined, undefined, 'user', 'wrong-password-xyz'))
    const leaked = wrong !== null && wrong.message.includes('wrong-password-xyz')
    await sb.git.clone(url, `${d}/c2`, undefined, undefined, 'user', 'secret')
    await sb.git.configureUser(...AUTHOR, 'local', `${d}/c2`)
    await write(sb, `${d}/c2/x.txt`, 'x\n')
    await commit(sb, `${d}/c2`, 'x')
    const push_no = await e(c, () => sb.git.push(`${d}/c2`))
    await sb.git.push(`${d}/c2`, 'user', 'secret')
    const stored = await sh(sb, `grep -rl secret ${d}/c2/.git/config; true`)
    await sb.git.dangerouslyAuthenticate('user', 'secret', `127.0.0.1:${port}`, 'http')
    await sb.git.clone(url, `${d}/c3`)
    const helper = await sh(sb, 'git config --global credential.helper')
    await sh(sb, 'rm -f ~/.git-credentials; git config --global --unset credential.helper')
    return { no_creds, wrong_password: errObs(wrong), wrong_leaked: leaked, push_no_creds: push_no, config_has_password: stored, helper }
  } finally {
    await sh(sb, `[ -f ${d}/httpd.pid ] && kill $(cat ${d}/httpd.pid); true`)
  }
})

testCase('git.history_route', 'space', async (c) => {
  const sb = await c.main()
  const { a } = await seeded(c, sb, 'hist')
  const r = await fetch(`${process.env.DAYTONA_API_URL}/toolbox/${sb.id}/git/history?path=${a}`, {
    headers: { Authorization: `Bearer ${process.env.DAYTONA_API_KEY}` },
  })
  const items: any[] = await r.json()
  return { count: items.length, first: [items[0].author, items[0].email, items[0].message], hash_len: items[0].hash.length, keys: Object.keys(items[0]).sort() }
})
