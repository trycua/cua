# Profile-based native rebuilds

This packaging follow-up to [RFC 3550](https://github.com/trycua/cua/issues/3550)
keeps plugin source, packaging tooling, and native qualification separate.
It prepares an optional package for one measured Omarchy x86_64 environment.
A profile or successful build is not native certification.

## Immutable inputs

Preserve the published Driver 0.24.0 plugin source archive and its checksums.
The archive's embedded verifier and manifest describe its historical release
environment. A rebuild kit supplies a separately identified verifier, reviewed
environment profile, recipe, and checksums. The new recipe must verify the
original archive and source inventory, use the kit verifier explicitly, and
not rewrite or invoke the embedded historical verifier.

Record the source archive digest and revision, tooling revision, profile
digest, native build inputs, resulting module and package digests, and separate
qualification evidence. A profile is reviewed data, not an instruction to
accept whatever environment the builder discovers. Preserve exact compositor,
headers, compiler, shared-runtime, source-integrity, and production-build checks.

Schema 1 remains restricted to the original Driver 0.24.0 source revision
`4b3396d9fe4bd3cf723b0eb8db83c18a8764b520`. Schema 2 uses the same profile fields
but allows an explicitly reviewed full source revision and stable Driver version,
with separately selected archive and manifest SHA-256 digests. Both schemas
preserve the selected archive and manifest bytes. The archive stem and package
version follow that validated source identity throughout generation, download
recipe export, and lifecycle checks.

The reviewer selects the source digests as trust roots. An archive inventory,
embedded manifest, or environment measurement cannot authorize its own source.
The source manifest must match the full declared revision and Driver version,
including the corresponding component release-tag field, and retain the original
manifest shape, production build flags, plugin version, architecture, and
historical build-environment fields. Schema 2 does not relax the profile's exact
compositor, compiler, header, runtime, mandatory-test, or activation checks.

Changing the native input implementation or Driver's application admission is
outside a packaging-only rebuild. Such changes need a separately reviewed source
revision and their affected native evidence. Schema 2 supports packaging that
candidate; neither generation nor a manifest's release-tag field proves that
the candidate is released. A local commit with a published version number must
never be relabeled or published as the existing release when its source differs.

## Initial qualification

Measure the intended channel's actual compositor, headers, compiler, shared
runtime, application packages, and installed Driver before committing a native
profile. Keep publication restricted to that one channel while qualifying it.
Do not downgrade the desktop or relax compatibility guards to fit old pins.

Inkscape 1.4.4-6 is the first application candidate because it matches Driver
0.24.0's admission contract. Two-lane proof requires distinct native Wayland
clients and independent Driver processes; two windows alone are insufficient.
Current LibreOffice versions outside the existing admission contract remain
unsupported until separately qualified. Do not silently reduce the two-lane
scope or widen production admission to satisfy a test fixture.

Use the complete native Linux runner selected in the
[test-harnesses guide](../../../docs/test-harnesses-guide.md), plus bounded
application, two-lane overlap, third-owner refusal, primary-input isolation,
conflict, stale-target/geometry, cancellation, and cleanup evidence. Preserve
the canonical runner's assertions. Instrumented diagnostics and the shipped
trace-disabled package require distinct identities and fresh-session results.
Application output and independent primary-input observations remain required.

## Distribution and maintenance

Keep installation opt-in without configuration edits, autoload, install hooks,
or hot replacement. Exact ABI-relevant package dependencies and the documented
consumer compatibility check must cover the supported activation path without
requiring a compiler on the consumer machine.

Before publication, verify real package installation, removal, reinstallation,
fresh-session activation, upgrade, and rollback. When a matching replacement is
unavailable, exit the graphical session, remove the optional plugin, update the
desktop, and verify a fresh session. Disabling input does not remove a package
dependency. Retain a matching rollback set.

Cua owns source tooling and native input qualification. The distribution names
its package-maintenance and signing owner before rollout. Relevant dependency
changes trigger a candidate build and requalification, not an automatic
compatibility claim. Before publication, resolve the exact component tag
`cua-driver-rs-v<driver_version>` and verify that it identifies the profile's full
source revision. A mismatched candidate needs the proper component release and
a profile bound to that exact source. Reuse qualification only for unchanged
source and package bytes; changed bytes require the affected qualification anew.
Download recipe generation only prepares a reviewable future URL; it does not
verify a remote tag or authorize publication.
Manual publication must use the certified package bytes;
unattended distribution requires an enforced artifact-to-evidence gate.

The downstream implementation remains
[omacom/omarchy-pkgs#346](https://github.com/omacom/omarchy-pkgs/pull/346).
This document records the selected contract, not completed package or native
validation.
