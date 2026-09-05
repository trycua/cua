"""Verify a loaded discovery-only plugin in a disposable Hyprland session.

This observes compositor state, not application delivery or foreground grabs.
The optional lifecycle test explicitly unloads and reloads the supplied module.
"""

import argparse
import json
import socket
import stat
import struct
import subprocess
from pathlib import Path

if not __debug__:
    raise RuntimeError("Discovery validation requires assertions; do not use -O or PYTHONOPTIMIZE")

HEADER = struct.Struct("!4sHHHHQI")


def hyprctl(*args):
    return subprocess.check_output(["hyprctl", *args], text=True, timeout=10)


def status():
    value = json.loads(hyprctl("-j", "cua:status"))
    assert value["state"] == "discovery_only"
    assert value["abi"]["match"] is True
    assert value["capabilities"]["supported"] == ["discovery"]
    assert value["capabilities"]["enabled"] == ["discovery"]
    assert value["transport"]["ready"] is True
    assert value["compositor_epoch"] != 0
    assert value["protocol"] == {"major": 2, "minor": 0, "max_frame_bytes": 4120}
    return value


def desktop_state():
    active = json.loads(hyprctl("-j", "activewindow"))
    clients = json.loads(hyprctl("-j", "clients"))
    return {
        "active_window": active.get("address"),
        "active_workspace": json.loads(hyprctl("-j", "activeworkspace")).get("id"),
        "cursor": json.loads(hyprctl("-j", "cursorpos")),
        "windows": sorted(
            (
                c["address"],
                c.get("at"),
                c.get("size"),
                c.get("workspace", {}).get("id"),
                c.get("floating"),
                c.get("mapped"),
                c.get("focusHistoryID"),
            )
            for c in clients
        ),
    }


def exchange(client, message, request_id, payload=b""):
    packet = HEADER.pack(b"CUA2", 2, 0, message, 0, request_id, len(payload)) + payload
    assert client.send(packet) == len(packet)
    response = client.recv(4121)
    assert HEADER.size <= len(response) <= 4120
    magic, major, minor, kind, flags, correlated, size = HEADER.unpack_from(response)
    assert (magic, major, minor, flags, correlated) == (b"CUA2", 2, 0, 0, request_id)
    assert len(response) == HEADER.size + size
    return kind, response[HEADER.size :]


def connect(metadata):
    path = Path(metadata["transport"]["socket"])
    assert stat.S_ISSOCK(path.stat().st_mode)
    assert stat.S_IMODE(path.stat().st_mode) == 0o600
    client = socket.socket(socket.AF_UNIX, socket.SOCK_SEQPACKET)
    client.settimeout(5)
    client.connect(str(path))
    kind, payload = exchange(client, 1, 1, struct.pack("!QQ", 1, 1))
    assert kind == 2
    assert struct.unpack("!QQQII", payload) == (metadata["compositor_epoch"], 1, 1, 4120, 0)
    return client


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--reload-module", type=Path)
    args = parser.parse_args()
    initial = status()
    before = desktop_state()
    with connect(initial) as client:
        assert exchange(client, 3, 2) == (4, b"")
        kind, payload = exchange(client, 5, 3)
        assert kind == 6
        assert struct.unpack("!QQQII", payload) == (initial["compositor_epoch"], 1, 1, 4120, 0)
        for request_id, kind in enumerate((0x100, 0x101, 0x102, 0x103, 0x110, 0x111), 4):
            response_kind, payload = exchange(client, kind, request_id)
            assert response_kind == 0xFFFF and struct.unpack_from("!I", payload)[0] == 8
        assert desktop_state() == before, "refused packets changed compositor state"
        if args.reload_module:
            module = str(args.reload_module.resolve(strict=True))
            assert hyprctl("plugin", "unload", module).strip() == "ok"
            assert client.recv(1) == b"", "unload left the old connection open"
            assert not Path(initial["transport"]["socket"]).exists()
            assert hyprctl("plugin", "load", module).strip() == "ok"
            assert hyprctl("reload").strip() == "ok"
            reloaded = status()
            assert reloaded["compositor_epoch"] != initial["compositor_epoch"]
            with connect(reloaded) as replacement:
                assert exchange(replacement, 3, 2) == (4, b"")
    print(
        json.dumps(
            {
                "result": "passed",
                "mutation_refusals": 6,
                "compositor_state_unchanged": True,
                "module_reload": bool(args.reload_module),
            }
        )
    )


if __name__ == "__main__":
    main()
