#!/usr/bin/env python3
"""Read-only prerequisites; never claims the full Fleet e2e has passed."""
import argparse
import fcntl
import ipaddress
import json
import os
from pathlib import Path
import shutil
import subprocess

NETWORKS = ("10.210.0.0/24", "10.211.0.0/16", "10.212.0.0/16")
REQUIRED_COVERAGE = ("node_pod_cidrs", "service_cidrs", "outer_routes", "vpn_routes")


def outer_evidence(value):
    # A partial inventory can detect conflicts, but cannot authorize networking.
    if isinstance(value, list):
        return value, list(REQUIRED_COVERAGE)
    routes = value["routes"]
    if not isinstance(routes, list):
        raise ValueError("outer evidence routes must be a list")
    coverage = value.get("coverage", {})
    missing = [key for key in REQUIRED_COVERAGE if coverage.get(key) is not True]
    return routes, missing


def overlaps(routes, networks=NETWORKS):
    conflicts = []
    for route in routes:
        dst = route.get("dst", "default")
        if dst == "default":
            continue
        existing = ipaddress.ip_network(dst, strict=False)
        for candidate in networks:
            planned = ipaddress.ip_network(candidate)
            if existing.version == planned.version and existing.overlaps(planned):
                conflicts.append({"route": dst, "planned": candidate})
    return conflicts


def kvm_probe():
    fds = []
    try:
        device = os.open("/dev/kvm", os.O_RDWR | os.O_CLOEXEC)
        fds.append(device)
        version = fcntl.ioctl(device, 0xAE00, 0)
        vm = fcntl.ioctl(device, 0xAE01, 0)
        fds.append(vm)
        vcpu = fcntl.ioctl(vm, 0xAE41, 0)
        fds.append(vcpu)
        return {"api_version": version, "vm_created": True, "vcpu_created": True}
    except OSError as exc:
        return {"error": str(exc)}
    finally:
        for fd in reversed(fds):
            os.close(fd)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--data-root", default="/var/lib/cyclops-dev")
    parser.add_argument("--outer-routes", type=Path,
                        help="JSON routes + coverage inventory captured from the outer cluster/VPN")
    args = parser.parse_args()
    routes = json.loads(subprocess.check_output(["ip", "-j", "route"], text=True))
    missing_coverage = list(REQUIRED_COVERAGE)
    if args.outer_routes:
        outer_routes, missing_coverage = outer_evidence(json.loads(args.outer_routes.read_text()))
        routes += outer_routes
    memory = int(next(line.split()[1] for line in Path("/proc/meminfo").read_text()
                      .splitlines() if line.startswith("MemTotal:"))) * 1024
    data = Path(args.data_root)
    probe = kvm_probe()
    report = {
        "cpu_count": os.cpu_count(), "memory_bytes": memory,
        "data_root": str(data), "data_is_mount": data.is_mount(),
        "data_free_bytes": shutil.disk_usage(data).free,
        "cgroup_v2": Path("/sys/fs/cgroup/cgroup.controllers").exists(),
        "route_conflicts": overlaps(routes), "outer_routes_checked": bool(args.outer_routes),
        "outer_coverage_missing": missing_coverage,
        "kvm": probe, "full_core_e2e": False,
        "warnings": [],
    }
    if report["cpu_count"] < 16 or memory < 30 * 1024**3:
        report["warnings"].append("Below proposed 16 CPU / 32 GiB engineering budget; measure each gate.")
    report["local_prerequisites_pass"] = bool(
        report["data_is_mount"] and report["data_free_bytes"] >= 100 * 1024**3
        and report["cgroup_v2"] and not report["route_conflicts"]
        and probe.get("api_version") == 12 and probe.get("vcpu_created")
    )
    report["network_apply_allowed"] = report["local_prerequisites_pass"] and not missing_coverage
    print(json.dumps(report, indent=2))
    return 0 if report["network_apply_allowed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
