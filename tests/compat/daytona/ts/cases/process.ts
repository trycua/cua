import { assert, eq, errObs, testCase } from '../harness.js'

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms))

testCase('process.exec_result_and_exit_code', 'mock', async (c) => {
  const sb = await c.main()
  let r = await sb.process.executeCommand('echo hi there')
  assert(r.exitCode === 0 && r.result === 'hi there\n', `echo ${JSON.stringify(r)}`)
  assert(r.artifacts?.stdout === r.result && r.artifacts?.charts === undefined || (r.artifacts?.charts ?? []).length === 0, 'artifacts')
  r = await sb.process.executeCommand('exit 3')
  assert(r.exitCode === 3, `fail ${r.exitCode}`)
  return { echo: 'hi there\n', fail: 3 }
})

testCase('process.exec_timeout_error', 'mock', async (c) => {
  const sb = await c.main()
  const t0 = Date.now()
  const e = await c.raises(() => sb.process.executeCommand('sleep 20000', undefined, undefined, 1))
  assert(!!e && e.type === 'DaytonaProcessExecutionTimeoutError', `timeout ${JSON.stringify(e)}`)
  assert(e!.status_code === 408 && e!.code === 'PROCESS_EXECUTION_TIMEOUT' && e!.source === 'DAYTONA_DAEMON', 'envelope')
  assert(Date.now() - t0 < 15000, 'the command was not stopped at its timeout')
  return errObs(e)
})

testCase('process.exec_cwd_env_user', 'space', async (c) => {
  const sb = await c.main()
  let r = await sb.process.executeCommand('pwd', '/tmp')
  assert(r.result.trim() === '/tmp', `cwd ${r.result}`)
  r = await sb.process.executeCommand('echo "$FOO:$BAR"', undefined, { FOO: 'bar', BAR: 'two words' })
  assert(r.result.trim() === 'bar:two words', `env ${r.result}`)
  r = await sb.process.executeCommand('whoami')
  assert(r.result.trim() === 'daytona', `user ${r.result}`)
  const rel = await sb.process.executeCommand('pwd', 'rel-dir-from-home')
  return { cwd: '/tmp', env: 'bar:two words', user: 'daytona', relativeCwdFailsOrResolves: [0, 1, 2].includes(rel.exitCode) }
})

testCase('process.exec_shell_semantics', 'space', async (c) => {
  const sb = await c.main()
  let r = await sb.process.executeCommand('echo a; echo b >&2; exit 5')
  assert(r.exitCode === 5 && r.result.includes('a') && r.result.includes('b'), `exit ${JSON.stringify(r)}`)
  r = await sb.process.executeCommand("cat <<'EOF'\nline1\nline2\nEOF")
  assert(r.result === 'line1\nline2\n', `heredoc ${JSON.stringify(r.result)}`)
  r = await sb.process.executeCommand('true && echo yes || echo no')
  assert(r.result.trim() === 'yes', 'and-or')
  r = await sb.process.executeCommand("nohup sh -c 'sleep 60' > /tmp/bg.log 2>&1 &")
  assert(r.exitCode === 0, 'background')
  return { exit: 5, heredoc: true, background: true }
})

testCase('process.exec_large_output', 'space', async (c) => {
  const sb = await c.main()
  const r = await sb.process.executeCommand("head -c 3000000 /dev/zero | tr '\\0' 'x'")
  assert(r.exitCode === 0 && r.result.length === 3_000_000, `len ${r.result.length}`)
  return { bytes: r.result.length }
})

testCase('process.code_run_python', 'space', async (c) => {
  const sb = await c.main()
  let r = await sb.process.codeRun("import sys; print(6*7, sys.argv[1:])", { argv: ['a'] })
  assert(r.exitCode === 0 && r.result.trim() === "42 ['a']", `argv ${JSON.stringify(r)}`)
  r = await sb.process.codeRun("import os; print(os.environ['K'])", { env: { K: 'v' } })
  assert(r.result.trim() === 'v', 'env')
  r = await sb.process.codeRun('raise SystemExit(4)')
  assert(r.exitCode === 4, `exit ${r.exitCode}`)
  r = await sb.process.codeRun('1/0')
  assert(r.exitCode !== 0 && r.result.includes('ZeroDivisionError'), 'traceback')
  return { argv: "42 ['a']", env: 'v', exit: 4, traceback: true }
})

testCase('process.code_run_timeout', 'space', async (c) => {
  const sb = await c.main()
  const e = await c.raises(() => sb.process.codeRun('import time; time.sleep(30)', undefined, 1))
  assert(!!e && (e.type === 'DaytonaProcessExecutionTimeoutError' || e.status_code === 408), `timeout ${JSON.stringify(e)}`)
  return { timedOut: true, status_code: 408 }
})

testCase('process.session_state_carries_over', 'space', async (c) => {
  const sb = await c.main()
  await sb.process.createSession('s1')
  let r = await sb.process.executeSessionCommand('s1', { command: 'cd /tmp && export X=7' })
  assert(r.exitCode === 0, 'first')
  r = await sb.process.executeSessionCommand('s1', { command: 'echo "$PWD:$X"' })
  assert((r.stdout ?? r.output ?? '').trim() === '/tmp:7', `state ${JSON.stringify(r)}`)
  r = await sb.process.executeSessionCommand('s1', { command: 'echo out; echo err >&2; exit 3' })
  assert(r.exitCode === 3 && (r.stdout ?? '').trim() === 'out' && (r.stderr ?? '').trim() === 'err', `third ${JSON.stringify(r)}`)
  const s = await sb.process.getSession('s1')
  eq(s.commands!.map((x: any) => x.exitCode), [0, 0, 3], 'exit codes')
  assert((await sb.process.listSessions()).some((x: any) => x.sessionId === 's1'), 'listed')
  await sb.process.deleteSession('s1')
  const e = await c.raises(() => sb.process.getSession('s1'))
  assert(!!e && e.status_code === 404, 'deleted')
  return { state: '/tmp:7', exitCodes: [0, 0, 3], deleted: errObs(e) }
})

testCase('process.session_async_command_and_logs', 'space', async (c) => {
  const sb = await c.main()
  await sb.process.createSession('sa')
  const a = await sb.process.executeSessionCommand('sa', { command: 'sleep 1; echo late; echo bad >&2', runAsync: true })
  assert(!!a.cmdId && (a.exitCode === undefined || a.exitCode === null), 'async start')
  assert(((await sb.process.getSessionCommand('sa', a.cmdId!)).exitCode ?? null) === null, 'still running')
  let cmd: any
  for (let i = 0; i < 60; i++) {
    cmd = await sb.process.getSessionCommand('sa', a.cmdId!)
    if (cmd.exitCode !== undefined && cmd.exitCode !== null) break
    await sleep(200)
  }
  assert(cmd.exitCode === 0, `exit ${cmd.exitCode}`)
  const logs = await sb.process.getSessionCommandLogs('sa', a.cmdId!)
  assert(logs.stdout!.trim() === 'late' && logs.stderr!.trim() === 'bad', `logs ${JSON.stringify(logs)}`)
  assert((logs.output ?? '').split('late').length === 2 && (logs.output ?? '').includes('bad'), 'combined')
  await sb.process.deleteSession('sa')
  return { asyncPending: true, stdout: 'late', stderr: 'bad', exit: 0 }
})

testCase('process.session_follow_logs', 'space', async (c) => {
  const sb = await c.main()
  await sb.process.createSession('sf')
  const a = await sb.process.executeSessionCommand('sf', {
    command: 'for i in 1 2 3; do echo o$i; sleep 0.3; done; echo e >&2',
    runAsync: true,
  })
  let so = ''
  let se = ''
  await sb.process.getSessionCommandLogs('sf', a.cmdId!, (ch: string) => (so += ch), (ch: string) => (se += ch))
  eq(so.replace(/\r/g, '').split(/\s+/).filter(Boolean), ['o1', 'o2', 'o3'], 'stdout')
  assert(se.trim() === 'e', `stderr ${se}`)
  await sb.process.deleteSession('sf')
  return { stdout: ['o1', 'o2', 'o3'], stderr: 'e' }
})

testCase('process.session_input_to_running_command', 'space', async (c) => {
  const sb = await c.main()
  await sb.process.createSession('si')
  const a = await sb.process.executeSessionCommand('si', { command: 'read line; echo got:$line', runAsync: true })
  await sb.process.sendSessionCommandInput('si', a.cmdId!, 'hello\n')
  let cmd: any
  for (let i = 0; i < 60; i++) {
    cmd = await sb.process.getSessionCommand('si', a.cmdId!)
    if (cmd.exitCode !== undefined && cmd.exitCode !== null) break
    await sleep(200)
  }
  const logs = await sb.process.getSessionCommandLogs('si', a.cmdId!)
  assert((logs.stdout ?? '').includes('got:hello'), `logs ${JSON.stringify(logs)}`)
  const e = await c.raises(() => sb.process.sendSessionCommandInput('si', a.cmdId!, 'late\n'))
  assert(!!e && e.status_code === 410 && e.code === 'COMMAND_ALREADY_COMPLETED', `late input ${JSON.stringify(e)}`)
  await sb.process.deleteSession('si')
  return { got: 'hello', afterCompletion: errObs(e) }
})

testCase('process.session_command_ordering', 'space', async (c) => {
  const sb = await c.main()
  await sb.process.createSession('so')
  const ids: string[] = []
  for (let i = 0; i < 5; i++) {
    const a = await sb.process.executeSessionCommand('so', { command: `echo ${i} >> /tmp/order.txt; sleep 0.2`, runAsync: true })
    ids.push(a.cmdId!)
  }
  for (const id of ids) {
    for (let i = 0; i < 100; i++) {
      const cmd = await sb.process.getSessionCommand('so', id)
      if (cmd.exitCode !== undefined && cmd.exitCode !== null) break
      await sleep(100)
    }
  }
  const r = await sb.process.executeCommand('cat /tmp/order.txt && rm /tmp/order.txt')
  eq(r.result.split(/\s+/).filter(Boolean), ['0', '1', '2', '3', '4'], 'order')
  await sb.process.deleteSession('so')
  return { order: ['0', '1', '2', '3', '4'] }
})

testCase('process.session_errors', 'mock', async (c) => {
  const sb = await c.main()
  const out: Record<string, any> = {}
  const tries: Record<string, () => Promise<unknown>> = {
    get: () => sb.process.getSession('no-such-session'),
    delete: () => sb.process.deleteSession('no-such-session'),
    logs: () => sb.process.getSessionCommandLogs('no-such-session', 'x'),
  }
  for (const [name, fn] of Object.entries(tries)) {
    const e = await c.raises(fn)
    assert(!!e && e.status_code === 404, `${name} ${JSON.stringify(e)}`)
    out[name] = errObs(e)
  }
  await sb.process.createSession('dupe')
  const dup = await c.raises(() => sb.process.createSession('dupe'))
  assert(!!dup && [400, 409].includes(dup.status_code!), 'duplicate')
  await sb.process.deleteSession('dupe')
  return { missing: out, duplicate: errObs(dup) }
})

testCase('process.pty_roundtrip', 'space', async (c) => {
  const sb = await c.main()
  const chunks: Uint8Array[] = []
  const h = await sb.process.createPty({ id: 'p1', cols: 80, rows: 24, onData: (d: Uint8Array) => void chunks.push(d) } as any)
  await h.waitForConnection()
  await h.sendInput('echo pty-$((6*7))\nexit\n')
  const r = await h.wait()
  assert(Buffer.concat(chunks).includes('pty-42'), 'pty output')
  assert(r.exitCode === 0, `exit ${r.exitCode}`)
  return { echo: 'pty-42', exit: 0 }
})

testCase('process.pty_resize_list_kill', 'space', async (c) => {
  const sb = await c.main()
  const h = await sb.process.createPty({ id: 'p2', cols: 80, rows: 24, onData: () => {} } as any)
  await h.waitForConnection()
  assert((await sb.process.listPtySessions()).some((p: any) => p.id === 'p2'), 'listed')
  const info = await sb.process.resizePtySession('p2', 120, 40)
  assert(info.cols === 120 && info.rows === 40, 'resized')
  const got = await sb.process.getPtySessionInfo('p2')
  assert(got.id === 'p2' && got.active, 'info')
  await sb.process.killPtySession('p2')
  const e = await c.raises(() => sb.process.getPtySessionInfo('p2'))
  assert(!!e && e.status_code === 404, 'killed')
  return { resized: [120, 40], killed: errObs(e) }
})

testCase('process.background_server_and_port', 'space', async (c) => {
  const sb = await c.main()
  await sb.process.executeCommand('mkdir -p /tmp/www9 && echo -n up > /tmp/www9/index.html')
  await sb.process.executeCommand('cd /tmp/www9 && nohup python3 -m http.server 9009 > /tmp/www9.log 2>&1 &')
  let r: any
  for (let i = 0; i < 40; i++) {
    r = await sb.process.executeCommand('curl -s localhost:9009/index.html || true')
    if (r.result.trim() === 'up') break
    await sleep(500)
  }
  assert(r.result.trim() === 'up', `served ${r.result}`)
  return { served: 'up' }
})
