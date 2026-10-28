"""Job services, member side: listen on the guest's loopback for each
service port and relay every connection through the Space's hotspot (a
SOCKS5 listener on guest loopback) to the same port on the job's host,
then run the member's command and exit with its status.

    python3 _cua_ports.py --socks 127.0.0.1:1080 --ports 49982,49443 -- CMD ...

Standard library only (it runs before the job's own environment exists).
Names under `.localhost` resolve to loopback for Python programs it starts,
when the guest's resolver does not already do it.
"""

import asyncio
import os
import signal
import socket
import struct
import sys
import tempfile

SITECUSTOMIZE = '''
import socket as _s
_orig = _s.getaddrinfo
def _gai(host, *a, **k):
    if isinstance(host, (str, bytes)):
        h = host.decode() if isinstance(host, bytes) else host
        if h == "localhost" or h.endswith(".localhost") or h.endswith(".localhost."):
            host = "127.0.0.1"
    return _orig(host, *a, **k)
_s.getaddrinfo = _gai
'''


def parse(argv):
    socks, ports, i = None, [], 0
    while i < len(argv):
        a = argv[i]
        if a == "--socks":
            socks = argv[i + 1]
            i += 2
        elif a == "--ports":
            ports = [int(p) for p in argv[i + 1].split(",") if p]
            i += 2
        elif a == "--":
            return socks, ports, argv[i + 1 :]
        else:
            raise SystemExit(f"_cua_ports: unknown argument {a}")
    raise SystemExit("_cua_ports: missing -- COMMAND")


async def socks_connect(socks, port):
    host, sport = socks.rsplit(":", 1)
    r, w = await asyncio.open_connection(host, int(sport))
    w.write(b"\x05\x01\x00")
    await w.drain()
    if await r.readexactly(2) != b"\x05\x00":
        raise ConnectionError("SOCKS5 greeting refused")
    addr = b"127.0.0.1"
    w.write(b"\x05\x01\x00\x03" + bytes([len(addr)]) + addr + struct.pack(">H", port))
    await w.drain()
    head = await r.readexactly(4)
    if head[1] != 0:
        raise ConnectionError(f"SOCKS5 connect to port {port} failed ({head[1]})")
    kind = head[3]
    skip = 4 if kind == 1 else 16 if kind == 4 else (await r.readexactly(1))[0]
    await r.readexactly(skip + 2)
    return r, w


async def pipe(r, w):
    try:
        while True:
            b = await r.read(65536)
            if not b:
                break
            w.write(b)
            await w.drain()
    except (ConnectionError, asyncio.CancelledError):
        pass
    finally:
        try:
            w.close()
        except Exception:
            pass


def serve(socks, port):
    async def handle(cr, cw):
        try:
            ur, uw = await socks_connect(socks, port)
        except Exception as e:  # noqa: BLE001
            print(f"_cua_ports: {e}", file=sys.stderr)
            cw.close()
            return
        await asyncio.gather(pipe(cr, uw), pipe(ur, cw))

    return asyncio.start_server(handle, "127.0.0.1", port)


def resolves_localhost():
    try:
        socket.getaddrinfo("cua-probe.localhost", 80)
        return True
    except OSError:
        return False


async def main():
    socks, ports, cmd = parse(sys.argv[1:])
    if not cmd:
        raise SystemExit("_cua_ports: empty command")
    servers = [await serve(socks, p) for p in ports]
    env = dict(os.environ)
    if not resolves_localhost():
        d = tempfile.mkdtemp(prefix="cua-ports-")
        with open(os.path.join(d, "sitecustomize.py"), "w") as f:
            f.write(SITECUSTOMIZE)
        env["PYTHONPATH"] = d + (os.pathsep + env["PYTHONPATH"] if env.get("PYTHONPATH") else "")
    proc = await asyncio.create_subprocess_exec(*cmd, env=env)
    loop = asyncio.get_running_loop()
    for sig in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
        loop.add_signal_handler(sig, lambda s=sig: proc.returncode is None and proc.send_signal(s))
    code = await proc.wait()
    for s in servers:
        s.close()
    return code if code >= 0 else 128 - code


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
