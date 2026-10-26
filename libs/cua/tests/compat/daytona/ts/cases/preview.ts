import { assert, testCase } from '../harness.js'

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms))
const get = async (url: string, headers: Record<string, string> = {}) => {
  const r = await fetch(url, { headers })
  return [r.status, await r.text()] as const
}

const serve = async (sb: any) => {
  if (!process.env.COMPAT_REAL_GUEST) return
  const r = await sb.process.executeCommand(
    "mkdir -p /tmp/www && printf hello-from-port-8080 >/tmp/www/index.html && (setsid nohup python3 -m http.server 8080 --directory /tmp/www >/dev/null 2>&1 &) ; for i in $(seq 50); do python3 -c 'import urllib.request;urllib.request.urlopen(\"http://127.0.0.1:8080/\")' 2>/dev/null && exit 0; sleep 0.2; done; exit 1",
  )
  assert(r.exitCode === 0, `serve ${r.result}`)
}

testCase('preview.private_link_needs_its_token', 'mock', async (c) => {
  const sb = await c.newSandbox({ autoStopInterval: 0 })
  await serve(sb)
  await serve(sb)
  await serve(sb)
  const link = await sb.getPreviewLink(8080)
  assert(link.sandboxId === sb.id && !!link.token && link.url.startsWith('https://8080-'), `link ${JSON.stringify(link)}`)
  const ok = await get(link.url, { 'X-Daytona-Preview-Token': link.token })
  assert(ok[0] === 200 && ok[1] === 'hello-from-port-8080', `token ${ok}`)
  assert((await get(link.url))[0] === 401, 'without')
  assert((await get(link.url, { 'X-Daytona-Preview-Token': 'wrong' }))[0] === 401, 'wrong')
  const viaKey = await get(link.url, { Authorization: `Bearer ${process.env.DAYTONA_API_KEY}` })
  assert(viaKey[0] === 200, 'api key')
  return { withToken: 200, without: 401, wrong: 401, withApiKey: 200 }
})

testCase('preview.public_link_needs_nothing', 'mock', async (c) => {
  const sb = await c.newSandbox({ public: true, autoStopInterval: 0 })
  await serve(sb)
  const link = await sb.getPreviewLink(8080)
  const r = await get(link.url)
  assert(r[0] === 200 && r[1] === 'hello-from-port-8080', 'public')
  await sb.setPublic(false)
  assert((await get(link.url))[0] === 401, 'after private')
  return { public: 200, afterPrivate: 401 }
})

testCase('preview.signed_url', 'mock', async (c) => {
  const sb = await c.newSandbox({ autoStopInterval: 0 })
  await serve(sb)
  const s = await sb.getSignedPreviewUrl(8080, 60)
  assert(s.port === 8080 && s.sandboxId === sb.id && s.url.includes(s.token), 'fields')
  const ok = await get(s.url)
  assert(ok[0] === 200 && ok[1] === 'hello-from-port-8080', `works ${ok} ${s.url}`)
  await sb.expireSignedPreviewUrl(8080, s.token)
  assert([401, 403, 404].includes((await get(s.url))[0]), 'expired')
  const short = await sb.getSignedPreviewUrl(8080, 1)
  assert((await get(short.url))[0] === 200, 'short works')
  await sleep(2200)
  assert([401, 403, 404].includes((await get(short.url))[0]), 'timed out')
  const def = await sb.getSignedPreviewUrl(8080)
  assert((await get(def.url))[0] === 200, 'default')
  return { works: 200, afterExpire: 'refused', afterTimeout: 'refused', defaultValid: true }
})

testCase('preview.port_not_open', 'mock', async (c) => {
  const sb = await c.newSandbox({ autoStopInterval: 0 })
  const link = await sb.getPreviewLink(9999)
  const r = await get(link.url, { 'X-Daytona-Preview-Token': link.token })
  assert(r[0] === 502, `closed ${r[0]}`)
  return { closedPort: 502 }
})

testCase('sandbox.metrics', 'mock', async (c) => {
  const sb = await c.main()
  const m = await sb.getMetricsLatest()
  assert(m.cpuCount >= 0 && m.memTotal > 0 && m.diskTotal >= 0, `metrics ${JSON.stringify(m)}`)
  const series = await sb.getMetrics()
  assert(series.length >= 1, 'series')
  return { hasCpu: true, hasMemory: true }
})

testCase('sandbox.home_and_work_dirs', 'mock', async (c) => {
  const sb = await c.main()
  const home = await sb.getUserHomeDir()
  const work = await sb.getWorkDir()
  assert(home!.startsWith('/') && work!.startsWith('/'), 'absolute')
  assert((await sb.getUserRootDir()) === home, 'root dir')
  return { absolute: true }
})
