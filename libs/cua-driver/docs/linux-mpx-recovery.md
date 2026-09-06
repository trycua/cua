# Linux X11 MPX recovery

Refs #3337.

At main `130df4251e75f48bcbc43145b0cf7c7feac9ddc5`, a real Xorg 21.1.11
desktop with libinput and `/dev/uinput` delivers a normal background scroll
and removes its temporary master devices. SIGTERM during a longer scroll
leaves its master pointer/keyboard pair after the owning process exits.
The GTK keyboard fixture still accepts keys afterward in this environment;
this does not reproduce the separate Chrome keyboard symptom reported in
the issue.

Recovery must distinguish a stale owner from a live process, PID reuse,
inaccessible process metadata, and a different host or PID namespace using
the same X server. A bare PID parsed from an old device name is insufficient.
Unknown or legacy ownership must remain untouched by automatic recovery.

The candidate will be checked on real Xorg for normal input, clean exit,
SIGTERM and SIGKILL during an operation, restart cleanup, and live-peer
preservation. Focused ownership tests and the canonical Linux desktop
harness complement that native evidence. No Wayland behavior is changed.

## Recovery contract

New master names encode a versioned ownership domain (kernel boot, PID
namespace, and effective user), PID, process start time, and unique nonce.
Ownership is part of `XIAddMaster`, so SIGKILL cannot interrupt a separate
ownership-property write. Startup of a standalone daemon removes a matching
pair only when its owner is provably absent or that PID has been reused.
Device-ID validation and removal run under the same X server grab.

Live peers, inaccessible process metadata, foreign ownership domains, and
legacy names are preserved. Older devices require deliberate manual recovery
after their owner has been verified; parsing a legacy PID alone is unsafe.
Direct embedded SDK startup does not perform recovery. SIGTERM and SIGKILL
can still leave devices until the next standalone daemon starts.

## Reproduce the focused native gate

Use a disposable real Xorg desktop with libinput, readable/writable
`/dev/uinput`, `xinput`, Python GTK3 bindings, and its session bus. Xvfb and
TigerVNC do not support the needed device hotplug path.

```sh
cargo build --locked -p cua-driver --manifest-path libs/cua-driver/rust/Cargo.toml
python3 libs/cua-driver/tests/linux-mpx-recovery.py \
  --driver libs/cua-driver/rust/target/debug/cua-driver \
  --output /tmp/mpx-recovery-evidence \
  --source-sha "$(git rev-parse HEAD)"
```

The output directory must be new. The harness starts only its own daemons and
fixture; it records native snapshots, fixture events, XInput inventories,
and a final `proof.json`. It checks normal input and clean exit, both signal
paths and restart recovery, preservation of a paused live peer followed by
successful resumed input, and preservation of pre-existing unowned devices.
It also verifies that read-only `describe` does not trigger recovery.
Run the canonical Linux harness separately on the final candidate.
