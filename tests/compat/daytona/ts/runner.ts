// Runs the shared cases with the TypeScript SDK. Usage:
//   npx tsx runner.ts --env env.json [--needs mock,space] [--only git,fs.list] [--out run.json]
import { readFileSync, readdirSync, writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { CASES, runAll } from './harness.js'

const args = process.argv.slice(2)
const opt = (n: string, d = ''): string => {
  const i = args.indexOf(`--${n}`)
  return i >= 0 ? args[i + 1] : d
}
const envFile = opt('env')
if (envFile) Object.assign(process.env, JSON.parse(readFileSync(envFile, 'utf8')))
const here = dirname(fileURLToPath(import.meta.url))
for (const f of readdirSync(join(here, 'cases')).filter((f) => f.endsWith('.ts')).sort()) {
  await import(pathToFileURL(join(here, 'cases', f)).href)
}
const only = opt('only').split(',').filter(Boolean)
const needs = new Set(opt('needs', 'mock,space').split(','))
const selected = [...CASES.keys()].sort().filter((id) => !only.length || only.some((o) => id === o || id.startsWith(`${o}.`)))
if (args.includes('--list')) {
  console.log(JSON.stringify([...CASES.values()].sort((a, b) => (a.id < b.id ? -1 : 1)).map((c) => ({ id: c.id, needs: c.needs })), null, 1))
  process.exit(0)
}
const t0 = Date.now()
const cases = await runAll(selected, needs, (m) => console.error(m))
const sdkVersion = JSON.parse(readFileSync(join(here, 'node_modules/@daytonaio/sdk/package.json'), 'utf8')).version
const doc = { runner: 'typescript', sdk: sdkVersion, seconds: Math.round((Date.now() - t0) / 100) / 10, cases }
const s = JSON.stringify(doc, null, 1)
if (opt('out')) writeFileSync(opt('out'), s)
else console.log(s)
const failed = Object.entries(cases).filter(([, v]: any) => !v.ok).map(([k]) => k)
console.error(`${Object.keys(cases).length - failed.length} passed, ${failed.length} failed ${JSON.stringify(failed)}`)
process.exit(failed.length ? 1 : 0)
