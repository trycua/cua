#!/bin/sh
# cua-runsc @VERSION@: written by cua (cua_vmm::container::snapshot); do not edit.
#
# The OCI runtime Docker runs gVisor Spaces with. It is runsc, except that a
# container created with the OCI annotation `ai.cua.restore=<dir>` is
# restored from the memory checkpoint in <dir> when it first starts, instead
# of booting its image (later starts boot it): `runsc create` runs as usual (Docker then sets up the
# container's network namespace: its own address), and `start` becomes
# `runsc restore` into that created sandbox, which takes the network from the
# new namespace.
#
# `cua-runsc cua <op> ...` is cua's control interface, run as root through a
# privileged helper container:
#   info                      what this host supports (key=value lines)
#   checkpoint <id> <dir>     save running container <id> (memory and its
#                             gVisor root overlay) into <dir>; it keeps running
#   rm <dir>...               delete checkpoints
#   forget <id>...            drop what is kept about deleted containers
set -u
RUNSC=@RUNSC@
[ -x "$RUNSC" ] || RUNSC=/usr/bin/runsc
STATE=@STATE@
STORE=@STORE@
VERSION=@VERSION@

if [ "${1:-}" = cua ]; then
    shift
    op="${1:-}"
    [ $# -gt 0 ] && shift
    case "$op" in
    info)
        # arm64 pointer authentication: gVisor (systrap) does not carry a
        # process's PAC keys across a restore, so PAC-built binaries die
        # with SIGILL in any restored sandbox (gvisor#13542).
        pac=0
        grep -qw paca /proc/cpuinfo 2>/dev/null && pac=1
        echo "version=$VERSION"
        echo "store=$STORE"
        echo "pac=$pac"
        echo "arch=$(uname -m)"
        echo "runsc=$("$RUNSC" --version 2>/dev/null | head -n 1)"
        exit 0
        ;;
    checkpoint)
        id="${1:?container id}"
        dir="${2:?checkpoint dir}"
        case "$dir" in "$STORE"/*) ;; *)
            echo "a checkpoint lives under $STORE" >&2
            exit 2
            ;;
        esac
        root=$(cat "$STATE/$id.root" 2>/dev/null) || {
            echo "container $id was not started by cua-runsc" >&2
            exit 3
        }
        tmp="$dir.partial"
        rm -rf "$tmp"
        mkdir -p "$tmp" || exit 1
        chmod 0700 "$tmp"
        "$RUNSC" --root "$root" checkpoint --leave-running --image-path "$tmp" "$id"
        rc=$?
        if [ "$rc" -ne 0 ]; then
            rm -rf "$tmp"
            exit "$rc"
        fi
        rm -rf "$dir"
        mv "$tmp" "$dir"
        exit $?
        ;;
    forget)
        for id in "$@"; do
            case "$id" in */* | .*) ;; *) rm -f "$STORE/.restored/$id" ;; esac
        done
        exit 0
        ;;
    rm)
        for d in "$@"; do
            case "$d" in "$STORE"/*) rm -rf "$d" "$d.partial" ;; esac
        done
        exit 0
        ;;
    *)
        echo "cua-runsc: unknown op '$op'" >&2
        exit 2
        ;;
    esac
fi

# OCI runtime mode. The shim calls `runsc <global flags> <command> <flags> <id>`.
sub=""
id=""
root=""
bundle=""
prev=""
for a in "$@"; do
    if [ -z "$sub" ]; then
        case "$prev" in --root | -root) root="$a" ;; esac
        case "$a" in
        --root=* | -root=*) root="${a#*=}" ;;
        create | start | delete) sub="$a" ;;
        esac
    else
        case "$prev" in --bundle | -bundle | -b) bundle="$a" ;; esac
        case "$a" in --bundle=* | -bundle=*) bundle="${a#*=}" ;; esac
        case "$a" in
        -*) ;;
        *)
            case "$prev" in
            --bundle | -bundle | -b | --pid-file | -pid-file | --console-socket | -console-socket) ;;
            *) id="$a" ;;
            esac
            ;;
        esac
    fi
    prev="$a"
done

case "$sub" in
create)
    [ -n "$bundle" ] || bundle=$(pwd)
    if [ -n "$id" ]; then
        mkdir -p "$STATE"
        [ -n "$root" ] && printf '%s' "$root" >"$STATE/$id.root"
        img=$(sed -n 's/.*"ai\.cua\.restore":"\([^"]*\)".*/\1/p' "$bundle/config.json" 2>/dev/null | head -n 1)
        # Only the first start restores: a container stopped and started
        # again boots (it has its own identity by then).
        if [ -n "$img" ] && [ ! -e "$STORE/.restored/$id" ]; then
            printf '%s' "$img" >"$STATE/$id.restore"
        else
            rm -f "$STATE/$id.restore"
        fi
    fi
    ;;
start)
    if [ -n "$id" ] && [ -f "$STATE/$id.restore" ]; then
        img=$(cat "$STATE/$id.restore")
        rm -f "$STATE/$id.restore"
        mkdir -p "$STORE/.restored" && : >"$STORE/.restored/$id"
        # Keep the global flags (everything before `start`), then restore.
        n=0
        for a in "$@"; do
            [ "$a" = start ] && break
            n=$((n + 1))
        done
        i=0
        for a in "$@"; do
            shift
            [ "$i" -lt "$n" ] && set -- "$@" "$a"
            i=$((i + 1))
        done
        # The restored sandbox's spec differs from the source's in what makes
        # it a new container (its id, hostname, /etc/hosts and resolv.conf
        # files); warn instead of refusing.
        exec "$RUNSC" "$@" --restore-spec-validation=warning restore --detach --background \
            --image-path "$img" "$id"
    fi
    ;;
delete)
    [ -n "$id" ] && rm -f "$STATE/$id.root" "$STATE/$id.restore"
    ;;
esac
exec "$RUNSC" "$@"
