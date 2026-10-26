"""SSH access: time-limited endpoint plus one-time key (needs sshd in the guest).

Derived from daytona 0.225.0 `_sync/sandbox.py` create_ssh_access /
validate_ssh_access / revoke_ssh_access and the SshAccessDto models."""

import shlex
import subprocess

from harness import case, err_obs


@case(
    "ssh.access_token_lifecycle",
    needs="space",
    ref="_sync/sandbox.py create_ssh_access/validate_ssh_access/revoke_ssh_access",
)
async def lifecycle(c):
    sb = await c.main()
    a = await c.w(sb.create_ssh_access(expires_in_minutes=5))
    assert a.sandbox_id == sb.id and a.token and a.ssh_command.startswith("ssh "), a
    assert a.expires_at > a.created_at
    v = await c.w(sb.validate_ssh_access(a.token))
    assert v.valid is True and v.sandbox_id == sb.id, v
    await c.w(sb.revoke_ssh_access(a.token))
    v2 = await c.w(sb.validate_ssh_access(a.token))
    assert v2.valid is False, v2
    # The Python client sends the token where the API reads the organization
    # (so it revokes every access of the sandbox); the TypeScript client sends
    # it as the token. Revoking again is never a server error.
    e = await c.raises(lambda: sb.revoke_ssh_access(a.token))
    assert e is None or e["status_code"] == 404, e
    return {"valid": True, "afterRevoke": False}


@case(
    "ssh.login_through_the_endpoint",
    needs="space",
    ref="Cua: sshCommand reaches the guest's sshd",
    requires="CUA_COMPAT_GATEWAY_LOCAL",
    why="the ssh endpoint is a loopback port on the gateway's machine: this run is not on it",
)
async def login(c):
    sb = await c.main()
    a = await c.w(sb.create_ssh_access(expires_in_minutes=5))
    try:
        argv = shlex.split(a.ssh_command) + ["-o", "BatchMode=yes", "-o", "ConnectTimeout=20"]
        out = subprocess.run(
            argv + ["echo ssh-ok; whoami"], capture_output=True, text=True, timeout=60
        )
        assert out.returncode == 0 and "ssh-ok" in out.stdout, (
            out.returncode,
            out.stdout,
            out.stderr,
        )
        assert out.stdout.split()[-1] == "daytona", out.stdout
    finally:
        await c.w(sb.revoke_ssh_access(a.token))
    after = subprocess.run(
        shlex.split(a.ssh_command) + ["-o", "BatchMode=yes", "-o", "ConnectTimeout=10", "true"],
        capture_output=True,
        text=True,
        timeout=60,
    )
    assert after.returncode != 0, "the revoked endpoint still accepted a login"
    return {"login": "ssh-ok", "user": "daytona", "afterRevoke": "refused"}
