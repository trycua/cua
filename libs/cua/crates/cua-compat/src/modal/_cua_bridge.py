"""Function container bridge: runs the container entrypoint and carries its
connections to the Modal gateway over this process's stdin and stdout.

    python3 _cua_bridge.py --port 49985 -- python -m modal._container_entrypoint

The entrypoint dials the gateway at 127.0.0.1:<port>. This process listens
there and relays every connection, as frames on stdout and stdin, to the
gateway on the host, which connects each one to its own API port and nothing
else. The container has no other path to the host.

Frames: type (1 byte), connection (4 bytes), length (4 bytes), payload.
  to the host    OPEN, DATA, CLOSE of a connection; LOG (fd byte + bytes) of
                 the entrypoint's output; EXIT (4 bytes, signed) when it ends
  from the host  DATA, CLOSE of a connection

Standard library only.
"""

import asyncio
import os
import signal
import struct
import sys

HDR = struct.Struct("!BII")
OPEN, DATA, CLOSE, LOG, EXIT = 1, 2, 3, 4, 5


def parse(argv):
    port, i = None, 0
    while i < len(argv):
        if argv[i] == "--port":
            port = int(argv[i + 1])
            i += 2
        elif argv[i] == "--":
            return port, argv[i + 1:]
        else:
            raise SystemExit(f"unknown argument {argv[i]!r}")
    raise SystemExit("usage: _cua_bridge.py --port N -- CMD ...")


async def main(port, cmd):
    loop = asyncio.get_running_loop()
    out = sys.stdout.buffer
    lock = asyncio.Lock()
    conns = {}
    next_id = [0]

    async def send(kind, conn, payload=b""):
        async with lock:
            out.write(HDR.pack(kind, conn, len(payload)) + payload)
            out.flush()

    async def serve(reader, writer):
        next_id[0] += 1
        cid = next_id[0]
        conns[cid] = writer
        await send(OPEN, cid)
        try:
            while True:
                data = await reader.read(65536)
                if not data:
                    break
                await send(DATA, cid, data)
        except (ConnectionError, OSError):
            pass
        finally:
            if conns.pop(cid, None) is not None:
                await send(CLOSE, cid)
            writer.close()

    server = await asyncio.start_server(serve, "127.0.0.1", port)

    child = await asyncio.create_subprocess_exec(
        *cmd,
        stdin=asyncio.subprocess.DEVNULL,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
    )

    async def pump(stream, fd):
        while True:
            data = await stream.read(65536)
            if not data:
                return
            await send(LOG, 0, bytes([fd]) + data)

    pumps = asyncio.gather(pump(child.stdout, 0), pump(child.stderr, 1))

    reader = asyncio.StreamReader()
    await loop.connect_read_pipe(lambda: asyncio.StreamReaderProtocol(reader), sys.stdin)

    async def from_host():
        try:
            while True:
                head = await reader.readexactly(HDR.size)
                kind, cid, n = HDR.unpack(head)
                payload = await reader.readexactly(n) if n else b""
                w = conns.get(cid)
                if kind == DATA and w is not None:
                    w.write(payload)
                elif kind == CLOSE:
                    w = conns.pop(cid, None)
                    if w is not None:
                        w.close()
        except (asyncio.IncompleteReadError, ConnectionError):
            pass
        # The host went away: stop the entrypoint.
        if child.returncode is None:
            child.terminate()

    host = asyncio.ensure_future(from_host())
    for sig in (signal.SIGTERM, signal.SIGINT):
        loop.add_signal_handler(sig, lambda: child.returncode is None and child.terminate())

    code = await child.wait()
    await pumps
    await send(EXIT, 0, struct.pack("!i", code))
    server.close()
    host.cancel()
    for w in list(conns.values()):
        w.close()
    return 0


if __name__ == "__main__":
    p, c = parse(sys.argv[1:])
    sys.exit(asyncio.run(main(p, c)))
