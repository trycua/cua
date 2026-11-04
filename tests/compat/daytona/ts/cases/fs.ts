import { createHash, randomBytes } from 'node:crypto'
import { mkdtempSync, readFileSync, rmSync, statSync, unlinkSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { Readable } from 'node:stream'
import { assert, eq, errObs, testCase } from '../harness.js'

const PAYLOAD = Buffer.concat(Array.from({ length: 1031 }, () => Buffer.from(Array.from({ length: 256 }, (_, i) => i))))
const sha = (b: Buffer) => createHash('sha256').update(b).digest('hex')

testCase('fs.roundtrip_binary_and_empty', 'mock', async (c) => {
  const sb = await c.main()
  await sb.fs.uploadFile(PAYLOAD, '/tmp/fsrt/bin.dat')
  const got = await sb.fs.downloadFile('/tmp/fsrt/bin.dat')
  assert(Buffer.compare(got, PAYLOAD) === 0, 'binary payload')
  await sb.fs.uploadFile(Buffer.alloc(0), '/tmp/fsrt/empty')
  assert((await sb.fs.downloadFile('/tmp/fsrt/empty')).length === 0, 'empty')
  const text = Buffer.from('héllo wörld ✓')
  await sb.fs.uploadFile(text, '/tmp/fsrt/uni.txt')
  assert(Buffer.compare(await sb.fs.downloadFile('/tmp/fsrt/uni.txt'), text) === 0, 'unicode')
  return { binary: got.length, sha: sha(got).slice(0, 16), empty: 0, unicode: text.length }
})

testCase('fs.unicode_and_space_names', 'mock', async (c) => {
  const sb = await c.main()
  const names = ['naïve café.txt', 'with space.txt', 'plus+sign&amp=eq.txt', '日本語.txt']
  for (const n of names) await sb.fs.uploadFile(Buffer.from(n), `/tmp/fsuni/${n}`)
  const listed = (await sb.fs.listFiles('/tmp/fsuni')).map((f: any) => f.name).sort()
  eq(listed, [...names].sort(), 'listed')
  for (const n of names) assert((await sb.fs.downloadFile(`/tmp/fsuni/${n}`)).toString() === n, n)
  return { names: listed.length }
})

testCase('fs.upload_creates_parents_and_overwrites', 'mock', async (c) => {
  const sb = await c.main()
  await sb.fs.uploadFile(Buffer.from('one'), '/tmp/fsp/a/b/c/f.txt')
  await sb.fs.uploadFile(Buffer.from('two'), '/tmp/fsp/a/b/c/f.txt')
  assert((await sb.fs.downloadFile('/tmp/fsp/a/b/c/f.txt')).toString() === 'two', 'overwritten')
  return { overwritten: true }
})

testCase('fs.list_info_folder_move_delete', 'mock', async (c) => {
  const sb = await c.main()
  await sb.fs.createFolder('/tmp/fsl', '755')
  await sb.fs.uploadFile(Buffer.from('payload'), '/tmp/fsl/a.txt')
  const info = await sb.fs.getFileDetails('/tmp/fsl/a.txt')
  assert(info.name === 'a.txt' && info.size === 7 && !info.isDir, 'info')
  assert((await sb.fs.getFileDetails('/tmp/fsl')).isDir, 'dir')
  eq((await sb.fs.listFiles('/tmp/fsl')).map((f: any) => f.name).sort(), ['a.txt'], 'list')
  await sb.fs.moveFiles('/tmp/fsl/a.txt', '/tmp/fsl/b.txt')
  assert((await sb.fs.downloadFile('/tmp/fsl/b.txt')).toString() === 'payload', 'moved')
  const e = await c.raises(() => sb.fs.getFileDetails('/tmp/fsl/a.txt'))
  assert(!!e && e.code === 'FILE_NOT_FOUND', `old path ${JSON.stringify(e)}`)
  await sb.fs.deleteFile('/tmp/fsl/b.txt')
  const e2 = await c.raises(() => sb.fs.downloadFile('/tmp/fsl/b.txt'))
  assert(!!e2 && e2.code === 'FILE_NOT_FOUND', 'after delete')
  return { size: 7, movedFrom: errObs(e), afterDelete: errObs(e2) }
})

testCase('fs.errors_carry_file_codes', 'mock', async (c) => {
  const sb = await c.main()
  const out: Record<string, any> = {}
  const tries: Record<string, () => Promise<unknown>> = {
    download: () => sb.fs.downloadFile('/tmp/does-not-exist'),
    info: () => sb.fs.getFileDetails('/tmp/does-not-exist'),
    delete: () => sb.fs.deleteFile('/tmp/does-not-exist'),
    move: () => sb.fs.moveFiles('/tmp/does-not-exist', '/tmp/x'),
  }
  for (const [name, fn] of Object.entries(tries)) {
    const e = await c.raises(fn)
    assert(!!e && e.type === 'DaytonaFileNotFoundError' && e.status_code === 404, `${name} ${JSON.stringify(e)}`)
    assert(e!.code === 'FILE_NOT_FOUND' && e!.source === 'DAYTONA_DAEMON', `${name} code`)
    out[name] = errObs(e)
  }
  return out
})

testCase('fs.bulk_upload_download', 'mock', async (c) => {
  const sb = await c.main()
  const files: Record<string, Buffer> = {}
  for (let i = 0; i < 30; i++) files[`/tmp/fsb/d${i % 3}/f${i}.bin`] = Buffer.alloc(i * 100 + 1, i)
  await sb.fs.uploadFiles(Object.entries(files).map(([destination, source]) => ({ source, destination })))
  const reqs = [...Object.keys(files).map((source) => ({ source })), { source: '/tmp/fsb/missing' }]
  const res = await sb.fs.downloadFiles(reqs)
  assert(res.length === 31, 'count')
  for (const r of res.slice(0, 30)) assert(!r.error && Buffer.compare(r.result as Buffer, files[r.source]) === 0, r.source)
  const miss = res[30]
  assert(!!miss.error && miss.result === undefined, `missing ${JSON.stringify(miss)}`)
  const dir = mkdtempSync(join(tmpdir(), 'dtn-fs-'))
  const dest = join(dir, 'sub', 'out.bin')
  const r = await sb.fs.downloadFiles([{ source: '/tmp/fsb/d0/f0.bin', destination: dest }])
  assert(r[0].result === dest && Buffer.compare(readFileSync(dest), files['/tmp/fsb/d0/f0.bin']) === 0, 'local destination')
  rmSync(dir, { recursive: true, force: true })
  return { ok: 30, missingHasError: true, missingStructured: miss.errorDetails?.code ?? null }
})

testCase('fs.download_to_local_path_and_stream', 'mock', async (c) => {
  const sb = await c.main()
  const blob = randomBytes(3 * 1024 * 1024 + 17)
  await sb.fs.uploadFile(blob, '/tmp/fss/blob.bin')
  const dir = mkdtempSync(join(tmpdir(), 'dtn-fs-'))
  const dest = join(dir, 'blob.bin')
  await sb.fs.downloadFile('/tmp/fss/blob.bin', dest)
  assert(Buffer.compare(readFileSync(dest), blob) === 0, 'local path')
  rmSync(dir, { recursive: true, force: true })
  const chunks: Buffer[] = []
  let last = 0
  const stream: Readable = await sb.fs.downloadFileStream('/tmp/fss/blob.bin', { onProgress: (p: any) => (last = p.bytesReceived) } as any)
  for await (const ch of stream) chunks.push(ch as Buffer)
  assert(Buffer.compare(Buffer.concat(chunks), blob) === 0 && chunks.length >= 2, `chunks ${chunks.length}`)
  assert(last === blob.length, `progress ${last}`)
  const e = await c.raises(async () => {
    const s: Readable = await sb.fs.downloadFileStream('/tmp/fss/none')
    for await (const _ of s) void _
  })
  assert(!!e && e.code === 'FILE_NOT_FOUND', `missing stream ${JSON.stringify(e)}`)
  return { bytes: blob.length, multipleChunks: true, progressTotal: true, missing: errObs(e) }
})

testCase('fs.upload_stream', 'mock', async (c) => {
  const sb = await c.main()
  const blob = randomBytes(2 * 1024 * 1024 + 5)
  let sent = 0
  await sb.fs.uploadFileStream(Readable.from([blob]), '/tmp/fsu/s.bin', { onProgress: (p: any) => (sent = p.bytesSent) } as any)
  assert(Buffer.compare(await sb.fs.downloadFile('/tmp/fsu/s.bin'), blob) === 0, 'content')
  assert(sent === blob.length, `progress ${sent}`)
  return { bytes: blob.length, progress: true }
})

testCase('fs.signed_urls', 'mock', async (c) => {
  const sb = await c.main()
  await sb.fs.uploadFile(Buffer.from('signed-payload'), '/tmp/fsg/s.txt')
  const url = await sb.downloadUrl('/tmp/fsg/s.txt', 60)
  const ok = await fetch(url)
  assert(ok.status === 200 && (await ok.text()) === 'signed-payload', 'download')
  const bad = url.slice(0, -3) + (url.endsWith('aaa') ? 'bbb' : 'aaa')
  assert((await fetch(bad)).status === 403, 'tampered')
  const up = await sb.uploadUrl('/tmp/fsg/up.txt', 60)
  const r = await fetch(up, { method: 'POST', body: 'via-signed-upload' })
  assert(r.status === 200, `upload ${r.status}`)
  assert((await sb.fs.downloadFile('/tmp/fsg/up.txt')).toString() === 'via-signed-upload', 'uploaded')
  await sb.rotateSigningKey()
  assert((await fetch(url)).status === 403, 'rotated')
  return { download: true, tampered: 403, upload: true, rotated: 403 }
})

testCase('fs.concurrent_transfers', 'mock', async (c) => {
  const sb = await c.main()
  const blobs = Array.from({ length: 16 }, (_, i) => [`/tmp/fsc/${i}.bin`, randomBytes(50_000 + i)] as const)
  await Promise.all(blobs.map(([k, v]) => sb.fs.uploadFile(v, k)))
  const got = await Promise.all(blobs.map(([k]) => sb.fs.downloadFile(k)))
  assert(got.every((g, i) => Buffer.compare(g, blobs[i][1]) === 0), 'all equal')
  return { files: got.length }
})

testCase('fs.large_file_integrity', 'space', async (c) => {
  const sb = await c.main()
  const size = 96 * 1024 * 1024
  const dir = mkdtempSync(join(tmpdir(), 'dtn-big-'))
  const src = join(dir, 'big.bin')
  const h = createHash('sha256')
  const parts: Buffer[] = []
  for (let i = 0; i < size / (1024 * 1024); i++) {
    const b = randomBytes(1024 * 1024)
    h.update(b)
    parts.push(b)
  }
  writeFileSync(src, Buffer.concat(parts))
  try {
    await sb.fs.uploadFile(src, '/tmp/fsbig/big.bin')
    const r = await sb.process.executeCommand('sha256sum /tmp/fsbig/big.bin')
    const want = h.digest('hex')
    assert(r.result.split(/\s+/)[0] === want, `remote sha ${r.result}`)
    const got = createHash('sha256')
    const s: Readable = await sb.fs.downloadFileStream('/tmp/fsbig/big.bin')
    for await (const ch of s) got.update(ch as Buffer)
    assert(got.digest('hex') === want, 'stream sha')
    assert(statSync(src).size === size, 'size')
  } finally {
    unlinkSync(src)
    rmSync(dir, { recursive: true, force: true })
  }
  return { mib: size / (1024 * 1024), sha256Matches: true }
})

testCase('fs.permissions_and_owner', 'space', async (c) => {
  const sb = await c.main()
  await sb.fs.uploadFile(Buffer.from('x'), '/tmp/fsperm/f.txt')
  await sb.fs.createFolder('/tmp/fsperm/dir', '700')
  assert((await sb.fs.getFileDetails('/tmp/fsperm/dir')).permissions!.endsWith('700'), 'dir mode')
  await sb.fs.setFilePermissions('/tmp/fsperm/f.txt', { mode: '640' })
  const f = await sb.fs.getFileDetails('/tmp/fsperm/f.txt')
  assert(f.permissions!.endsWith('640') && f.owner === 'daytona', `file ${JSON.stringify(f)}`)
  return { dir: '700', file: '640', owner: f.owner }
})

testCase('fs.find_search_replace', 'space', async (c) => {
  const sb = await c.main()
  await sb.fs.uploadFiles([
    { source: Buffer.from('alpha\nneedle one\nomega\n'), destination: '/tmp/fsfs/a.txt' },
    { source: Buffer.from('nothing here\nneedle two\n'), destination: '/tmp/fsfs/sub/b.txt' },
    { source: Buffer.from('x'), destination: '/tmp/fsfs/sub/c.log' },
  ])
  const found = await sb.fs.findFiles('/tmp/fsfs', 'needle')
  const got = found.map((m: any) => [m.file.split('/').pop(), m.line, m.content.trim()]).sort()
  eq(got, [['a.txt', 2, 'needle one'], ['b.txt', 2, 'needle two']], 'find')
  const s = await sb.fs.searchFiles('/tmp/fsfs', '*.txt')
  eq(s.files.map((f: string) => f.split('/').pop()).sort(), ['a.txt', 'b.txt'], 'search')
  const rep = await sb.fs.replaceInFiles(['/tmp/fsfs/a.txt', '/tmp/fsfs/sub/b.txt', '/tmp/fsfs/nope.txt'], 'needle', 'thread')
  const ok: Record<string, boolean> = {}
  for (const r of rep) ok[r.file!.split('/').pop()!] = !!r.success
  assert(ok['a.txt'] && ok['b.txt'] && !ok['nope.txt'], `replace ${JSON.stringify(ok)}`)
  assert((await sb.fs.downloadFile('/tmp/fsfs/a.txt')).toString().includes('thread one'), 'replaced')
  return { found: got, search: ['a.txt', 'b.txt'], replaced: ok }
})
