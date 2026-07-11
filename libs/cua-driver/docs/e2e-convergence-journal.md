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
| `6733efe9` to `b52d239f` | Removed duplicate Windows modality-input probes, normalized desktop-scope ownership, and corrected Electron's X11 show path | Local testkit checks stayed green. Linux run `29133849724` still rejected a blank root-desktop recording, so it produced no behavioral verdict. |
| `c3454d1b` to `e54088f3` | Shared the Electron foreground sentinel and attached its Focus, ZOrder, Cursor, and NoLeakedInput contract to WPF background actions | Windows run `29134124533` passed all 18 WPF tests. Its lane failed later on an independent WebView2 CDP-readiness race. |
| `48afc293` to `842f3c86` | Moved background capture, minimized launch, and agent-cursor behavior into their action-owned tests | The replacements assert application or pixel state plus the desktop observer instead of a focus-only guard. |
| `c49dd59d` | Rejected Chromium UIA Invoke's false-positive and tested coordinate injection as a replacement route | Focused run `29134320325` proved the route still refused a fully occluded target. That route was not accepted as a fix. |
| `521508aa` to `99897fed` | Added the reusable typed executor and typed WPF, launch, capture, and cursor records | Sixteen testkit tests and host compile checks passed. Fresh native and capture runs are validating the Windows-only bodies. |
| `ad891b48` | Made CDP page discovery wait for the first usable target | The earlier native run's only failure was WebView2 exposing a listener before `/json` contained a page. The DOM-state assertion remains unchanged. |
| `10a1a749` | Preserved the old guard's anti-overclaim check in canonical background text/key rows and aligned action-owned video labels | Fable's deletion audit identified protocol honesty as the only unique old-guard assertion. Chromium AX background click now probes a target-bound LegacyIAccessible action rather than a coordinate route. |
| `5bca25be` | Mapped the normal Electron harness at construction | Linux's deferred BrowserWindow was enumerable and capturable but absent from the X11 root recording. A focused preflight run is validating this lifecycle correction. |

## Active Validation

| Platform | Run | Purpose | State |
| --- | --- | --- | --- |
| Windows | `29134898319` | Target-bound Chromium AX background click probe | Running |
| Windows | `29134649191` | Typed WPF, launch, cursor, WinUI3, and WebView2 replacements | Running |
| Windows | `29134649821` | Typed capture and desktop-scope replacements | Running |
| Linux X11 | `29134942303` | Mapped Electron preflight and focused foreground cell | Running |
| macOS | Local preflight | Verify installed source identity, TCC, fixture visibility, capture, and video | Blocked before cells: the installed app fell back to ad-hoc signing, so the existing Screen Recording grant no longer applies. |

The macOS preflight failure is separate from driver behavior. It must be fixed
or re-authorized before any local macOS cell result is accepted.
