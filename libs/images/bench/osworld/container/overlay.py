#!/usr/bin/env python3
"""OSWorld container variant: the layer that turns the upstream tree into a
container, plus the image config.

    overlay.py SOURCES_JSON SPACESD OUT_DIR

The base is the upstream disk's userland (``guestfish tar-out``, see
scripts/bench-images/build-qcow2.sh). This layer is assembled from files
only, like the VM recipe: nothing is executed in the guest tree and nothing
the tasks use is installed, upgraded or rebuilt. It adds:

- supervisord 4.3.0 (pure Python, pinned wheel) under /opt/cua/python, run by
  the guest's own python3, as PID 1 instead of systemd;
- Xorg's dummy video driver (Ubuntu 22.04's package, pinned) for the guest's
  own Xorg 21.1, so the container runs the same X server as the VM, at
  1920x1080, without a GPU or a console;
- cua-spacesd and the shared token scripts from libs/images/linux;
- the entrypoint, the desktop starter (Xorg, session D-Bus, GNOME in builtin
  mode) and the supervisord programs (desktop, OSWorld server, spacesd).

Writes OUT_DIR/overlay.tar.gz (+ .json metadata) and OUT_DIR/config.json.
"""

from __future__ import annotations

import io
import json
import sys
import tarfile
import time
import zipfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
BENCH = HERE.parent
REPO = BENCH.parents[3]
sys.path.insert(0, str(REPO / "scripts" / "bench-images"))
import bench  # noqa: E402

DESK = REPO / "libs" / "images" / "linux" / "files"
FILES = HERE / "files"
VM_FILES = BENCH / "vm" / "files"

DUMMY_CONF = """\
# Xorg dummy display for the OSWorld container variant (1920x1080, depth 24).
Section "Device"
    Identifier "cua-dummy"
    Driver "dummy"
    VideoRam 256000
EndSection
Section "Monitor"
    Identifier "cua-monitor"
    HorizSync 5.0-1000.0
    VertRefresh 5.0-200.0
    Modeline "1920x1080" 148.50 1920 2008 2052 2200 1080 1084 1089 1125 +hsync +vsync
EndSection
Section "Screen"
    Identifier "cua-screen"
    Device "cua-dummy"
    Monitor "cua-monitor"
    DefaultDepth 24
    SubSection "Display"
        Depth 24
        Modes "1920x1080"
        Virtual 1920 1080
    EndSubSection
EndSection
Section "ServerFlags"
    Option "DontVTSwitch" "true"
    Option "AllowMouseOpenFail" "true"
    Option "AutoAddDevices" "false"
    # No X screen saver or DPMS: the display never blanks on its own.
    Option "BlankTime" "0"
    Option "StandbyTime" "0"
    Option "SuspendTime" "0"
    Option "OffTime" "0"
EndSection
"""

# The system bus in a container: gVisor on some hosts reports the wrong
# peer credentials, so EXTERNAL auth is rejected and every system-bus client
# fails (gnome-shell exits on its power indicator). Anonymous connections are
# accepted as well; the default policy still applies (nothing privileged
# runs on this bus in the container).
DBUS_SYSTEM_CONF = """\
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<!-- OSWorld container variant (cua): accept anonymous clients as well. -->
<busconfig>
  <auth>ANONYMOUS</auth>
  <allow_anonymous/>
</busconfig>
"""

LAUNCHER = """#!/usr/bin/python3
# supervisor 4.3.0 from /opt/cua/python (pinned wheel), on the guest's python3.
import sys
sys.path.insert(0, "/opt/cua/python")
from supervisor.{module} import main
sys.exit(main())
"""


class Layer:
    def __init__(self) -> None:
        self.entries: dict[str, tuple] = {}

    def dir(self, path: str, mode: int = 0o755, uid: int = 0, gid: int = 0) -> None:
        self.entries[path.strip("/")] = ("dir", mode, uid, gid, None)

    def file(self, path: str, data: bytes, mode: int = 0o644, uid: int = 0, gid: int = 0) -> None:
        # No parent entries: directories the upstream tree already has keep
        # its owner and mode (e.g. /var/log is root:syslog 0775); only the
        # directories this layer creates are declared (see dir()).
        self.entries[path.strip("/")] = ("file", mode, uid, gid, data)

    def link(self, path: str, target: str) -> None:
        self.entries[path.strip("/")] = ("sym", 0o777, 0, 0, target)

    def write(self, out: Path) -> dict:
        layer = bench._Layer(out, compress=True)
        for name in sorted(self.entries):
            kind, mode, uid, gid, data = self.entries[name]
            ti = tarfile.TarInfo(name)
            ti.mode, ti.uid, ti.gid, ti.mtime = mode, uid, gid, 0
            if kind == "dir":
                ti.type = tarfile.DIRTYPE
                layer.tar.addfile(ti)
            elif kind == "sym":
                ti.type, ti.linkname = tarfile.SYMTYPE, data
                layer.tar.addfile(ti)
            else:
                ti.size = len(data)
                layer.tar.addfile(ti, io.BytesIO(data))
        return layer.close()


def main() -> int:
    sources = json.loads(Path(sys.argv[1]).read_text())
    spacesd = Path(sys.argv[2])
    out = Path(sys.argv[3])
    out.mkdir(parents=True, exist_ok=True)
    b = bench.load("osworld")
    L = Layer()
    for d in ("opt/cua", "opt/cua/bin", "opt/cua/python", "etc/supervisor", "etc/cua-image"):
        L.dir(d)

    # cua-spacesd and the shared token scripts (the VM uses the same files).
    L.file("usr/local/bin/cua-spacesd", spacesd.read_bytes(), 0o755)
    L.link("usr/local/bin/cua-env-driver", "cua-spacesd")
    L.link("usr/local/bin/cua-guestd", "cua-spacesd")
    for name in ("start-spacesd.sh", "ensure-env-token.sh", "env-token-mode.sh", "start-token-sync.sh"):
        L.file(f"opt/cua/bin/{name}", (DESK / name).read_bytes(), 0o755)
    L.link("opt/cua/bin/start-env-driver.sh", "start-spacesd.sh")
    L.link("opt/cua/bin/start-guestd.sh", "start-spacesd.sh")
    L.file("opt/cua/bin/cua-osworld-session-env.sh", (VM_FILES / "cua-osworld-session-env.sh").read_bytes(), 0o755)
    for name in ("entrypoint.sh", "healthcheck.sh", "start-osworld-desktop.sh"):
        L.file(f"opt/cua/bin/{name}", (FILES / name).read_bytes(), 0o755)

    # supervisord from the pinned wheel.
    with zipfile.ZipFile(sources["container:supervisor"]) as z:
        for info in z.infolist():
            if info.is_dir() or ".dist-info/" in info.filename and info.filename.endswith("RECORD"):
                continue
            parts = info.filename.split("/")
            for i in range(1, len(parts)):
                L.dir("opt/cua/python/" + "/".join(parts[:i]))
            L.file(f"opt/cua/python/{info.filename}", z.read(info), 0o644)
    for module, exe in (("supervisord", "supervisord"), ("supervisorctl", "supervisorctl")):
        L.file(f"usr/local/bin/{exe}", LAUNCHER.format(module=module).encode(), 0o755)
    L.file("etc/supervisor/supervisord.conf", (FILES / "supervisord.conf").read_bytes())
    L.dir("var/log/supervisor")

    # /tmp with the X11 and ICE socket directories, as the VM's tmpfiles.d
    # creates them. gVisor mounts its own small tmpfs (half the sandbox
    # memory) over an empty /tmp, which is too small for the tasks; a
    # non-empty /tmp stays on the rootfs like every other directory.
    for d in ("tmp", "tmp/.X11-unix", "tmp/.ICE-unix"):
        L.dir(d, 0o1777)

    # Xorg dummy video driver (Ubuntu 22.04, matches the guest's Xorg 21.1).
    deb = bench.deb_members(Path(sources["container:xorg-dummy"]))
    drv = "usr/lib/xorg/modules/drivers/dummy_drv.so"
    L.file(drv, deb[drv], 0o644)
    L.dir("usr/share/doc/xserver-xorg-video-dummy")
    L.file("usr/share/doc/xserver-xorg-video-dummy/copyright",
           deb["usr/share/doc/xserver-xorg-video-dummy/copyright"])
    L.file("etc/X11/cua-dummy.conf", DUMMY_CONF.encode())
    L.file("etc/dbus-1/system.d/cua-container.conf", DBUS_SYSTEM_CONF.encode())

    # Image markers.
    L.file("etc/cua-image/variant", b"container\n")
    L.file("etc/cua-image/spacesd-source", b"local\n")
    L.link("etc/cua-image/env-driver-source", "spacesd-source")
    L.link("etc/cua-image/guestd-source", "spacesd-source")
    L.file("etc/cua-image/bench.json", (BENCH / "bench.json").read_bytes())
    # What the image claims, for `cua-spacesd doctor` (build-qcow2.sh generates it).
    manifest = out / "manifest.json"
    if not manifest.is_file():
        raise SystemExit(f"missing {manifest}")
    L.file("etc/cua-image/manifest.json", manifest.read_bytes())
    L.file("etc/cua-image/image.json", (BENCH / "image.json").read_bytes())

    meta = L.write(out / "overlay.tar.gz")
    labels = bench.labels(b, "rootfs")
    config = {
        "created": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(0)),
        "config": {
            "Env": [
                "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
                "LANG=en_US.UTF-8",
                "CUA_DESKTOP_USER=user",
                "CUA_DISPLAY=:0",
            ],
            "Cmd": ["/opt/cua/bin/entrypoint.sh"],
            "ExposedPorts": {p: {} for p in ("5000/tcp", "9222/tcp", "8080/tcp", "3211/tcp", "3212/udp")},
            "Labels": labels,
            "StopSignal": "SIGTERM",
            "Healthcheck": {"Test": ["CMD", "/opt/cua/bin/healthcheck.sh"], "Interval": 10_000_000_000,
                            "Timeout": 5_000_000_000, "StartPeriod": 90_000_000_000, "Retries": 18},
        },
    }
    (out / "config.json").write_text(json.dumps(config, indent=2) + "\n")
    print(json.dumps(meta, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
