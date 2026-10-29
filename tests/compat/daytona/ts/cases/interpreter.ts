import { assert, errObs, testCase } from '../harness.js'

testCase('interpreter.default_context_keeps_state', 'space', async (c) => {
  const sb = await c.main()
  const ci = sb.codeInterpreter
  let r = await ci.runCode('x = 41\nimport math\ndef f(a): return a + 1')
  assert(!r.error && r.stdout === '' && r.stderr === '', `first ${JSON.stringify(r)}`)
  r = await ci.runCode('print(f(x), math.floor(2.7))')
  assert(r.stdout === '42 2\n' && !r.error, `second ${JSON.stringify(r)}`)
  return { stdout: '42 2\n' }
})

testCase('interpreter.contexts_are_isolated', 'space', async (c) => {
  const sb = await c.main()
  const ci = sb.codeInterpreter
  const a = await ci.createContext()
  const b = await ci.createContext('/tmp')
  try {
    await ci.runCode("v = 'a'", { context: a })
    await ci.runCode("v = 'b'", { context: b })
    const ra = await ci.runCode('print(v)', { context: a })
    const rb = await ci.runCode('print(v)', { context: b })
    assert(ra.stdout === 'a\n' && rb.stdout === 'b\n', 'isolated')
    const rc = await ci.runCode('import os; print(os.getcwd())', { context: b })
    assert(rc.stdout.trim() === '/tmp', 'cwd')
    const miss = await ci.runCode('print(v)')
    assert(miss.error?.name === 'NameError', 'default context has no v')
    const ids = new Set((await ci.listContexts()).map((x: any) => x.id))
    assert(ids.has(a.id) && ids.has(b.id) && !ids.has('default'), 'listed')
    assert(a.language === 'python' && b.cwd === '/tmp' && a.active === true, 'fields')
  } finally {
    await ci.deleteContext(a)
    await ci.deleteContext(b)
  }
  const gone = await c.raises(() => ci.deleteContext(a))
  assert(!!gone && gone.status_code === 404, `delete twice ${JSON.stringify(gone)}`)
  const ids = new Set((await ci.listContexts()).map((x: any) => x.id))
  assert(!ids.has(a.id) && !ids.has(b.id), 'removed')
  return { isolated: true, cwd: '/tmp', defaultNotListed: true, deleteTwice: errObs(gone) }
})

testCase('interpreter.streams_stdout_stderr_and_errors', 'space', async (c) => {
  const sb = await c.main()
  const ci = sb.codeInterpreter
  const out: string[] = []
  const err: string[] = []
  const errs: any[] = []
  const code =
    "import sys, time\nfor i in range(3):\n    print('o%d' % i, flush=True)\n    time.sleep(0.2)\nsys.stderr.write('warn\\n')\nraise ValueError('boom')\n"
  const r = await ci.runCode(code, {
    onStdout: (m: any) => void out.push(m.output),
    onStderr: (m: any) => void err.push(m.output),
    onError: (e: any) => void errs.push(e),
  })
  assert(out.join('') === 'o0\no1\no2\n' && out.length >= 2, `stdout ${JSON.stringify(out)}`)
  assert(err.join('') === 'warn\n', 'stderr')
  assert(r.stdout === 'o0\no1\no2\n' && r.stderr === 'warn\n', 'result')
  assert(r.error?.name === 'ValueError' && r.error.value === 'boom', `error ${JSON.stringify(r.error)}`)
  assert(r.error!.traceback!.includes('ValueError: boom') && r.error!.traceback!.includes('Traceback'), 'traceback')
  assert(errs.length === 1 && errs[0].name === 'ValueError', 'on_error')
  const syn = await ci.runCode('def (:')
  assert(syn.error?.name === 'SyntaxError', `syntax ${JSON.stringify(syn.error)}`)
  return { stdout: 'o0\no1\no2\n', stderr: 'warn\n', error: ['ValueError', 'boom'], syntax: 'SyntaxError', streamedInChunks: true }
})

testCase('interpreter.envs_and_exit', 'space', async (c) => {
  const sb = await c.main()
  const ci = sb.codeInterpreter
  let r = await ci.runCode("import os; print(os.environ.get('K_ENV'))", { envs: { K_ENV: 'v1' } })
  assert(r.stdout === 'v1\n', 'env')
  r = await ci.runCode("import os; print(os.environ.get('K_ENV'))")
  assert(r.stdout === 'None\n', "an execution's envs do not outlive it")
  r = await ci.runCode("import subprocess; subprocess.run(['echo', 'from-child'])")
  assert(r.stdout === 'from-child\n', `child ${JSON.stringify(r)}`)
  r = await ci.runCode('raise SystemExit(3)')
  assert(!!r.error, 'exit is an error')
  r = await ci.runCode("print('still alive')")
  assert(r.stdout === 'still alive\n', 'alive')
  return { env: 'v1', restored: 'None', child: 'from-child\n', survivesExit: true }
})

testCase('interpreter.timeout', 'space', async (c) => {
  const sb = await c.main()
  const ci = sb.codeInterpreter
  const e = await c.raises(() => ci.runCode('import time\nwhile True:\n    time.sleep(0.1)', { timeout: 1 }))
  assert(!!e && e.type === 'DaytonaTimeoutError', `timeout ${JSON.stringify(e)}`)
  const r = await ci.runCode("print('ok after timeout')")
  assert(r.stdout === 'ok after timeout\n', 'usable after')
  return { timeout: errObs(e), usableAfter: true }
})

testCase('process.code_run_charts', 'space', async (c) => {
  const sb = await c.main()
  const have = await sb.process.executeCommand(
    "python3 -c 'import matplotlib' 2>&1 || python3 -m pip install --user --break-system-packages -q matplotlib 2>&1 | tail -1; python3 -c 'import matplotlib; print(1)' 2>&1 | tail -1",
  )
  const last = have.result.trim().split('\n').pop()
  if (last !== '1') return { skipped: 'no matplotlib in the guest' }
  const code = `
import matplotlib.pyplot as plt
plt.figure(); plt.plot([1, 2, 3], [4, 5, 6], label='l1'); plt.title('Line'); plt.xlabel('X'); plt.ylabel('Y'); plt.show()
plt.figure(); plt.scatter([1, 2], [3, 4], label='pts'); plt.title('Scatter')
plt.figure(); plt.bar(['a', 'b'], [3, 5]); plt.title('Bars')
plt.figure(); plt.pie([30, 70], labels=['p', 'q']); plt.title('Pie')
plt.figure()
try:
    plt.boxplot([[1, 2, 3, 4, 100]], tick_labels=['one'])
except TypeError:
    plt.boxplot([[1, 2, 3, 4, 100]], labels=['one'])
plt.title('Box')
print('done')
`
  const r = await sb.process.codeRun(code)
  assert(r.exitCode === 0 && r.result === 'done\n' && r.artifacts?.stdout === 'done\n', `run ${JSON.stringify(r).slice(0, 300)}`)
  const charts: any[] = r.artifacts?.charts ?? []
  const kinds = charts.map((ch) => ch.type)
  assert(JSON.stringify(kinds) === JSON.stringify(['line', 'scatter', 'bar', 'pie', 'box_and_whisker']), `kinds ${kinds}`)
  const line = charts[0]
  assert(line.title === 'Line' && line.x_label === 'X' && line.y_label === 'Y', `line ${JSON.stringify(line).slice(0, 200)}`)
  assert(line.elements[0].label === 'l1' && JSON.stringify(line.elements[0].points) === '[[1,4],[2,5],[3,6]]', 'points')
  assert(charts.every((ch) => !!ch.png), 'png')
  const bar = charts[2]
  assert(JSON.stringify(bar.elements.map((e: any) => [e.label, e.value])) === '[["a","3"],["b","5"]]', 'bars')
  const box = charts[4].elements[0]
  assert(box.label === 'one' && box.median === 3 && JSON.stringify(box.outliers) === '[100]', `box ${JSON.stringify(box)}`)
  const none = await sb.process.codeRun("print('no figures')")
  assert((none.artifacts?.charts ?? []).length === 0 && none.result === 'no figures\n', 'no charts')
  return { types: kinds, line: [[1, 4], [2, 5], [3, 6]], bars: [['a', '3'], ['b', '5']], boxMedian: 3, outliers: [100] }
})
