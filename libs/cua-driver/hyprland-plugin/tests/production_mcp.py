"""Normal direct-runtime MCP transport for disposable native production proofs."""
import os
from pathlib import Path
import subprocess

from driver_input_live import MCP


PROFILE_ENV = (
    'CUA_DRIVER_PERMISSION_MODE', 'CUA_DRIVER_DANGEROUSLY_BYPASS_APPROVALS',
    'CUA_DRIVER_CAPABILITY_MANIFEST_FILE', 'CUA_DRIVER_CAPABILITY_MANIFEST_APPROVED',
    'CUA_DRIVER_SESSION_POLICY_FILE', 'CUA_DRIVER_SESSION_POLICY_APPROVED',
)


def profile_environment(profile, inherited=None):
    """Select a reviewed profile; retain inherited managed/user policy ceilings."""
    mode = profile['mode']
    assert mode in ('standard', 'bounded', 'unrestricted')
    manifest = profile.get('manifest')
    assert mode != 'bounded' or manifest, 'bounded needs an approved manifest'
    assert mode != 'unrestricted' or profile.get('acknowledge_unrestricted') is True
    assert not manifest or profile.get('approve_manifest') is True
    env = dict(os.environ if inherited is None else inherited)
    for name in PROFILE_ENV:
        env.pop(name, None)
    env['CUA_DRIVER_PERMISSION_MODE'] = mode
    if mode == 'unrestricted':
        env['CUA_DRIVER_DANGEROUSLY_BYPASS_APPROVALS'] = '1'
    if manifest:
        assert Path(manifest).is_file(), 'manifest does not exist'
        env['CUA_DRIVER_CAPABILITY_MANIFEST_FILE'] = str(Path(manifest).resolve())
        env['CUA_DRIVER_CAPABILITY_MANIFEST_APPROVED'] = '1'
    return env


def stop_process(process):
    """Reap a process owned by this run, escalating only that exact child."""
    if process.poll() is not None:
        process.wait(timeout=5)
        return
    if process.stdin:
        try:
            process.stdin.close()
        except (BrokenPipeError, OSError):
            pass
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)


class DirectMCP(MCP):
    """One process, one private runtime. Never attach to a daemon socket."""

    def __init__(self, driver, directory, profile):
        self.directory = directory
        env = profile_environment(profile)
        self.log = (directory / 'mcp.stderr').open('w')
        self.process = None
        self.counter = 0
        self.failed = False
        self.closed = False
        try:
            self.process = subprocess.Popen(
                [str(driver), 'mcp', '--direct'], env=env,
                stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=self.log,
                text=True, bufsize=1)
            self.rpc('initialize', {'protocolVersion': '2025-06-18', 'capabilities': {},
                                   'clientInfo': {'name': 'production-native-proof', 'version': '1'}})
            self.process.stdin.write('{"jsonrpc":"2.0","method":"notifications/initialized"}\n')
            self.process.stdin.flush()
        except BaseException:
            self.close()
            raise

    def rpc(self, method, params):
        if self.failed or self.closed:
            raise RuntimeError('MCP outcome unknown or transport closed; do not replay')
        try:
            return super().rpc(method, params)
        except BaseException:
            self.failed = True
            raise

    def close(self):
        if self.closed:
            return
        self.closed = True
        try:
            if self.process:
                stop_process(self.process)
        finally:
            self.log.close()


def assert_distinct_runtimes(clients):
    pids = [client.process.pid for client in clients]
    assert len(pids) == len(set(pids)), 'agents share a Driver process'
    assert all(client.process.poll() is None for client in clients), 'Driver process exited'
    return pids
