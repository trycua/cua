# shellcheck shell=bash
# Which of the browsers' own sandboxes this runtime can carry. Sourced by
# start-desktop.sh (the results go into desktop.env) and by
# ../tests/test-browser-sandbox.sh, which stubs the three probes below.

under_gvisor() {
    # Current runsc releases name themselves in the kernel release; older
    # ones only in the first dmesg line.
    grep -qi gvisor /proc/sys/kernel/osrelease 2>/dev/null \
        || dmesg 2>/dev/null | head -1 | grep -q gVisor
}
unprivileged_userns() { unshare --user --map-root-user true 2>/dev/null; }
machine() { uname -m; }

# Prints the desktop.env lines that turn Firefox's and Chromium's sandboxes
# down to what the runtime supports, and nothing where they work as shipped.
#
# Firefox under gVisor: its content/RDD/GPU children die with
# "VideoBridgeParent ... AbnormalShutdown" and no window maps, so its inner
# sandboxes go off there.
#
# Chromium keeps its namespace sandbox wherever the runtime has one:
# - No unprivileged user namespaces (docker's default seccomp profile): no
#   sandbox at all is possible, the container boundary is the sandbox, and
#   Chromium runs with --no-sandbox (CUA_DRIVER_BROWSER_NO_SANDBOX).
# - gVisor on arm64: the namespaces work, but the renderer's seccomp-bpf
#   filter kills every tab ("Aw, Snap!", "seccomp-bpf failure in syscall
#   nr=0x7b", sched_getaffinity). Only the seccomp layer goes off
#   (--disable-seccomp-filter-sandbox, CUA_DRIVER_BROWSER_NO_SECCOMP_SANDBOX);
#   the namespace sandbox stays, and gVisor filters the syscalls itself.
#   gVisor on x86_64 runs the full sandbox.
# Either variable set by the operator wins over detection.
browser_sandbox_env() {
    local gvisor=0
    under_gvisor && gvisor=1
    if [ "$gvisor" = 1 ]; then
        local v
        for v in MOZ_DISABLE_CONTENT_SANDBOX MOZ_DISABLE_GMP_SANDBOX MOZ_DISABLE_RDD_SANDBOX \
            MOZ_DISABLE_SOCKET_PROCESS_SANDBOX MOZ_DISABLE_UTILITY_SANDBOX; do
            echo "$v=1"
        done
    fi
    if [ -n "${CUA_DRIVER_BROWSER_NO_SANDBOX:-}" ] || [ -n "${CUA_DRIVER_BROWSER_NO_SECCOMP_SANDBOX:-}" ]; then
        [ -n "${CUA_DRIVER_BROWSER_NO_SANDBOX:-}" ] \
            && echo "CUA_DRIVER_BROWSER_NO_SANDBOX=$CUA_DRIVER_BROWSER_NO_SANDBOX"
        [ -n "${CUA_DRIVER_BROWSER_NO_SECCOMP_SANDBOX:-}" ] \
            && echo "CUA_DRIVER_BROWSER_NO_SECCOMP_SANDBOX=$CUA_DRIVER_BROWSER_NO_SECCOMP_SANDBOX"
        return 0
    fi
    if [ "$gvisor" = 1 ]; then
        case "$(machine)" in
        aarch64 | arm64) echo "CUA_DRIVER_BROWSER_NO_SECCOMP_SANDBOX=1" ;;
        esac
    elif ! unprivileged_userns; then
        echo "CUA_DRIVER_BROWSER_NO_SANDBOX=1"
    fi
}
