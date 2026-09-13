# Discovery foundation validation

The discovery-only plugin passed a native build and lifecycle checks on Cua
Cloud Fleet on September 4, 2026. These results support review of the foundation;
the full promotion gates in [the test plan](README.md) remain open.

The [September 5 merge-readiness pass](#merge-readiness-revalidation-september-5-2026)
records a fresh candidate build and the remaining system-installation gate.

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

## Follow-up at `3789f0e6e`

A second Fleet run on September 4, 2026 (September 5 UTC), tested commit
`3789f0e6eeb350587fe48c4da35b327975a5bd9c` through the Cua Sandbox SDK.
The image had the same component versions listed above. The source archive
contained the committed plugin directory and repository license, with SHA-256
`27835f9192a35d9b721396642e91a81eb18088075d15c5b4ed2f1f912e159aa4`.

The matching signed compiler package and shared runtime were used again.
The Arch recipe built successfully with `makepkg --nodeps`; all six native
CTests passed. The downloaded artifacts matched the guest hashes:

| Artifact        | SHA-256                                                            |
| --------------- | ------------------------------------------------------------------ |
| Arch package    | `2c4881b4a17aef554edcaa1181f73ae2453e39b05c7bb5169db8ae9be34adebf` |
| Packaged module | `693d6470a011956e58320de67fae8ed8d4628476e605f74cea018eb2eb5d5705` |

Additional native checks passed:

- Two simultaneous Hyprland `0.56.2` processes, nested under the Fleet desktop,
  exposed distinct discovery sockets and epochs. The live discovery harness
  passed all six mutation refusals and its compositor-state comparison in each.
- Restarting one nested compositor closed its old connection and removed its
  socket. Its replacement used a fresh instance socket and epoch. The other
  compositor's existing discovery connection remained responsive through both
  restart and shutdown of its sibling.
- A genuine Hyprland `0.56.1-3` Arch package, verified against its signature,
  ran as another nested compositor. Loading the unmodified `0.56.2` plugin
  explicitly failed with an ABI fingerprint mismatch. No plugin, status
  command, or discovery socket remained registered, and the compositor still
  answered version queries. This closes the earlier synthetic-only ABI gap
  for this specific version pair, not arbitrary upgrades.

The lifecycle orchestration was task-local; these additional checks are not
automated by `live_discovery.py` alone. Initial headless startup failed before
loading the plugin; nested Wayland startup succeeded. The nested tests do not
certify direct DRM startup, physical input, application delivery, or isolation
during a foreground grab. All input mutations remained disabled.

The SDK shell ran as an unprivileged desktop user. `sudo -n -l` required a
password, and the SDK shell interface provided no user-selection parameter.
System installation and upgrade-transaction checks therefore remain open;
package construction is not evidence of `pacman -U` installation.

The [hosted Linux CI run](https://github.com/trycua/cua/actions/runs/33937061198)
also passed all six tests at this SHA, using Ubuntu 24.04, Clang 18.1.3, and
CMake 3.31.6. It covers the native Unix transport and mocked compositor APIs,
not a loaded compositor module. A fresh macOS Debug build passed six tests.

All test compositor units were stopped, the plugin was unloaded, and the
original desktop remained responsive with its configuration hash unchanged.
No system package was installed and no autoload setting was added.
The SDK accepted deletion of the follow-up pool. Immediate lookup still
returned it; later pool and namespace lookups returned `403`, so final resource
absence could not be independently confirmed with that credential.
A subsequent successful account-level SDK namespace listing returned no matching
namespace. Direct lookup still returned `403`; that response alone is not
treated as proof of deletion.

## Reproducible lifecycle follow-up at `0a67bf5c8`

The checked-in [nested lifecycle runner](nested_lifecycle.py) passed on Cua
Cloud Fleet at `0a67bf5c844f5b358651737fb3545aaed5522007` on September 5, 2026
(UTC), through the Cua Sandbox SDK. The committed plugin, workflow, and license
archive had SHA-256
`960beaebeba6b4b10e9b08cdeb56e3843313b5e41a4154ca95693615e2a06ce0`.

The guest had Omarchy `4.0.1-1`, Hyprland `0.56.2-1`, portal `1.4.1-1`, and
installed driver `0.22.2`. This run reused the matching-toolchain module from
the `3789f0e6e` build, SHA-256
`693d6470a011956e58320de67fae8ed8d4628476e605f74cea018eb2eb5d5705`.
Plugin C++, headers, CMake, and packaging were unchanged between that build and
this candidate; this run tests the new orchestration, not a new package build.

Both exact-PID-owned nested compositors passed the live six-mutation refusal
and measured-state check. One restarted with a fresh socket and epoch; its old
connection closed and socket disappeared. The sibling's connection remained
responsive through restart and shutdown. Both owned processes stopped, only
the original parent remained in the instance inventory, and the parent's
plugin list was empty. Seven runner control-flow/cleanup tests also passed on
the guest and on the macOS review host. Hosted CI now runs those tests alongside
the existing six CTests; CI itself does not launch nested Hyprland.

The guest still lacked `renderD128` and passwordless package-install access.
No driver was installed, no system package transaction ran, and no input
capability was enabled. Nested startup can affect the disposable parent's
focus, so parent foreground preservation is not claimed by this lifecycle run.
Application delivery, foreground-grab isolation, and direct-DRM behavior remain
outside its evidence. The report-only follow-up does not change the tested
runner or executable sources.

## Outstanding acceptance gates

System installation with `pacman -U`, dependency-upgrade refusal, the complete
Fleet baseline, and physical-host certification remain unverified. Native
two-instance and clean-restart evidence is limited to nested compositors.
Second-seat delivery, target tokens, operator authorization, driver
integration, and the application/foreground-grab matrix remain future work.

The initial source differed from the first Fleet-tested tree only in documentation and
formatting: shutdown wording describes the worker join accurately, this report
records the evidence, and Markdown/Python formatting follows repository tools.
The Python harness's parsed syntax tree is checked against the tested copy.
Plugin C++ source, CMake files, and packaging are unchanged.

The guest configuration was restored, the plugin unloaded, and both initial
disposable Fleet pools deleted with fresh lookups confirming their removal.
The follow-up report changes only this Markdown file after testing
`3789f0e6e`; it does not change the plugin, packaging, CI, or test harness.

## Merge-readiness revalidation (September 5, 2026)

Tested source: `e254619817e75019d025e2714a395378ea533a28`, after merging
`1336a5e419c933eb750bfefe2d9db6aad63235de` from main. The merge resolved one
installation-document conflict without changing the plugin implementation.
The test-helper follow-up refuses optimized Python before validation or
compositor access; four regression tests cover that guard and packet exchange.
The plugin remains version `0.1.0`, discovery-only, and outside the Driver
installation/release path. There are no Rust delivery changes in this PR.

A fresh disposable Cua Cloud Fleet claim used Omarchy `4.0.1-1`, Hyprland
`0.56.2-1`, and portal `1.4.1-1`. The installed Driver remained `0.22.2` and
was not replaced or used as input-delivery evidence. Hyprland's ELF compiler
record and the selected plugin compiler both reported GCC `16.1.1 20260728`;
the guest's default compiler was `16.2.1` and was not used for the plugin.
The module linked the guest's shared `libstdc++.so.6.0.36`.

Validation results:

- The unchanged Arch recipe built with CMake `4.4.3` and Ninja `1.13.2`.
  `makepkg --nodeps` skipped dependency-database checks because CMake/Ninja
  were task-local wheels; the recipe's exact installed Hyprland and header
  checks ran. All six native Release CTests passed.
- All 11 Python helper tests passed on the guest and macOS review host.
  Six portable Release CTests also passed on macOS; those are not Linux proof.
- Package metadata pins `hyprland=0.56.2-1`. Its payload contains only the
  module and license plus their parent directories, with no autoload config.
- `nested_lifecycle.py` passed against the freshly built module: two actual
  owned compositors, six mutation refusals per start, a clean restart, old
  connection/socket closure, fresh socket and epoch, and sibling liveness.
  Both children stopped; the original compositor remained responsive and its
  plugin list was empty. This does not claim unchanged parent focus during
  nested-window startup.
- A real `sudo -n pacman -U` attempt refused because the guest requires a sudo
  password. A subsequent package query confirmed that the plugin was not
  installed. **System installation and dependency-upgrade refusal remain
  unverified**; package creation and metadata inspection do not replace them.

| Artifact                                   | SHA-256                                                            |
| ------------------------------------------ | ------------------------------------------------------------------ |
| Exported plugin source and license archive | `4feaafc406f6557ea21b358a5f883cd77053d70a31623483d2ac06b82b4d90e9` |
| Arch package                               | `300cb04ba8c001daeb84f37232fedc135920e5c424ea651d3db1b3cbb3b6d23c` |
| Native module                              | `e179eaa165a20d2f0365e0062c1cca8ac4c4caa9532124417636a603ee4ba73d` |

The remaining merge gates are a recorded discovery-only RFC disposition and
the system package-transaction evidence above. Direct-DRM restart, the exact
physical-host baseline, and input/client certification remain separate
promotion gates. The nonshipping input experiment in #3572 is not included
in this foundation's scope or results.

The claim and pool were deleted after evidence collection. A fresh successful
SDK namespace inventory found zero matching namespaces. The local evidence
archive is retained; no Fleet image or package was published.
