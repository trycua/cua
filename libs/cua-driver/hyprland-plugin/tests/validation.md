# Discovery foundation validation

The discovery-only plugin passed a native build and lifecycle checks on Cua
Cloud Fleet on September 4, 2026. These results support review of the foundation;
the full promotion gates in [the test plan](README.md) remain open.

## Candidate and environment

The tested source was based on Cua commit
`4da67bb61c262c663fa88559d1d3f1a138b3a463`. The 27-file plugin tree had SHA-256
`32c4c2224bcf260921083797a4c5bfd72dbca0ced70cbd0d24738de2caf83007`.
This digest hashes sorted relative file paths and raw file contents, each
followed by a NUL byte. It excludes this subsequently added validation report.

| Component                      | Tested value          |
| ------------------------------ | --------------------- |
| Plugin                         | `0.1.0`               |
| Omarchy                        | `4.0.1-1`             |
| Hyprland                       | `0.56.2-1`            |
| xdg-desktop-portal-hyprland    | `1.4.1-1`             |
| Installed Cua Driver           | `0.22.2`              |
| Compositor and plugin compiler | GCC `16.1.1 20260728` |
| CMake / Ninja                  | `4.4.3` / `1.13.2`    |
| Shared C++ runtime             | `libstdc++.so.6.0.36` |

This Fleet image matches the Hyprland baseline but has older Omarchy and
Cua Driver versions than the complete acceptance target. Driver integration
was not exercised by this plugin run.

The exact compiler package was selected from Hyprland's package build metadata
and its Arch signature verified. The plugin used the shared C++ runtime already
loaded by the compositor. The actual Arch recipe ran with `makepkg --nodeps`
because CMake and Ninja were task-local Python wheels. Its independent checks
for installed Hyprland and header versions still ran.

| Artifact        | SHA-256                                                            |
| --------------- | ------------------------------------------------------------------ |
| Arch package    | `a9e401ca9ac1c1f0532519f74a650e84ff679ce2e29ab8751b85903e1e0a7e99` |
| Packaged module | `842a4aa768e95434bba7c9ac2c56c4846a518f0cff883f8c9fee7b2480736f43` |

## Results

- Native package build and `check()`: six of six CTests passed, including the
  production Linux `SOCK_SEQPACKET` transport.
- Package inventory: shared module and license, plus package metadata; exact
  `hyprland=0.56.2-1` dependency; no configuration edits or autoload.
- Real Hyprland load: matching ABI fingerprints, disabled initial transport,
  and enabled discovery socket with mode `0600` after config reload.
- Ten consecutive live unload/reload cycles: all six mutation types returned
  `background_unavailable`, measured compositor state stayed unchanged, old
  connections closed, and replacement transports published fresh epochs.
- Disable/re-enable: socket and client closed, disabled epoch became zero,
  and re-enabling created a fresh epoch.
- Synthetic ABI mismatch: altering the compiled fingerprint caused explicit
  load refusal before command/socket registration. The matching module loaded
  afterward. This was not a second real Hyprland version.
- Supplemental native transport fixture: same-UID acceptance, different-UID
  rejection through `SO_PEERCRED`, and independent sockets/epochs. Only that
  fixture's permissions were relaxed to test peer rejection independently of
  filesystem access. This was not two simultaneous compositor instances.
- macOS review-host suites: six of six tests passed in normal, ASan/UBSan, and
  TSan configurations. macOS uses a transport shim and mocked compositor APIs.

The reusable live check is [live_discovery.py](live_discovery.py). It measures
active-window identity, workspace, cursor coordinates, window geometry/mapping,
and focus history. It does not prove application delivery, complete stacking
order, physical input isolation, or behavior during an active foreground grab.
The peer and synthetic ABI checks were supplemental fixtures; they are not
claimed as automated by that script.

## Runtime-linkage failure investigated

An earlier compiler staging directory omitted its separately packaged shared
runtime. GCC selected `libstdc++.a`; a later reload of that module hung
Hyprland. Debugger attachment was unavailable, so static linkage remains the
leading suspected cause rather than a backtrace-confirmed diagnosis.

Correcting the runtime search path produced a module linked to the compositor's
shared C++ runtime. A fresh Fleet passed the repeated lifecycle checks above.
The build now rejects modules without a shared C++ runtime dependency.

## Remaining gates

System installation with `pacman -U`, dependency-upgrade refusal, a genuine
cross-version ABI mismatch, two real compositor instances, clean compositor
restart, the complete Fleet baseline, and physical-host certification remain
unverified. Second-seat delivery, target tokens, operator authorization, driver
integration, and the application/foreground-grab matrix remain future work.

The final source differs from the Fleet-tested tree only in documentation and
formatting: shutdown wording describes the worker join accurately, this report
records the evidence, and Markdown/Python formatting follows repository tools.
The Python harness's parsed syntax tree is checked against the tested copy.
Plugin C++ source, CMake files, and packaging are unchanged.

The guest configuration was restored, the plugin unloaded, and both disposable
Fleet pools deleted with fresh lookups confirming their removal.
