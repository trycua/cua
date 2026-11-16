import { Image } from '@daytonaio/sdk'
import { mkdirSync, mkdtempSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { assert, eq, errObs, testCase } from '../harness.js'

const started = (sb: any) => String(sb.state).toLowerCase().endsWith('started')
const collect = async (it: AsyncIterable<any>) => {
  const out: any[] = []
  for await (const s of it) out.push(s)
  return out
}
const dropSnap = async (c: any, n: string) => {
  await c.raises(() => c.daytona.snapshot.delete(n))
}

testCase('snapshot.create_list_get_delete', 'mock', async (c) => {
  const name = c.name('snap')
  const made = await c.daytona.snapshot.create({ name, image: 'python:3.12' })
  assert(made.name === name && String(made.state).toLowerCase().endsWith('active'), `state ${made.state}`)
  const got = await c.daytona.snapshot.get(name)
  assert(got.id === made.id && got.imageName === 'python:3.12', 'get')
  const page = await c.daytona.snapshot.list({ page: 1, limit: 100 })
  assert(page.items.some((s: any) => s.name === name) && page.total >= 1 && page.page === 1, 'list')
  const sb = await c.newSandbox({ snapshot: name })
  assert(started(sb) && sb.snapshot === name, 'sandbox from snapshot')
  await c.daytona.snapshot.delete(name)
  const e = await c.raises(() => c.daytona.snapshot.get(name))
  assert(!!e && e.type === 'DaytonaNotFoundError', 'after delete')
  return { state: 'active', imageName: 'python:3.12', sandboxSnapshot: name === sb.snapshot, afterDelete: errObs(e) }
})

testCase('snapshot.pagination', 'mock', async (c) => {
  const tag = c.name('pg')
  const names: string[] = []
  for (let i = 0; i < 3; i++) {
    const n = `${tag}-${i}`
    await c.daytona.snapshot.create({ name: n, image: 'python:3.12' })
    names.push(n)
  }
  try {
    const p1 = await c.daytona.snapshot.list({ page: 1, limit: 1 })
    assert(p1.items.length === 1 && p1.total >= 3 && p1.totalPages >= 3, 'page 1')
    const bad = await c.raises(() => c.daytona.snapshot.list({ page: 0 }))
    assert(bad !== null, 'page 0 refused')
    return { pageSize: p1.items.length, totalAtLeast3: p1.total >= 3, pageZeroRefused: true }
  } finally {
    for (const n of names) await dropSnap(c, n)
  }
})

testCase('snapshot.duplicate_and_missing', 'mock', async (c) => {
  const name = c.name('dup')
  await c.daytona.snapshot.create({ name, image: 'python:3.12' })
  try {
    const dup = await c.raises(() => c.daytona.snapshot.create({ name, image: 'python:3.12' }))
    assert(!!dup && dup.type === 'DaytonaConflictError' && dup.status_code === 409, `dup ${JSON.stringify(dup)}`)
    const miss = await c.raises(() => c.daytona.snapshot.get('no-such-snapshot-' + c.runId))
    assert(!!miss && miss.type === 'DaytonaNotFoundError', 'missing')
    const missSb = await c.raises(() => c.daytona.create({ snapshot: 'nope-' + c.runId }))
    assert(!!missSb && missSb.status_code === 404, 'sandbox from missing snapshot')
    return { duplicate: errObs(dup), missing: errObs(miss), sandboxFromMissing: errObs(missSb) }
  } finally {
    await dropSnap(c, name)
  }
})

testCase('snapshot.activate_deactivate', 'mock', async (c) => {
  const name = c.name('act')
  await c.daytona.snapshot.create({ name, image: 'python:3.12' })
  try {
    const already = await c.raises(() => c.daytona.snapshot.activate(name))
    assert(!!already && already.status_code === 400, `already active ${JSON.stringify(already)}`)
    return { activateActive: already!.status_code }
  } finally {
    await dropSnap(c, name)
  }
})

testCase('snapshot.from_sandbox', 'mock', async (c) => {
  const sb = await c.newSandbox()
  const name = c.name('fromsb')
  await sb.createSnapshot(name)
  try {
    const got = await c.daytona.snapshot.get(name)
    assert(got.name === name, 'registered')
    const kid = await c.newSandbox({ snapshot: name })
    assert(started(kid), 'sandbox from it')
    const dup = await c.raises(() => sb.createSnapshot(name))
    assert(!!dup && dup.type === 'DaytonaConflictError', 'duplicate')
    return { registered: true, sandboxFrom: true, duplicate: errObs(dup) }
  } finally {
    await dropSnap(c, name)
  }
})

testCase('snapshot.build_from_image_object', 'mock', async (c) => {
  const name = c.name('built')
  const image = Image.debianSlim('3.12').pipInstall(['requests']).env({ BUILT: 'yes' }).workdir('/w')
  const logs: string[] = []
  const s = await c.daytona.snapshot.create({ name, image }, { onLogs: (l: string) => logs.push(l) })
  try {
    assert(String(s.state).toLowerCase().endsWith('active'), `state ${s.state}`)
    assert(!!s.buildInfo && s.buildInfo.dockerfileContent!.includes('FROM python:3.12'), 'buildInfo')
    assert(logs.some((l) => l.includes('Creating snapshot')), `logs ${JSON.stringify(logs)}`)
    assert(logs.some((l) => l.includes('Created snapshot')), `logs ${JSON.stringify(logs)}`)
    const sb = await c.newSandbox({ snapshot: name })
    assert(started(sb), 'sandbox')
    return { state: 'active', dockerfile: true, logged: true }
  } finally {
    await dropSnap(c, name)
  }
})

testCase('snapshot.build_failure_is_reported', 'mock', async (c) => {
  const name = c.name('bad')
  const image = Image.base('python:3.12').dockerfileCommands(['ONBUILD RUN true'])
  const e = await c.raises(() => c.daytona.snapshot.create({ name, image }))
  try {
    assert(!!e && e.type === 'DaytonaError' && e.message.includes('Failed to create snapshot'), `error ${JSON.stringify(e)}`)
    return { type: e!.type, failed: true }
  } finally {
    await dropSnap(c, name)
  }
})

testCase('volume.crud', 'mock', async (c) => {
  const name = c.name('vol')
  const v = await c.daytona.volume.create(name)
  try {
    assert(v.name === name && String(v.state).toLowerCase().endsWith('ready'), `state ${v.state}`)
    assert((await c.daytona.volume.get(name)).id === v.id, 'get')
    assert((await c.daytona.volume.list()).some((x: any) => x.name === name), 'list')
    assert((await c.daytona.volume.get(name, true)).id === v.id, 'get create=true existing')
    const dup = await c.raises(() => c.daytona.volume.create(name))
    assert(!!dup && dup.type === 'DaytonaConflictError', 'dup')
    const bad = await c.raises(() => c.daytona.volume.create('bad name!'))
    assert(!!bad && bad.status_code === 400, 'bad name')
    const fresh = await c.daytona.volume.get(name + '-new', true)
    await c.daytona.volume.delete(fresh)
    const gone = await c.raises(() => c.daytona.volume.get(name + '-new'))
    assert(!!gone && gone.type === 'DaytonaNotFoundError', 'gone')
    return { state: 'ready', duplicate: errObs(dup), invalidName: errObs(bad), afterDelete: errObs(gone) }
  } finally {
    await c.raises(() => c.daytona.volume.delete(v))
  }
})

testCase('volume.mount_needs_cua_volume', 'hermetic', async (c) => {
  const name = c.name('mnt')
  const v = await c.daytona.volume.create(name)
  try {
    const e = await c.raises(() =>
      c.daytona.create({ labels: { vm: c.runId }, volumes: [{ volumeId: v.id, mountPath: '/data' }] }),
    )
    assert(!!e && e.status_code === 400 && e.message.includes('Cua Volume'), `no cua volume ${JSON.stringify(e)}`)
    assert((await collect(c.daytona.list({ labels: { vm: c.runId } }))).length === 0, 'no sandbox left behind')
    const missing = await c.raises(() => c.daytona.create({ volumes: [{ volumeId: 'no-such-volume', mountPath: '/data' }] }))
    assert(!!missing && missing.status_code === 404, 'missing volume')
    const badPath = await c.raises(() => c.daytona.create({ volumes: [{ volumeId: v.id, mountPath: 'relative' }] }))
    assert(!!badPath && badPath.status_code === 400, 'relative path')
    return { noCuaVolume: errObs(e), missingVolume: errObs(missing), relativePath: errObs(badPath) }
  } finally {
    await c.raises(() => c.daytona.volume.delete(v))
  }
})

testCase('secret.crud', 'mock', async (c) => {
  const name = c.name('sec').replace(/-/g, '_')
  const s = await c.daytona.secret.create({ name, value: 's3cret', hosts: ['api.example.com'], description: 'd' })
  try {
    assert(s.name === name && eq(s.hosts, ['api.example.com'], 'hosts') === undefined && !!s.placeholder && !s.placeholder.includes('s3cret'), 'created')
    assert((await c.daytona.secret.get(s.id)).id === s.id, 'get')
    const page = await c.daytona.secret.list({ limit: 100 })
    assert(page.items.some((x: any) => x.name === name) && page.total >= 1, 'list')
    const up = await c.daytona.secret.update(s.id, { value: 'new', hosts: ['*.example.com'] })
    eq(up.hosts, ['*.example.com'], 'updated hosts')
    const dup = await c.raises(() => c.daytona.secret.create({ name, value: 'x' }))
    assert(!!dup && dup.status_code === 409, 'dup')
    const badName = await c.raises(() => c.daytona.secret.create({ name: '1bad', value: 'x' }))
    assert(!!badName && badName.status_code === 400, 'bad name')
    const badHost = await c.raises(() => c.daytona.secret.create({ name: name + 'x', value: 'x', hosts: ['a.com:443'] }))
    assert(!!badHost && badHost.status_code === 400, 'bad host')
    await c.daytona.secret.delete(s.id)
    const gone = await c.raises(() => c.daytona.secret.get(s.id))
    assert(!!gone && gone.status_code === 404, 'gone')
    return { valueNotReturned: true, hosts: ['*.example.com'], duplicate: 409, badName: 400, badHost: 400, afterDelete: 404 }
  } finally {
    await c.raises(() => c.daytona.secret.delete(s.id))
  }
})

testCase('secret.mount_is_opt_in', 'mock', async (c) => {
  const name = c.name('mnt').replace(/-/g, '_')
  const s = await c.daytona.secret.create({ name, value: 'v-123' })
  try {
    const refused = await c.raises(() => c.daytona.create({ secrets: { MY_KEY: name } }))
    assert(!!refused && refused.status_code === 400 && refused.message.includes('cua.secrets=plaintext'), `refused ${JSON.stringify(refused)}`)
    const sb = await c.newSandbox({ secrets: { MY_KEY: name }, labels: { 'cua.secrets': 'plaintext' } })
    assert(sb.env.MY_KEY === 'v-123', `env ${JSON.stringify(sb.env)}`)
    const missing = await c.raises(() => c.daytona.create({ secrets: { K: 'no-such-secret' }, labels: { 'cua.secrets': 'plaintext' } }))
    assert(!!missing && missing.status_code === 404, 'missing secret')
    return { refusedWithoutLabel: errObs(refused), injected: true, missing: errObs(missing) }
  } finally {
    await c.raises(() => c.daytona.secret.delete(s.id))
  }
})

testCase('image.create_with_layers', 'mock', async (c) => {
  const image = Image.debianSlim('3.12').pipInstall(['requests']).runCommands('echo built').env({ X: '1' })
  const sb = await c.newSandbox({ image, labels: { img: c.runId } })
  assert(started(sb), `state ${sb.state}`)
  return { started: true }
})

testCase('image.context_upload', 'mock', async (c) => {
  const d = mkdtempSync(join(tmpdir(), 'dtn-ctx-'))
  mkdirSync(join(d, 'pkg', 'sub'), { recursive: true })
  writeFileSync(join(d, 'pkg', 'a.txt'), 'a')
  writeFileSync(join(d, 'pkg', 'sub', 'b.txt'), 'b')
  writeFileSync(join(d, 'single.txt'), 'single')
  const image = Image.debianSlim('3.12')
    .addLocalFile(join(d, 'single.txt'), '/home/daytona/single.txt')
    .addLocalDir(join(d, 'pkg'), '/home/daytona/pkg')
  const sb = await c.newSandbox({ image })
  assert(started(sb), 'first')
  const sb2 = await c.newSandbox({ image })
  assert(started(sb2), 'second (context already uploaded)')
  return { uploadedAndReused: true }
})

testCase('image.unbuildable_dockerfile_is_refused', 'mock', async (c) => {
  const image = Image.base('python:3.12').dockerfileCommands(['ONBUILD RUN true'])
  const e = await c.raises(() => c.daytona.create({ image }))
  assert(!!e && e.status_code === 400 && e.message.includes('ONBUILD'), `refused ${JSON.stringify(e)}`)
  return { refused: errObs(e) }
})

testCase('warm_pool.crud', 'mock', async (c) => {
  const snap = c.name('wp')
  await c.daytona.snapshot.create({ name: snap, image: 'python:3.12' })
  try {
    const p = await c.daytona.warmPool.create({ snapshot: snap, pool: 2 })
    try {
      assert(p.snapshot === snap && p.pool === 2, 'created')
      assert((await c.daytona.warmPool.list()).some((x: any) => x.id === p.id), 'listed')
      const dup = await c.raises(() => c.daytona.warmPool.create({ snapshot: snap, pool: 1 }))
      assert(!!dup && dup.status_code === 409, 'dup')
      const up = await c.daytona.warmPool.update(p.id, { pool: 3 })
      assert(up.pool === 3, 'updated')
      return { pool: 3, duplicate: 409 }
    } finally {
      await c.daytona.warmPool.delete(p.id)
    }
  } finally {
    await dropSnap(c, snap)
  }
})

testCase('volume.mount_without_a_grant_is_explained', 'space', async (c) => {
  const v = await c.daytona.volume.create(c.name('ng'))
  try {
    const e = await c.raises(() => c.daytona.create({ volumes: [{ volumeId: v.id, mountPath: '/data' }] }, { timeout: 300 }))
    if (e === null) return { mounted: true }
    assert(e.status_code === 400 && e.message.includes('Cua Volume') && e.message.includes('cua volume grant'), `explained ${JSON.stringify(e)}`)
    return { refused: 400, explainsGrant: true }
  } finally {
    await c.raises(() => c.daytona.volume.delete(v))
  }
})

testCase('volume.shared_between_sandboxes', 'space', async (c) => {
  const v = await c.daytona.volume.create(c.name('shr'))
  try {
    const mounts = [{ volumeId: v.id, mountPath: '/data' }]
    const a = await c.newSandbox({ volumes: mounts })
    const w = await a.process.executeCommand('echo shared-by-a > /data/note.txt && cat /data/note.txt')
    assert(w.exitCode === 0 && w.result.trim() === 'shared-by-a', `write ${w.exitCode} ${w.result}`)
    const b = await c.newSandbox({ volumes: mounts })
    const r = await b.process.executeCommand('cat /data/note.txt')
    assert(r.result.trim() === 'shared-by-a', `read ${r.result}`)
    return { shared: true }
  } finally {
    await c.raises(() => c.daytona.volume.delete(v))
  }
}, 180, 'CUA_COMPAT_VOLUME_GRANT')
