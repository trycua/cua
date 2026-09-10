# Build a reviewed native-profile package

This kit rebuilds the unchanged Driver 0.24.0 plugin source for one explicitly
reviewed native profile. The historical archive and its embedded manifest and
verifier retain their original bytes. The recipe uses `profile_verify.py` from
this kit. It does not invoke the historical verifier or rewrite source files.

The kit, source, and native profile have separate identities. `PROFILE.json`
contains the profile ID, kit version, package release, source checksums, and
measured compiler, compositor, header-tree and shared-runtime identities.
`KIT-PROVENANCE.json` binds that profile's exact bytes to the committed tooling
and fixed production build options. `SHA256SUMS` includes the recipe, tooling,
profile, manifests and original source archive. None of these files asserts
native certification; use separately reviewed evidence for the exact package
bytes and environment before rollout.

## Prepare and inspect the kit

Obtain the kit and its outer checksum from the reviewed distribution channel.
Do not accept a newly downloaded profile because its values match your machine.
Verify the outer archive checksum against the independently reviewed value,
then extract into a dedicated empty directory and run:

```sh
sha256sum -c SHA256SUMS
```

Review `PKGBUILD`, `PROFILE.json`, and `KIT-PROVENANCE.json`. Retain the reviewed
SHA-256 of `KIT-PROVENANCE.json` separately for lifecycle and consumer checks.
Checksums establish agreement with reviewed files, not publisher authenticity.

Run `makepkg` as an ordinary user. The original source tarball is already in the
kit; no Git checkout or network source resolution is needed. The matching native
compiler, headers, runtime, CMake, Ninja, Python, pkg-config and binutils must
already be installed. To select a compiler outside `/usr/bin/g++`, set
`CUA_RELEASE_CXX` to its absolute path. The kit neither installs a compiler nor
changes runtime search paths or the desktop environment.

The recipe checks exact native package versions, compositor and compiler bytes,
GCC version/date and emitted ELF comment, the package-owned Hyprland header tree,
and matching shared-runtime bytes. The unchanged source requires Hyprland 0.56.2
headers. CMake enables production input, disables experimental input and tracing,
and builds the bundled tests. Packaging runs CTest even with `--nocheck` or
`--repackage`; skipping makepkg integrity checks does not skip the recipe checks.

Build with the system `/usr/bin/pkgconf` and package-owned
`/usr/share/pkgconfig/hyprland.pc`. The canonical Hyprland header tree must lead
pkg-config's include selection: its hashed `protocols` directory may precede
the root, and the root must precede any external include directory. Other
include roots must be real paths under `/usr/include`. Clear pkg-config/CMake
routing overrides, compiler include-path
variables such as `CPATH` and `CPLUS_INCLUDE_PATH`, and flags that inject include
paths, headers, sysroots, toolchains or response files. Ordinary makepkg
optimization and hardening flags remain supported. The verifier refuses these
overrides instead of silently discarding them. It records the actual pkgconf
executable, `.pc` file digests and flags in build provenance, and checks CMake's
cached Hyprland flags against that same canonical selection. These are targeted
build-selection checks, not a sandbox for arbitrary build environments.

## Qualify package transactions

In a disposable matching Arch environment, with ordinary-user build tools and
previously authorized noninteractive sudo for isolated ALPM roots, run:

```sh
python3 lifecycle.py --kit . \
  --revision 4b3396d9fe4bd3cf723b0eb8db83c18a8764b520 \
  --driver-version 0.24.0 --kit-sha256 REVIEWED_KIT_PROVENANCE_SHA256 \
  --output NEW_EVIDENCE_DIRECTORY
```

Add `--cxx /absolute/compiler/path` when needed. The runner validates the fresh
kit, builds the package, and checks its exact payload and provenance. It performs
install, remove, reinstall, and a paired dependency-refusal control in new
isolated ALPM roots. Dependency fixtures contain metadata only; native checks
are performed by the recipe. It does not alter the host package database or load
the module. A pass records kit/profile, package and payload hashes in `RESULT.json`.
Retain logs locally for review. Live activation, restart, upgrade, rollback and
native input qualification remain separate gates.

## Check and activate an installed package

Before installing, upgrading, rolling back, or removing the package, save work
and exit the Hyprland session. Use `pacman -U` from a text console with the exact
reviewed package file. Keep the prior package, matching dependencies and evidence
for rollback. Do not replace or unload the module inside a running compositor.

Installation includes the module, license, source/build/kit/profile provenance,
and the optional verifier. It has no hooks, autoload or configuration edits.
With Python 3.11+, binutils (`readelf`) and the system `ldd` available, check the
installed package using the previously reviewed kit-provenance digest:

```sh
python3 /usr/share/cua-hyprland-plugin/profile_verify.py \
  --kit /usr/share/cua-hyprland-plugin \
  --kit-sha256 REVIEWED_KIT_PROVENANCE_SHA256 \
  --consumer /usr/lib/cua/hyprland/cua-hyprland-plugin.so
```

This check needs no compiler, headers, or pkg-config. It verifies the reviewed
profile/tooling identity, module and build provenance, exact installed ABI package
versions, compositor bytes/compiler comment, and compositor/module shared-runtime
bytes. It does not prove that a running compositor mapped these bytes or that
input works. Run it before deliberate activation in a fresh session. Follow the
separately qualified activation procedure and verify application results.

When ABI dependencies change, obtain a matching qualified package. If none is
available, exit the graphical session, remove the optional plugin package,
update the desktop and verify a fresh session. Disabling input alone does not
remove package dependencies. Rollback requires the matching saved package and
native environment followed by a fresh compositor session.
