import { assert, errObs, testCase } from '../harness.js'

const PNG = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a])
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms))

testCase('computer.start_status_and_processes', 'mock', async (c) => {
  const sb = await c.main()
  const cu = sb.computerUse
  const started = await cu.start()
  const names = Object.keys(started.status ?? {}).sort()
  assert(JSON.stringify(names) === JSON.stringify(['novnc', 'x11vnc', 'xfce4', 'xvfb']), `names ${names}`)
  assert(Object.values(started.status!).every((s: any) => s.running), 'running')
  assert((await cu.getStatus()).status === 'active', 'active')
  const st = await cu.getProcessStatus('xvfb')
  assert(st.processName === 'xvfb' && st.running === true, 'process status')
  assert((await cu.restartProcess('novnc')).processName === 'novnc', 'restart')
  assert((await cu.getProcessLogs('x11vnc')).processName === 'x11vnc', 'logs')
  assert((await cu.getProcessErrors('xfce4')).errors === '', 'errors')
  const stopped = await cu.stop()
  assert(Object.values(stopped.status!).every((s: any) => s.running), 'the desktop keeps running')
  const e = await c.raises(() => cu.getProcessStatus('no-such-process'))
  assert(!!e && e.type === 'DaytonaProcessNotFoundError' && e.status_code === 404, `unknown ${JSON.stringify(e)}`)
  return { processes: names, status: 'active', stopKeepsRunning: true, unknown: errObs(e) }
})

testCase('computer.mouse', 'mock', async (c) => {
  const sb = await c.main()
  const m = sb.computerUse.mouse
  const p = await m.move(120, 80)
  assert(p.x === 120 && p.y === 80, 'move')
  assert((await m.getPosition()).x === 120, 'position')
  const r = await m.click(200, 150)
  assert(r.x === 200 && r.y === 150, 'click')
  await m.click(210, 160, 'right')
  await m.click(220, 170, 'left', true)
  await m.click(230, 180, 'left', false, 3, ['ctrl', 'shift'])
  const d = await m.drag(10, 10, 300, 200)
  assert(d.x === 300 && d.y === 200, 'drag')
  assert((await m.scroll(100, 100, 'down', 5)) === true, 'scroll')
  await m.scroll(100, 100, 'up', 2)
  await m.down(5, 6, 'left')
  const up = await m.up(7, 8, 'left')
  assert(up.x === 7 && up.y === 8, 'up')
  const e = await c.raises(() => m.click(1, 1, 'laser'))
  assert(!!e && e.status_code === 400, 'bad button')
  const e2 = await c.raises(() => m.scroll(1, 1, 'sideways' as any, 1))
  assert(!!e2 && e2.status_code === 400, 'bad direction')
  return { moved: [120, 80], clicked: [200, 150], dragEnd: [300, 200], up: [7, 8], badButton: errObs(e), badDirection: errObs(e2) }
})

testCase('computer.keyboard', 'mock', async (c) => {
  const sb = await c.main()
  const k = sb.computerUse.keyboard
  await k.type('hello world')
  await k.type('slow', 10)
  await k.press('Return')
  await k.press('a', ['ctrl'])
  await k.hotkey('ctrl+shift+t')
  await k.down('shift')
  await k.up('shift')
  const e = await c.raises(() => k.press('no-such-key'))
  assert(!!e && e.status_code === 400, 'bad key')
  const e2 = await c.raises(() => k.hotkey(''))
  assert(!!e2 && e2.status_code === 400, 'empty hotkey')
  return { typed: true, badKey: errObs(e), emptyHotkey: errObs(e2) }
})

testCase('computer.screenshots', 'mock', async (c) => {
  const sb = await c.main()
  const s = sb.computerUse.screenshot
  const full = await s.takeFullScreen(true)
  const raw = Buffer.from(full.screenshot!, 'base64')
  assert(raw.subarray(0, 8).equals(PNG), 'png')
  assert(full.sizeBytes === raw.length && !!full.cursorPosition, 'metadata')
  const reg = await s.takeRegion({ x: 10, y: 20, width: 300, height: 200 })
  assert(Buffer.from(reg.screenshot!, 'base64').subarray(0, 8).equals(PNG), 'region')
  const comp = await s.takeCompressed({ format: 'jpeg', quality: 80, scale: 0.5 })
  assert(!!comp.screenshot, 'compressed')
  const creg = await s.takeCompressedRegion({ x: 0, y: 0, width: 100, height: 100 }, { format: 'webp' })
  assert(!!creg.screenshot, 'compressed region')
  const bad = await c.raises(() => s.takeCompressed({ format: 'bmp' }))
  assert(!!bad && bad.status_code === 400, 'bad format')
  return { png: true, region: true, compressed: true, badFormat: errObs(bad) }
})

testCase('computer.display_and_windows', 'mock', async (c) => {
  const sb = await c.main()
  const d = sb.computerUse.display
  const info = await d.getInfo()
  assert(info.displays!.length >= 1 && info.displays![0].isActive === true, 'display')
  assert(info.displays![0].width === 1280 && info.displays![0].height === 800, `size ${JSON.stringify(info.displays![0])}`)
  const ws = await d.getWindows()
  assert(Array.isArray(ws.windows), 'windows')
  return { display: [1280, 800], active: true, windows: true }
})

testCase('computer.accessibility', 'mock', async (c) => {
  const sb = await c.main()
  const a = sb.computerUse.accessibility
  const tree = await a.getTree()
  assert(!!tree.root, 'tree root')
  const found = await a.findNodes({ role: 'button' })
  assert(found.matches !== undefined, 'matches')
  if (found.matches!.length) {
    const nid = found.matches![0].id!
    await a.focusNode(nid)
    await a.invokeNode(nid)
    await a.invokeNode(nid, 'press')
  }
  const gone = await c.raises(() => a.focusNode('no-such:node'))
  assert(!!gone && [404, 503].includes(gone.status_code!), 'unknown node')
  const badRe = await c.raises(() => a.findNodes({ name: '(', nameMatch: 'regex' }))
  assert(!!badRe && badRe.status_code === 400, 'bad regex')
  return { treeRoot: true, unknownNode: gone!.status_code, badRegex: errObs(badRe) }
})

testCase('computer.recording', 'space', async (c) => {
  const sb = await c.main()
  const rec = sb.computerUse.recording
  let started: any
  try {
    started = await rec.start('probe')
  } catch (e: any) {
    const o = (await import('../harness.js')).describeError(e)
    if (o.code === 'RECORDING_FFMPEG_NOT_FOUND') {
      assert(o.status_code === 503 && o.type === 'DaytonaRecordingFfmpegNotFoundError', `ffmpeg ${JSON.stringify(o)}`)
      return { skipped: 'no ffmpeg on the gateway host' }
    }
    throw e
  }
  await sleep(2500)
  const busy = await c.raises(() => rec.delete(started.id))
  assert(!!busy && busy.code === 'RECORDING_STILL_ACTIVE' && busy.status_code === 409, `busy ${JSON.stringify(busy)}`)
  const done = await rec.stop(started.id)
  assert(done.status === 'completed' && !!done.sizeBytes && done.durationSeconds! >= 2, `done ${JSON.stringify(done)}`)
  const got = await rec.get(started.id)
  assert(got.id === started.id && got.fileName.endsWith('.mp4'), 'get')
  const { mkdtempSync, readFileSync, rmSync } = await import('node:fs')
  const { tmpdir } = await import('node:os')
  const { join } = await import('node:path')
  const dir = mkdtempSync(join(tmpdir(), 'dtn-rec-'))
  const dest = join(dir, 'out.mp4')
  await rec.download(started.id, dest)
  assert(readFileSync(dest).subarray(4, 8).toString() === 'ftyp', 'mp4')
  rmSync(dir, { recursive: true, force: true })
  await rec.delete(started.id)
  const gone = await c.raises(() => rec.get(started.id))
  assert(!!gone && gone.status_code === 404, 'deleted')
  return { mp4: true, stillActive: errObs(busy), afterDelete: errObs(gone) }
}, 240)

testCase('computer.real_desktop', 'space', async (c) => {
  const sb = await c.main()
  const cu = sb.computerUse
  await cu.start()
  const info = (await cu.display.getInfo()).displays![0]
  const shot = await cu.screenshot.takeFullScreen()
  const raw = Buffer.from(shot.screenshot!, 'base64')
  assert(raw.subarray(0, 8).equals(PNG), 'png')
  assert(raw.readUInt32BE(16) === info.width && raw.readUInt32BE(20) === info.height, `size ${raw.readUInt32BE(16)}x${raw.readUInt32BE(20)} vs ${info.width}x${info.height}`)
  await cu.mouse.move(Math.floor(info.width! / 2), Math.floor(info.height! / 2))
  const pos = await cu.mouse.getPosition()
  assert(Math.abs(pos.x! - Math.floor(info.width! / 2)) <= 2 && Math.abs(pos.y! - Math.floor(info.height! / 2)) <= 2, 'pointer')
  const jpeg = await cu.screenshot.takeCompressed({ format: 'jpeg', quality: 60 })
  assert(Buffer.from(jpeg.screenshot!, 'base64')[0] === 0xff, 'jpeg')
  return { screenshotMatchesDisplay: true, pointerMoved: true, jpeg: true }
}, 240)
