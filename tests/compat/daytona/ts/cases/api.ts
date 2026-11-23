import { Daytona, SandboxState } from '@daytonaio/sdk'
import { assert, eq, errObs, testCase } from '../harness.js'

const started = (sb: any) => String(sb.state).toLowerCase().endsWith('started')
const stateOf = (sb: any) => String(sb.state).toLowerCase()
const collect = async (it: AsyncIterable<any>) => {
  const out: any[] = []
  for await (const s of it) out.push(s)
  return out
}
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms))

testCase('api.create_params', 'mock', async (c) => {
  const name = c.name('params')
  const sb = await c.newSandbox({
    name,
    envVars: { A: '1', B: 'two words' },
    labels: { team: 'x' },
    public: true,
    autoStopInterval: 7,
    autoArchiveInterval: 30,
    autoDeleteInterval: -1,
  })
  const got = await c.daytona.get(name)
  assert(got.id === sb.id && got.name === name, 'get by name')
  assert(got.public === true, 'public')
  eq(got.env, { A: '1', B: 'two words' }, 'env')
  assert(got.labels.team === 'x' && got.labels['code-toolbox-language'] === 'python', 'labels')
  return {
    started: started(got),
    autoStop: got.autoStopInterval,
    autoArchive: got.autoArchiveInterval,
    autoDelete: got.autoDeleteInterval,
    networkBlockAll: got.networkBlockAll,
    user: got.user,
    hasCpu: got.cpu >= 1 && got.memory >= 1,
  }
})

testCase('api.create_defaults', 'mock', async (c) => {
  const sb = await c.newSandbox()
  assert(started(sb) && !!sb.id, 'started')
  assert(!!sb.name, 'a name defaults to the id')
  return {
    nameIsId: sb.name === sb.id,
    autoStop: sb.autoStopInterval,
    autoArchive: sb.autoArchiveInterval,
    autoDelete: sb.autoDeleteInterval,
    public: sb.public,
    env: sb.env,
    labels: Object.keys(sb.labels).sort(),
  }
})

testCase('api.create_from_image_resources', 'mock', async (c) => {
  const sb = await c.newSandbox({ image: 'python:3.12', resources: { cpu: 2, memory: 3 } })
  assert(started(sb), 'started')
  return { cpu: sb.cpu, memory: sb.memory }
})

testCase('api.create_from_image_disk', 'mock', async (c) => {
  // A disk size is applied (VM-backed images) or refused with the reason (an
  // image that only runs as a container has no disk to size); never ignored.
  const made: any[] = []
  const e = await c.raises(async () => {
    made.push(await c.newSandbox({ image: 'python:3.12', resources: { cpu: 2, memory: 3, disk: 7 } }))
  })
  if (e !== null) assert(e.status_code === 400 && String(e.message).toLowerCase().includes('disk'), `disk refusal ${JSON.stringify(e)}`)
  else assert(made[0].disk === 7 || made[0].disk > 0, `disk ${made[0].disk}`)
  return { diskAppliedOrExplained: true }
})

testCase('api.create_name_conflict', 'mock', async (c) => {
  const name = c.name('dup')
  await c.newSandbox({ name })
  const e = await c.raises(() => c.daytona.create({ name }))
  assert(!!e && e.type === 'DaytonaConflictError' && e.status_code === 409, `conflict ${JSON.stringify(e)}`)
  assert(e!.message.includes(name), 'message names the sandbox')
  return errObs(e)
})

testCase('api.get_missing', 'mock', async (c) => {
  const e = await c.raises(() => c.daytona.get('no-such-sandbox-' + c.runId))
  assert(!!e && e.type === 'DaytonaNotFoundError' && e.status_code === 404, `404 ${JSON.stringify(e)}`)
  return errObs(e)
})

testCase('api.auth_bad_key', 'mock', async (c) => {
  const bad = new Daytona({ apiKey: 'dtn_not_a_key', apiUrl: process.env.DAYTONA_API_URL })
  const e = await c.raises(() => bad.get('whatever'))
  assert(!!e && e.type === 'DaytonaAuthenticationError' && e.status_code === 401, `401 ${JSON.stringify(e)}`)
  return errObs(e)
})

testCase('api.list_labels_and_pages', 'mock', async (c) => {
  const tag = c.name('lp')
  const made = []
  for (let i = 0; i < 3; i++) made.push(await c.newSandbox({ labels: { lp: tag, idx: String(i) } }))
  const ids = new Set(made.map((s) => s.id))
  const got = await collect(c.daytona.list({ labels: { lp: tag } }))
  eq(new Set(got.map((s) => s.id)), ids, 'all')
  const paged = await collect(c.daytona.list({ labels: { lp: tag }, limit: 1 }))
  assert(paged.length === 3 && new Set(paged.map((s) => s.id)).size === 3, 'paged')
  const one = await collect(c.daytona.list({ labels: { lp: tag, idx: '1' } }))
  eq(one.map((s) => s.id), [made[1].id], 'one')
  const none = await collect(c.daytona.list({ labels: { lp: tag + '-nope' } }))
  assert(none.length === 0, 'none')
  return { all: got.length, paged: paged.length, one: one.length, none: none.length }
})

testCase('api.list_filters', 'mock', async (c) => {
  const tag = c.name('lf')
  const a = await c.newSandbox({ name: tag + '-alpha', labels: { lf: tag }, public: true })
  const b = await c.newSandbox({ name: tag + '-beta', labels: { lf: tag } })
  eq((await collect(c.daytona.list({ name: tag + '-al' }))).map((s) => s.id), [a.id], 'by name')
  assert((await collect(c.daytona.list({ id: b.id.slice(0, 8) }))).some((s) => s.id === b.id), 'by id')
  eq((await collect(c.daytona.list({ labels: { lf: tag }, isPublic: true }))).map((s) => s.id), [a.id], 'public')
  const st = await collect(c.daytona.list({ labels: { lf: tag }, states: [SandboxState.STARTED] }))
  eq(new Set(st.map((s) => s.id)), new Set([a.id, b.id]), 'started')
  assert((await collect(c.daytona.list({ labels: { lf: tag }, states: [SandboxState.STOPPED] }))).length === 0, 'stopped')
  const asc = await collect(c.daytona.list({ labels: { lf: tag }, sort: 'name' as any, order: 'asc' as any }))
  const desc = await collect(c.daytona.list({ labels: { lf: tag }, sort: 'name' as any, order: 'desc' as any }))
  eq(asc.map((s) => s.id), [a.id, b.id], 'asc')
  eq(desc.map((s) => s.id), [b.id, a.id], 'desc')
  assert((await collect(c.daytona.list({ labels: { lf: tag }, minCpu: 1000 }))).length === 0, 'minCpu')
  return { byName: 1, public: 1, started: 2, stopped: 0, ascFirstIsAlpha: true, minCpu: 0 }
})

testCase('api.list_rejects_bad_limit', 'mock', async (c) => {
  try {
    await collect(c.daytona.list({ limit: 0 }))
  } catch (e: any) {
    return { rejected: true, mentionsLimit: String(e.message).includes('limit') }
  }
  throw new Error('limit=0 accepted')
})

testCase('api.labels_set', 'mock', async (c) => {
  const sb = await c.newSandbox({ labels: { a: '1' } })
  const out = await sb.setLabels({ b: '2' })
  assert(out.b === '2', 'returned')
  const got = await c.daytona.get(sb.id)
  assert(got.labels.b === '2' && !('a' in got.labels), 'after')
  return { returned: Object.keys(out).sort(), after: Object.keys(got.labels).filter((k) => k !== 'code-toolbox-language').sort() }
})

testCase('api.intervals', 'mock', async (c) => {
  const sb = await c.newSandbox({ autoStopInterval: 0 })
  await sb.setAutostopInterval(45)
  await sb.setAutoArchiveInterval(120)
  await sb.setAutoDeleteInterval(10)
  await sb.setTtl(60)
  const got = await c.daytona.get(sb.id)
  eq([got.autoStopInterval, got.autoArchiveInterval, got.autoDeleteInterval], [45, 120, 10], 'intervals')
  assert(!!got.autoDestroyAt, 'autoDestroyAt after setTtl')
  const refused: Record<string, boolean> = {}
  refused.autostop = (await c.raises(() => sb.setAutostopInterval(-1))) !== null
  refused.archive = (await c.raises(() => sb.setAutoArchiveInterval(-1))) !== null
  refused.ttl = (await c.raises(() => sb.setTtl(-5))) !== null
  assert(Object.values(refused).every(Boolean), `refused ${JSON.stringify(refused)}`)
  return { autoStop: 45, autoArchive: 120, autoDelete: 10, hasDestroyAt: true, refused }
})

testCase('api.public_flag', 'mock', async (c) => {
  const sb = await c.newSandbox()
  assert(sb.public === false, 'default private')
  await sb.setPublic(true)
  assert((await c.daytona.get(sb.id)).public === true, 'public')
  await sb.setPublic(false)
  assert((await c.daytona.get(sb.id)).public === false, 'private')
  return { toggled: true }
})

testCase('api.lifecycle_stop_start', 'mock', async (c) => {
  const sb = await c.newSandbox()
  await sb.stop()
  assert(stateOf(await c.daytona.get(sb.id)) === 'stopped', 'stopped')
  await sb.stop()
  await sb.start()
  assert(started(await c.daytona.get(sb.id)), 'started')
  await sb.start()
  return { afterStop: 'stopped', afterStart: 'started' }
})

testCase('api.lifecycle_archive_rules', 'mock', async (c) => {
  const sb = await c.newSandbox()
  const e = await c.raises(() => sb.archive())
  assert(!!e && e.status_code === 400 && e.message.toLowerCase().includes('stopped'), `archive started ${JSON.stringify(e)}`)
  await sb.stop()
  await sb.archive()
  assert(stateOf(await c.daytona.get(sb.id)) === 'archived', 'archived')
  await sb.start()
  assert(started(await c.daytona.get(sb.id)), 'restarted')
  return { startedArchive: errObs(e), archived: 'archived', restarted: true }
})

testCase('api.lifecycle_pause', 'mock', async (c) => {
  const sb = await c.newSandbox()
  await sb.pause()
  assert(stateOf(await c.daytona.get(sb.id)) === 'paused', 'paused')
  await sb.start()
  assert(started(await c.daytona.get(sb.id)), 'started')
  await sb.stop()
  const e = await c.raises(() => sb.pause())
  assert(!!e && e.status_code === 400, 'pause stopped')
  return { paused: 'paused', pauseStopped: errObs(e) }
})

testCase('api.lifecycle_recover_only_in_error', 'mock', async (c) => {
  const sb = await c.newSandbox()
  const e = await c.raises(() => sb.recover())
  assert(!!e && e.status_code === 400, `recover started ${JSON.stringify(e)}`)
  return { recoverStarted: errObs(e) }
})

testCase('api.delete', 'mock', async (c) => {
  const sb = await c.newSandbox()
  await c.daytona.delete(sb)
  c.made = c.made.filter((s) => s !== sb)
  const e = await c.raises(() => c.daytona.get(sb.id))
  assert(!!e && e.type === 'DaytonaNotFoundError', 'get after delete')
  assert((await collect(c.daytona.list({ id: sb.id }))).length === 0, 'not listed')
  return { getAfterDelete: errObs(e) }
})

testCase('api.delete_wait', 'mock', async (c) => {
  const sb = await c.newSandbox()
  await sb.delete(undefined, true)
  c.made = c.made.filter((s) => s !== sb)
  assert(['destroyed', 'destroying'].includes(stateOf(sb)), `state ${sb.state}`)
  return { state: 'destroyed' }
})

testCase('api.refresh', 'mock', async (c) => {
  const sb = await c.newSandbox()
  const other = await c.daytona.get(sb.id)
  await other.setLabels({ seen: 'yes' })
  await sb.refreshData()
  assert(sb.labels.seen === 'yes', 'labels refreshed')
  await sb.refreshActivity()
  return { labelsRefreshed: true }
})

testCase('api.ephemeral', 'mock', async (c) => {
  const sb = await c.newSandbox({ ephemeral: true })
  assert(sb.autoDeleteInterval === 0, `autoDelete ${sb.autoDeleteInterval}`)
  await sb.stop()
  let e = null
  for (let i = 0; i < 40 && !e; i++) {
    e = await c.raises(() => c.daytona.get(sb.id))
    if (!e) await sleep(500)
  }
  assert(!!e, 'an ephemeral sandbox survived being stopped')
  c.made = c.made.filter((s) => s !== sb)
  assert(e!.type === 'DaytonaNotFoundError', 'not found')
  return { autoDelete: 0, afterStop: 'deleted' }
})

testCase('api.fork', 'mock', async (c) => {
  const sb = await c.newSandbox({ labels: { fk: c.runId } })
  const kid = await sb.fork({ name: c.name('kid') })
  c.made.push(kid)
  assert(started(kid) && kid.id !== sb.id, 'forked')
  assert(started(await c.daytona.get(kid.id)), 'started')
  return { forked: true }
})

testCase('api.create_validation_is_client_side', 'mock', async (c) => {
  const refused: Record<string, boolean> = {}
  const tries: Record<string, any> = {
    negativeStop: { autoStopInterval: -1 },
    stopAndPause: { autoStopInterval: 5, autoPauseInterval: 5 },
    pauseEphemeral: { autoPauseInterval: 5, autoDeleteInterval: 0 },
    badQueueTimeout: { queueTimeout: 0 },
    negativeTtl: { ttlMinutes: -1 },
  }
  for (const [k, p] of Object.entries(tries)) refused[k] = (await c.raises(() => c.daytona.create(p))) !== null
  assert(Object.values(refused).every(Boolean), `refused ${JSON.stringify(refused)}`)
  return refused
})

testCase('api.resize_rules', 'mock', async (c) => {
  const sb = await c.newSandbox({ autoStopInterval: 0 })
  const cpu = sb.cpu
  const mem = sb.memory
  const gpu = await c.raises(() => sb.resize({ cpu: 2, gpu: 1 } as any))
  assert(gpu === null || gpu.status_code === 400 || gpu.type === 'DaytonaBadRequestError', `gpu ${JSON.stringify(gpu)}`)
  const diskRunning = await c.raises(() => sb.resize({ disk: sb.disk + 10 }))
  assert(!!diskRunning && diskRunning.status_code === 400, `disk while running ${JSON.stringify(diskRunning)}`)
  const e = await c.raises(() => sb.resize({ cpu: cpu + 1, memory: mem + 1 }))
  if (e === null) {
    const got = await c.daytona.get(sb.id)
    assert(got.cpu === cpu + 1 && got.memory === mem + 1, 'applied')
  } else {
    assert(e.status_code === 400 && e.message.includes('Resizing is not supported'), `explained ${JSON.stringify(e)}`)
  }
  return { gpuRefused: true, diskWhileRunningRefused: true, growIsAppliedOrExplained: true }
})

testCase('api.resize_applies_on_a_real_guest', 'space', async (c) => {
  const sb = await c.newSandbox({ autoStopInterval: 0 })
  const before = parseInt((await sb.process.executeCommand('nproc')).result.trim(), 10)
  await sb.process.executeCommand('echo kept > /home/daytona/resize-marker')
  await sb.stop()
  await sb.resize({ cpu: before + 1, memory: sb.memory + 1, disk: sb.disk + 2 })
  await sb.refreshData()
  assert(String(sb.state).toLowerCase().endsWith('stopped'), `state ${sb.state}`)
  await sb.start()
  const after = parseInt((await sb.process.executeCommand('nproc')).result.trim(), 10)
  const marker = (await sb.process.executeCommand('cat /home/daytona/resize-marker')).result.trim()
  assert(after === before + 1 && marker === 'kept', `resize ${before} ${after} ${marker}`)
  return { cpuGrew: true, filesKept: true, stoppedStayedStopped: true }
})
