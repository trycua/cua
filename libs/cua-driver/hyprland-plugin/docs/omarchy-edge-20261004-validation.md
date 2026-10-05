# Omarchy Edge kit qualification: 2026-10-04

Refs [#4216](https://github.com/trycua/cua/issues/4216). This supersedes the
in-progress [2026-10-01 record](omarchy-edge-20261001-validation.md).

The target is an upgraded, disposable x86_64 Omarchy Edge guest. It is not a
fresh ISO installation or Omarchy's Omabot acceptance run.

## Identity

| Item | Value |
| --- | --- |
| Driver release | [`cua-driver-rs-v0.33.2`](https://github.com/trycua/cua/releases/tag/cua-driver-rs-v0.33.2), source `c82d32e3e1adbc6578a148962002ddf6e3e8a15a` |
| Plugin source archive | `cua-hyprland-plugin-0.33.2-c82d32e3e1adbc6578a148962002ddf6e3e8a15a.tar.gz`, SHA-256 `79596a9a5e7300546ad4ab0646430edcf3e0c0456a541458de2a52b8c44a8fad` (matches the published asset digest) |
| Packaging tooling | `c82d32e3e1adbc6578a148962002ddf6e3e8a15a` |
| Profile | [`omarchy-edge-20261004`](../packaging/release/profiles/omarchy-edge-20261004.json), kit `1.3.0`, package release `2`, SHA-256 `0ada2c227d0817b0228e9d8127e2363a785c13175a9b74fbebaedc9825d3a4fd` |
| `KIT-PROVENANCE.json` | SHA-256 `e1c1653045149af9e1fba077a5bd91008c3178a180e62ac06dac9f19c6db5fbe` |
| Kit archive | SHA-256 `ac560bba4bc656004564fd9cc09daea1206402148825ca7d7ac27ccdd56345b1` |
| Production package | `cua-hyprland-plugin-0.33.2-2-x86_64.pkg.tar.zst`, SHA-256 `669773904b972855cf9d08b181d6c13b2b8ddc1726f22c2e4e2ff28da970e7b3` |
| Module | SHA-256 `9cc1ea1bb7fe2b025f9980c38b142cab1ffd0e13a3d74cf6fbdfc36e95aba837`; production input on, experimental input and tracing off |
| Published Driver binary | `cua-driver-rs-0.33.2-linux-x86_64-binary.tar.gz`, SHA-256 `845d0c4eb15d9baaf1343e53074dbfbbef5adcc4e415526b929e549d250f221b`; executable `a818162f9c997548473598b9c73081365830156360e3f6e4a77723ab47f5e78e` |
| Proof harness | `bbea0acf875bfe113f052fa65720e05880b21ecd` (this branch before the main merge) |

## Environment

Measured on Omarchy Edge, 2026-10-04:

- **Compositor:** Hyprland `0.56.2-4` with headers `0.56.2`. The header-inventory hash is `1fdefe6ac027a159d04a5dfee4928ec7ebd15544a9a25b5f66d2f5a46fcf364a`, the same value Omarchy measured for its own `-4` profile.
- **Compiler:** GCC `16.2.1 20260810`.
- **Runtime:** Aquamarine `0.15.1-1`, glibc `2.44+r50+g1848099f063e-1`, Mesa `1:26.2.4-1`. The profile pins all 77 runtime ABI package versions.
- **Session:** kernel `7.2.8-arch1-2`, display 1280x800.
- **Application:** Inkscape `1.4.4-6`.

Against the 2026-10-01 profile, Edge changed Hyprland (`-3` to `-4`), Mesa,
glslang, spirv-tools and harfbuzz, so a new profile was required. The compiler
is unchanged.

## Evidence

All results below are for the identities above.

1. **Build and isolated lifecycle.** `lifecycle.py` passes: the native recipe, all 20 bundled CTests, install, removal and reinstallation in isolated ALPM roots, and the paired dependency-refusal control. Its rebuild (package `a790fd7a…`) maps the same module bytes as the production package.
2. **Live package transactions**, each with SDDM stopped and a fresh session verified afterwards. In every fresh session the exact module is mapped, the ABI matches, input protocol 3 is transport-ready, and there are no config errors:
   - Install `0.33.2-2` over Omarchy's own signed `cua-hyprland-plugin 0.32.0-3`.
   - Roll back to that signed `0.32.0-3` package.
   - Upgrade to `0.33.2-2` again.
   - Remove: no module is mapped. The sandbox image's own plugin configuration reports one unknown-key error until reinstallation.
   - Reinstall `0.33.2-2`.
   - Reboot: new boot ID, fresh activation.
3. **Consumer verifier.** `profile_verify.py --consumer` with the reviewed provenance digest passes after the install and after the reboot.
4. **Production single-app smoke** with the published Driver binary and the installed module passes before and after the reboot. A separate GTK foreground fixture holds focus. Inkscape is driven in the background, and its saved SVG verifies a two-pixel move of the rectangle.
5. **Two-lane production proof** with the published Driver binary, the trace-disabled module, two independent Inkscape processes and two Driver runtimes passes:
   - Select, move and save run in parallel across the two lanes, six actions in total.
   - Both saved SVGs verify the move.
   - The independent parked-primary observer records zero violations.
   - The negative control detects a deliberate primary pointer excursion.
   - Continuous isolation and synthetic-input cleanup stay unproven on this trace-disabled path; they belong to the instrumented diagnostics.
6. **Complete native Linux runner** (`scripts/ci/linux/run-rust-e2e.sh`, all suites) passes with the kit module loaded and the Driver built from the tag source: 90 delivered, 42 expected refusals, 0 failed, 0 skipped. The manual foreground-safety (3/3) and native-observation (1/1) gates also pass.
7. **Driver certification.** The Hyprland fixes in 0.33.2 came from #4396. That PR's candidate passed the complete native runner on this Edge environment, together with Omarchy's `0.32.0-3` plugin. It also passed the hosted Linux, Windows and macOS lanes. The canonical Lume gate found no regression against `main`; #4607 tracks the one test that still fails there. The 0.33.2 release gate passed.

Disclosed setup and harness issues:

- The first production smoke stopped as `inspection_only` because no separate foreground window was active.
- The first attempt in a cold session, and the first after the reboot, failed the smoke's 10-second `git status` provenance check before any input. That check is slow on the guest's partial clone. A warm rerun passed.
- The native runner needed an unlocked Secret Service keyring, as the hosted lanes use, and the sentinel `no_anim` rule from the test-harness guide.
- The background smoke also used the Inkscape `no_initial_focus` fixture rule.

No assertion was changed. Evidence archives are retained privately:

- native runner: `8d5c83caf5d3bdfa684ac70bc30f7aa78a53044c96aaae4b331678bafe77cc5e`
- live transactions, smokes and two-lane proof: `43ec60db9210563ca455f64803a1dff3301a3d249945106a32e56a42e58a30e4`

## Supported pairing

The plugin built from this kit pairs with Cua Driver `0.33.2`, input
protocol 3. The evidence uses the published `0.33.2` Driver binary for
application input and the tag source for the native runner. It covers no other
Driver version. The kit plugin reports `keyboard_layout_independent: true` and
`foreground_numlock_compatible: true` from `hyprctl -j cua:status` (#3970).

## Limits

- Native Wayland Inkscape `1.4.4-6` is the only background-input application.
  Background input keeps the canonical US-keymap requirement. Foreground typing
  keeps the upstream equivalence check, so remapped keymaps such as
  `ctrl:swapcaps` remain refused.
- Not covered: Calc 26.8, Chromium/Electron raw background input, XWayland,
  Unicode/IME, non-US layouts and multi-output capture (#4161, #4305).
- Faults beyond the 2026-10-01 bounded cases are not repeated here: active-drag
  primary conflict, active-lease contention, other-lane recovery, same-process
  siblings, PID/address reuse and reacquisition remain unproven.
- The evidence comes from one disposable guest, not arbitrary applications or
  physical hardware.

## Rebuild and requalification

Run `profile_measure.py measure` on the target, then
`profile_measure.py reuse --reviewed` with this profile:

- `profile-unchanged`: only the profile data matches. Tooling, kit, module and
  package bytes, and their evidence, still need a separate check.
- `relabel-rebuild`: build with a higher package release and repeat the package
  lifecycle evidence.
- `rebuild`: a changed compositor build, headers, compiler or runtime package
  needs a new reviewed profile and the affected native qualification.

Every runtime ABI package is pinned exactly, so an Edge update to any of them
makes this package uninstallable until a new profile lands. A new Driver release
needs a new kit bound to its own tag, even when the plugin source is unchanged.
A matching Hyprland version string alone is not sufficient after an Arch rebuild.

## Downstream patches

Omarchy's `downstream.patch` (omacom/omarchy-pkgs#772) adds two behaviours that
this kit's upstream source does not have:

- a typing check that admits keymap remaps by testing only the chords Driver types;
- an IME popup guard that hooks Hyprland's input-method popup functions.

Keep the patch until both land upstream and ship in a qualified kit. Applying
it to this kit changes the source, module and package bytes, so the patched
package needs its own evidence, as the downstream recipe already requires.

## Ownership

Cua owns the source, tooling and the native evidence above. Omarchy owns Omabot
replay, package maintenance and signing, and edge-to-RC-to-stable promotion.
This record does not publish a package into Omarchy's repositories.
