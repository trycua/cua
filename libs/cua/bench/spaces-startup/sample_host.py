#!/usr/bin/env python3
"""One JSON line about this host's memory and its QEMU VMs (macOS or Linux).

    python3 sample_host.py [label]

Host: physical memory, what the OS counts as available (macOS
`kern.memorystatus_level`, the figure `memory_pressure` prints; Linux
`MemAvailable`), memory pressure (macOS `kern.memorystatus_vm_pressure_level`;
Linux PSI), compressed memory, swap use, and CPU idle.

QEMU VMs (every `qemu-system-*` process): their count, the sum of their
private memory (macOS `phys_footprint`; Linux `Anonymous` + `Swap` of
`smaps_rollup`), the sum of their plain RSS (shared pages counted once per
process), and the resident part of every distinct guest RAM file they map
(`mem-path=` of a `memory-backend-file`), each counted once. `vm_rss_mb` is
private + shared once: what the VMs really use together, the figure
cua-vmm's admission (`CUA_VM_MAX_RSS_FRACTION`) counts.
"""

import ctypes
import ctypes.util
import json
import mmap
import os
import platform
import re
import subprocess
import sys
import time

MAC = platform.system() == "Darwin"


def sh(*a):
    return subprocess.run(a, capture_output=True, text=True).stdout


def qemu_procs():
    out = []
    for line in sh("ps", "-axo", "pid=,rss=,command=").splitlines():
        parts = line.strip().split(None, 2)
        if len(parts) == 3 and "qemu-system-" in parts[2].split(" ")[0]:
            out.append((int(parts[0]), int(parts[1]) * 1024, parts[2]))
    return out


if MAC:
    _libc = ctypes.CDLL(ctypes.util.find_library("c"))
    _libc.proc_pid_rusage.argtypes = [ctypes.c_int, ctypes.c_int, ctypes.c_void_p]

    def private_bytes(pid):
        # rusage_info_v4: uuid[16], then u64 user_time, system_time,
        # pkg_idle_wkups, interrupt_wkups, pageins, wired_size,
        # resident_size, phys_footprint, ...
        buf = ctypes.create_string_buffer(512)
        if _libc.proc_pid_rusage(pid, 4, buf) != 0:
            return None
        return int.from_bytes(buf.raw[16 + 7 * 8 : 16 + 8 * 8], "little")

    def sysctl(name):
        v = sh("sysctl", "-n", name).strip()
        return int(v) if v.lstrip("-").isdigit() else None

    def host():
        total = sysctl("hw.memsize")
        o = sh("vm_stat")
        page = int(re.search(r"page size of (\d+)", o).group(1))
        g = lambda k: int(re.search(k + r":\s+(\d+)", o).group(1)) * page
        level = sysctl("kern.memorystatus_level")
        p = sysctl("kern.memorystatus_vm_pressure_level")
        swap = re.search(r"used = ([\d.]+)M", sh("sysctl", "-n", "vm.swapusage"))
        return {
            "total_mb": total >> 20,
            "available_mb": (total >> 20) * level // 100,
            "pressure": {1: "normal", 2: "warn", 4: "critical"}.get(p, str(p)),
            "compressed_mb": g("Pages occupied by compressor") >> 20,
            "wired_mb": g("Pages wired down") >> 20,
            "free_mb": g("Pages free") >> 20,
            "swap_used_mb": float(swap.group(1)) if swap else 0.0,
        }

    def cpu_idle():
        o = sh("top", "-l", "2", "-n", "0", "-s", "1")
        m = re.findall(r"CPU usage: [\d.]+% user, [\d.]+% sys, ([\d.]+)% idle", o)
        return float(m[-1]) if m else None

else:

    def private_bytes(pid):
        try:
            t = open(f"/proc/{pid}/smaps_rollup").read()
        except OSError:
            return None
        kb = lambda k: int(re.search(k + r":\s+(\d+)", t).group(1)) if re.search(k + r":", t) else 0
        return (kb("Anonymous") + kb("Swap")) * 1024

    def host():
        t = open("/proc/meminfo").read()
        kb = lambda k: int(re.search(k + r":\s+(\d+)", t).group(1))
        pressure = "normal"
        try:
            p = open("/proc/pressure/memory").read()
            avg = lambda kind: float(re.search(kind + r" avg10=([\d.]+)", p).group(1))
            pressure = "critical" if avg("full") > 10 else "warn" if avg("some") > 10 else "normal"
        except OSError:
            pass
        return {
            "total_mb": kb("MemTotal") // 1024,
            "available_mb": kb("MemAvailable") // 1024,
            "pressure": pressure,
            "compressed_mb": 0,
            "free_mb": kb("MemFree") // 1024,
            "swap_used_mb": (kb("SwapTotal") - kb("SwapFree")) / 1024,
        }

    def cpu_idle():
        def snap():
            f = [int(x) for x in open("/proc/stat").readline().split()[1:]]
            return f[3] + f[4], sum(f)

        i1, t1 = snap()
        time.sleep(1)
        i2, t2 = snap()
        return round(100.0 * (i2 - i1) / max(1, t2 - t1), 1)

def resident_bytes(path):
    """Bytes of `path` in the page cache, by mincore over a private map."""
    libc = ctypes.CDLL(ctypes.util.find_library("c")) if MAC else ctypes.CDLL(None)
    libc.mincore.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_char_p]
    try:
        fd = os.open(path, os.O_RDONLY)
    except OSError:
        return 0
    try:
        size = os.fstat(fd).st_size
        if size == 0:
            return 0
        # Copy-on-write (nothing is written): ctypes needs a writable buffer
        # for the address; untouched pages are the file's.
        m = mmap.mmap(fd, size, access=mmap.ACCESS_COPY)
        try:
            page = os.sysconf("SC_PAGE_SIZE")
            vec = ctypes.create_string_buffer((size + page - 1) // page)
            view = (ctypes.c_char * size).from_buffer(m)
            r = libc.mincore(ctypes.addressof(view), size, vec)
            del view
            return 0 if r else sum(1 for b in vec.raw if b & 1) * page
        finally:
            m.close()
    finally:
        os.close(fd)


def sample(label=""):
    procs = qemu_procs()
    private = 0
    rss = 0
    files = {}
    per_vm = []
    for pid, r, cmd in procs:
        p = private_bytes(pid)
        if p is None:
            continue
        private += p
        rss += r
        per_vm.append(p >> 20)
        m = re.search(r"memory-backend-file,[^ ]*mem-path=([^, ]+)", cmd)
        if m and m.group(1) not in files:
            files[m.group(1)] = resident_bytes(m.group(1))
    shared = sum(files.values())
    per_vm.sort()
    d = {
        "t": time.time(),
        "label": label,
        "os": platform.system().lower(),
        "host": host(),
        "cpu_idle": cpu_idle(),
        "vms": len(per_vm),
        "vm_private_mb": private >> 20,
        "vm_rss_sum_mb": rss >> 20,
        "vm_shared_files": len(files),
        "vm_shared_mb": shared >> 20,
        "vm_rss_mb": (private + shared) >> 20,
        "vm_private_p50_mb": per_vm[len(per_vm) // 2] if per_vm else 0,
        "vm_private_max_mb": per_vm[-1] if per_vm else 0,
    }
    d["per_vm_mb"] = round(d["vm_rss_mb"] / len(per_vm), 1) if per_vm else 0
    return d


if __name__ == "__main__":
    print(json.dumps(sample(sys.argv[1] if len(sys.argv) > 1 else "")))
