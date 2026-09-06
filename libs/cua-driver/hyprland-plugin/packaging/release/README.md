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
the standalone operator `README.md` from `USAGE.md`, `lifecycle.py`, and
`SHA256SUMS`.
The package version follows the Driver release version; the plugin's
CMake version is recorded separately.

For component release assets, add `--release-assets`. This mode requires the
exact `cua-driver-rs-vDRIVER_VERSION` tag to resolve to `COMMIT_SHA` and emits
only two uniquely named archives:

- `cua-hyprland-plugin-DRIVER_VERSION-COMMIT_SHA.tar.gz`: pinned source.
- `cua-hyprland-plugin-DRIVER_VERSION-COMMIT_SHA-build-kit.tar.gz`: `PKGBUILD`,
  `SOURCE-PROVENANCE.json`, operator `README.md`, `lifecycle.py`, and `SHA256SUMS`.

The Driver release workflow generates these assets for stable component tag
builds and publishing dispatches, with the same source override as the native
Driver builds. The tag and committed Cargo version must match that source.
Nightly and manual build-only runs do not generate plugin release assets.
Recovery of historical tags that contain no bundler produces Driver-only
assets. If a tag contains a bundler but generation fails, publication is blocked.
Publication waits for this job and includes both archives in the component
release checksums. The kit's `SHA256SUMS` remains inside its archive to avoid
colliding with other release assets. Existing output directories are refused.

The default local preparation mode does not require a tag. Neither mode resolves
a generic latest release. Review the generated assets before publication.
Checksums establish consistency with the reviewed recipe; they are
not an independent signature or native certification.
The manifest's `native_certified: false` describes the generator's evidence
scope. An independently certified release must publish its separate native
evidence for the exact revision and environment; this field does not negate it.

## Build without a checkout

Download both archives from the same exact component release and verify them
against its published checksums. Extract the build kit into a dedicated empty
directory and place the source tarball alongside the extracted files. Verify
`SHA256SUMS` from that directory. Use the generated `README.md` for installation,
activation, upgrade, restart, and rollback instructions.
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

To qualify a candidate before publishing, generate the default development kit
from the exact committed candidate SHA. Copy that directory to a disposable
pinned Arch environment and run its `lifecycle.py` as documented in `USAGE.md`.
The runner builds without a checkout, uses the recipe's compiler and ABI checks,
and qualifies package transactions in fresh isolated ALPM roots. It leaves any
running desktop untouched. Its dependency packages contain synthetic metadata;
they prove package-manager constraints, while the native build proves the build
environment checks. They do not prove runtime activation or restart safety.

Retain the package, build provenance, transaction logs, and `RESULT.json`.
Only a successful run writes `RESULT.json`; failed runs retain diagnostics.
Keep raw logs local until reviewed for machine paths and environment details.
The runner leaves its isolated roots and build output for inspection; dispose
of them with the disposable test environment. Native input certification,
fresh-session activation, upgrade/rollback across compositor restarts, and
published component-download verification remain separate release gates.

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
