import { assert, errObs, testCase } from '../harness.js'

const APP = `import os


def greet(name):
    return "hi " + name


class Greeter:
    def hello(self):
        return greet("x")


os.pa
`

testCase('lsp.python_session', 'space', async (c) => {
  const sb = await c.main()
  await sb.fs.uploadFile(Buffer.from(APP), '/tmp/lspp/app.py')
  const lsp = await sb.createLspServer('python', '/tmp/lspp')
  await lsp.start()
  try {
    await lsp.didOpen('/tmp/lspp/app.py')
    const comp = await lsp.completions('/tmp/lspp/app.py', { line: 12, character: 5 })
    const labels = comp.items.map((i: any) => i.label)
    assert(labels.some((l: string) => l.startsWith('path')), `labels ${labels.slice(0, 20)}`)
    const syms = await lsp.documentSymbols('/tmp/lspp/app.py')
    const names = syms.map((s: any) => s.name).sort()
    assert(names.includes('greet') && names.includes('Greeter'), `names ${names}`)
    const greet: any = syms.find((s: any) => s.name === 'greet')
    assert(greet.location.range.start.line === 3 && greet.location.uri.endsWith('/tmp/lspp/app.py'), `greet ${JSON.stringify(greet)}`)
    const ws = await lsp.sandboxSymbols('Greeter')
    assert(ws.some((s: any) => s.name === 'Greeter'), 'workspace symbols')
    await lsp.didClose('/tmp/lspp/app.py')
  } finally {
    await lsp.stop()
  }
  return { completionHasPath: true, symbols: ['Greeter', 'greet'], greetLine: 3, workspaceFindsClass: true }
}, 420)

testCase('lsp.errors', 'space', async (c) => {
  const sb = await c.main()
  const lsp = await sb.createLspServer('python', '/tmp/never-started')
  const e = await c.raises(() => lsp.completions('/tmp/never-started/a.py', { line: 0, character: 0 }))
  assert(!!e && e.type === 'DaytonaLspServerNotInitializedError', `not started ${JSON.stringify(e)}`)
  assert(e!.status_code === 400 && e!.code === 'LSP_SERVER_NOT_INITIALIZED', 'envelope')
  // the TypeScript client refuses an unknown language itself; Python asks the server
  const bad = await c.raises(async () => (await sb.createLspServer('cobol', '/tmp/x')).start())
  assert(!!bad && (bad.status_code === 400 || ['DaytonaInvalidArgumentError', 'DaytonaBadRequestError'].includes(bad.type)), `unsupported language ${JSON.stringify(bad)}`)
  return { notStarted: errObs(e), unsupportedLanguage: 400 }
})
