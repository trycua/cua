# Pinned source release

This directory prepares a standalone Arch recipe and source archive for the
production input candidate. It does not certify native behavior or publish
assets. The existing `../arch/PKGBUILD` remains a discovery-only local recipe.

The candidate requires Linux x86_64, Hyprland headers `0.56.2`, the Arch package
`hyprland=0.56.2-1`, and GCC `16.1.1 20260728`. Both the compositor and a compiler
probe must carry the exact ELF comment `GCC: (GNU) 16.1.1 20260728`. The selected
compiler must resolve the shared runtime `libstdc++.so.6.0.36`. Other architectures
and toolchains need separate validation and an explicit contract update.

## Prepare release assets

After committing the source and these helpers, run the generator with Python
3.11 or later from the release checkout. Replace `COMMIT_SHA` with the full
40-character commit SHA, `DRIVER_VERSION` with its Cargo workspace version, and
`NEW_OUTPUT_DIRECTORY` with a directory that does not yet exist:

```sh
python3 libs/cua-driver/hyprland-plugin/packaging/release/bundle.py \
  --repo . --revision COMMIT_SHA --driver-version DRIVER_VERSION \
  --output NEW_OUTPUT_DIRECTORY
```

The generator reads an explicit allowlist of Git blobs at that commit, including
the root license and the recipe/verifier. It ignores dirty and untracked files,
live test scripts, evidence, and unrelated tracked content. Archive ordering,
ownership, modes, and timestamps are fixed. The manifest records each source
file's SHA-256, the source revision, component version, plugin version, ABI pins,
and build options. The recipe pins the archive and manifest checksums. Generation
refuses an existing output directory and mismatched versions.

The output contains the source tarball, `PKGBUILD`, `SOURCE-PROVENANCE.json`,
the standalone operator `README.md` from `USAGE.md`, and `SHA256SUMS`.
The package version follows the Driver release version; the plugin's
CMake version is recorded separately. Release automation must verify that the
exact `cua-driver-rs-vDRIVER_VERSION` tag resolves to `COMMIT_SHA` before attaching
these assets to that component release. The generator does not resolve a generic
latest release or assert that a tag exists. Review the generated assets before
publication. Checksums establish consistency with the reviewed recipe; they are
not an independent signature or native certification.
The manifest's `native_certified: false` describes the generator's evidence
scope. An independently certified release must publish its separate native
evidence for the exact revision and environment; this field does not negate it.

## Build without a checkout

Download all five assets from the same exact component release into a dedicated
directory. Use the generated `README.md` for installation, activation, upgrade,
restart, and rollback instructions.
Review the recipe, provide the pinned Hyprland package and matching compiler,
and run `makepkg` as an ordinary user. `makepkg` can download the pinned source
tarball itself; no Git checkout is needed.

If the compiler is staged outside `/usr/bin/g++`, select its absolute executable
path with `CUA_RELEASE_CXX` when invoking `makepkg`. The recipe does not download
or install a compiler, configure a runtime search path, or replace a compositor.
The compiler and compositor must use the matching shared C++ runtime in the
target environment. The recipe records the compiler executable, compiler
runtime, compositor, and module hashes as build provenance.
It also checks that `ldd` resolves the compositor and built module to the same
runtime bytes as the compiler. A custom compiler does not waive this check.

The build enables `CUA_HYPRLAND_INPUT=ON`, disables experimental signed input and
input tracing, and runs the bundled CTest suite. Packaging also runs tests, so
`--nocheck` cannot produce an unchecked package. The package contains only the
module, license, and source/build provenance. It has no install hooks, autoload,
configuration edits, or hot replacement. Module activation and compositor
restart belong to the separately validated operator workflow.

## Verify this preparation

Run the focused packaging tests with Python 3.12 or later:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover \
  -s libs/cua-driver/hyprland-plugin/packaging/release -p 'test_*.py'
```

These tests cover deterministic archives, exclusion of unrelated and dirty
content, checksum and version refusals, compiler/ABI contract checks with
synthetic command responses, and the installation file list. They do not replace
native validation. Before shipping, build the generated archive with `makepkg`
in the pinned Arch environment, run the native input and desktop certification
at the exact source SHA, inspect the package contents and ELF dependencies, and
verify clean install, deliberate activation after restart, package upgrade,
removal, and refusal after compositor/compiler drift. Release download and
checksum verification from the published component tag also remain required.
