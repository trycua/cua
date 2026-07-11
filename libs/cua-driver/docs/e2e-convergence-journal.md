# E2E Convergence Journal

This journal records changes and test evidence for the Rust desktop harness
convergence work. It omits machine names, credentials, and partner names.

## 2026-07-10

| Commit | Change | Verification or finding |
| --- | --- | --- |
| `0fda31bf` | Added typed case declarations, results, and lane preflight | Testkit unit tests passed locally. The first Windows and Linux runs exposed fixture and recording failures before behavioral classification. |
| `f2e5804c` | Added full result dimensions to the reporter | Reporter tests reject changed contracts and missing rows. |
| `6fa9311e` | Removed the synthetic calculator fixture | The canonical shared target now uses only controls in the repo-local web harness. |
| `891d4c94` | Added the platform-independent desktop observer core and Windows backend | Unit tests prove that transient focus changes, z-order changes, cursor movement, and input leaks fail independently. |
| `3a48ba0d` | Made the Linux desktop preflight reject blank recordings | Run `29131453240` stopped with `94/2073600` non-dark pixels. This was an environment failure, so it produced no behavioral verdicts. |
| `28ce18b6` | Added fail-closed cell filtering | An unmatched filter now fails instead of reporting a green zero-cell run. |
| `303c7bf3` to `056728ec` | Added the occluding Electron sentinel and staged its preload | A local sentinel smoke test wrote direct ready and focus events to its JSONL journal. |
| `640fcc9a` | Replaced the default route label with explicit backend routes | Unit coverage keeps Windows Chromium PX background delivery on `WindowsTargetedInjection`; it remains a required capability. |
| `580708f7` | Added the direct macOS observer | Fifteen testkit tests and strict testkit Clippy passed locally. A native snapshot smoke test read the foreground app and real cursor. |
| `00a1d31f` | Added the direct X11 observer and attached desktop checks to background shared cells | Fable review found and corrected XWayland, reparenting, raw input-focus, and occlusion traps. Sixteen testkit tests passed locally. |
| `bcc53fe7` | Expanded the shared catalog from a 19-cell diagonal to 32 cells per host | Route tests cover every declared Win32, Quartz, X11, and Wayland combination. |
| `21f3f6e0` | Fixed workflow dispatch checkout semantics | Earlier branch dispatches had selected the branch workflow but checked out the default `inputs.ref=main`. Their behavioral results were discarded. |
| `69c7c2c1` | Made each OS runner select only the typed shared catalog | Legacy tests remain in source until their assertions pass the deletion ledger, but they no longer define the canonical shared run. |

## Active Validation

| Platform | Run | Purpose | State |
| --- | --- | --- | --- |
| Windows | `29132563928` | First branch-source Electron AX background click probe | Running |
| Linux X11 | `29132563205` | First branch-source Electron AX background click probe | Running |
| macOS | Local preflight | Verify installed source identity, TCC, fixture visibility, capture, and video | Blocked before cells: the installed app fell back to ad-hoc signing, so the existing Screen Recording grant no longer applies. |

The macOS preflight failure is separate from driver behavior. It must be fixed
or re-authorized before any local macOS cell result is accepted.
