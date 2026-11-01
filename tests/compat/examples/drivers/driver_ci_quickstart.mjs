// Our driver for the @e2b/code-interpreter quickstart (README of
// e2b-dev/code-interpreter). Written by us; the SDK is the unmodified npm
// package. Prints PASS <check> per check and a final PASS.
import { Sandbox } from '@e2b/code-interpreter'

if (process.env.EX_IN_SPACE !== '1') { console.error('runs only inside a job Space (EX_IN_SPACE=1)'); process.exit(2) }

const failed = []
const check = (name, cond, detail = '') => {
  console.log((cond ? 'PASS ' : 'FAIL ') + name + (!cond && detail ? ` (${detail})` : ''))
  if (!cond) failed.push(name)
}
const sbx = await Sandbox.create({ timeoutMs: 300_000 })
try {
  await sbx.runCode('x = 1')
  const r = await sbx.runCode('x += 1; x')
  check('state persists across runCode', r.text === '2', r.text)

  const lines = []
  await sbx.runCode("print('a'); print('b')", { onStdout: (m) => lines.push(m.line) })
  check('stdout callback order', lines.join('').replace(/\n/g, '') === 'ab', JSON.stringify(lines))

  const e = await sbx.runCode('1/0')
  check('error name', e.error?.name === 'ZeroDivisionError', JSON.stringify(e.error))

  const ctx = await sbx.createCodeContext()
  await sbx.runCode('y = 41', { context: ctx })
  check('context isolation', (await sbx.runCode('y + 1', { context: ctx })).text === '42')
  const n = await sbx.runCode('y')
  check('default context does not see y', n.error?.name === 'NameError', JSON.stringify(n.error))

  const js = await sbx.runCode("console.log('js-ok'); 6*7", { language: 'js' })
  check('javascript kernel', js.logs.stdout.join('').includes('js-ok'), JSON.stringify(js.logs))

  check('commands share the sandbox', (await sbx.commands.run('echo shell-ok')).stdout.trim() === 'shell-ok')
} finally {
  await sbx.kill()
}
console.log(failed.length ? 'FAILED: ' + failed.join(', ') : 'PASS')
process.exit(failed.length ? 1 : 0)
