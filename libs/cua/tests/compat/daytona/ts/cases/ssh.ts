import { assert, errObs, testCase } from '../harness.js'

testCase('ssh.access_token_lifecycle', 'space', async (c) => {
  const sb = await c.main()
  const a = await sb.createSshAccess(5)
  assert(a.sandboxId === sb.id && !!a.token && a.sshCommand.startsWith('ssh '), `access ${JSON.stringify(a)}`)
  assert(new Date(a.expiresAt) > new Date(a.createdAt), 'expiry after creation')
  const v = await sb.validateSshAccess(a.token)
  assert(v.valid === true && v.sandboxId === sb.id, 'valid')
  await sb.revokeSshAccess(a.token)
  assert((await sb.validateSshAccess(a.token)).valid === false, 'revoked')
  const e = await c.raises(() => sb.revokeSshAccess(a.token))
  assert(e === null || e.status_code === 404, `revoke twice ${JSON.stringify(e)}`)
  return { valid: true, afterRevoke: false }
})

testCase('ssh.login_through_the_endpoint', 'space', async (c) => {
  const { spawnSync } = await import('node:child_process')
  const sb = await c.main()
  const a = await sb.createSshAccess(5)
  const words = a.sshCommand.match(/(?:[^\s"]+|"[^"]*")+/g)!.map((w: string) => w.replace(/^"|"$/g, ''))
  try {
    const out = spawnSync(words[0], [...words.slice(1), '-o', 'BatchMode=yes', '-o', 'ConnectTimeout=20', 'echo ssh-ok; whoami'], { encoding: 'utf8', timeout: 60000 })
    assert(out.status === 0 && out.stdout.includes('ssh-ok'), `login ${out.status} ${out.stdout} ${out.stderr}`)
    assert(out.stdout.trim().split(/\s+/).pop() === 'daytona', 'user')
  } finally {
    await sb.revokeSshAccess(a.token)
  }
  const after = spawnSync(words[0], [...words.slice(1), '-o', 'BatchMode=yes', '-o', 'ConnectTimeout=10', 'true'], { encoding: 'utf8', timeout: 60000 })
  assert(after.status !== 0, 'the revoked endpoint still accepted a login')
  return { login: 'ssh-ok', user: 'daytona', afterRevoke: 'refused' }
}, 180, 'CUA_COMPAT_GATEWAY_LOCAL')
