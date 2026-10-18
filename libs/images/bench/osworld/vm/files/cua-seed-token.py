#!/usr/bin/python3
"""Take the cua SDK's spacesd token from a NoCloud seed, for guests without
cloud-init (the OSWorld disk has none).

The SDK boots local VMs with a `cidata` seed whose #cloud-config writes the
token to /etc/cua/env-token and extra variables to /etc/cua/spacesd.env. This
reads only those two write_files entries (nothing else in the seed runs) and
writes them root-only; ensure-env-token.sh then installs the token as usual
(local mode). With a Fleet claim share (virtio-fs) present, the claim token is
authoritative and the seed is ignored.
"""
import email
import glob
import os
import subprocess
import sys
import time

WANTED = {"/etc/cua/env-token": 0o600, "/etc/cua/spacesd.env": 0o600}
MOUNT = "/run/cua-seed"


def log(msg: str) -> None:
    print(f"cua-seed-token: {msg}", flush=True)


def claim_share_present() -> bool:
    for dev in glob.glob("/sys/bus/virtio/devices/*/device"):
        try:
            if open(dev).read().strip() == "0x001a":
                return True
        except OSError:
            pass
    return False


def cloud_configs(raw: bytes) -> list:
    text = raw.decode("utf-8", "replace")
    if text.lstrip().startswith("#cloud-config"):
        return [text]
    msg = email.message_from_string(text)
    if not msg.is_multipart():
        return []
    return [p.get_payload(decode=True).decode("utf-8", "replace") for p in msg.walk()
            if p.get_content_type() == "text/cloud-config"]


def main() -> int:
    if claim_share_present():
        log("Fleet claim share present; the claim token is authoritative")
        return 0
    seed = None
    for _ in range(20):  # bounded: udev may still be naming the drive
        seed = next(iter(glob.glob("/dev/disk/by-label/cidata") + glob.glob("/dev/disk/by-label/CIDATA")), None)
        if seed:
            break
        time.sleep(0.5)
    if not seed:
        log("no cidata seed; nothing to do")
        return 0
    os.makedirs(MOUNT, mode=0o700, exist_ok=True)
    if subprocess.run(["mount", "-o", "ro", seed, MOUNT]).returncode != 0:
        log(f"cannot mount {seed}")
        return 0
    try:
        raw = open(os.path.join(MOUNT, "user-data"), "rb").read()
    except OSError:
        raw = b""
    finally:
        subprocess.run(["umount", MOUNT])
    import yaml

    wrote = []
    for doc in cloud_configs(raw):
        cfg = yaml.safe_load(doc) or {}
        for entry in cfg.get("write_files") or []:
            path = entry.get("path")
            if path not in WANTED:
                continue
            os.makedirs(os.path.dirname(path), mode=0o755, exist_ok=True)
            fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, WANTED[path])
            with os.fdopen(fd, "w") as f:
                f.write(str(entry.get("content", "")))
            os.chown(path, 0, 0)
            os.chmod(path, WANTED[path])
            wrote.append(path)
    log(f"wrote {', '.join(wrote) if wrote else 'nothing (no token in the seed)'}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
