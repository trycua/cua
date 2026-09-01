# Task 2 Report: Add Image References to VM Templates

## Commit

- `d29e2d23e8468ea3e0a76c233d4701fa6f824ef8` `feat(fleet): add Image references to VM templates`

## Files Changed

- `libs/fleet/sdk-schema/src/common.rs`
- `libs/fleet/sdk-schema/src/lib.rs`
- `libs/fleet/sdk-schema/tests/ffi_models.rs`
- `libs/fleet/sdk-schema/tests/uniffi_builder.rs`
- `libs/fleet/uniffi-builder-derive/src/lib.rs`
- `libs/fleet/sdk-bindings/python/fleet_sdk/_schema.py`
- `libs/fleet/sdk-bindings/kotlin/ai/cua/cyclops/sdk/schema/cyclops_sdk_schema.kt`
- `libs/fleet/sdk-bindings/ruby/cyclops_sdk/schema.rb`
- `libs/fleet/sdk-bindings/swift/CyclopsSdkSchema.swift`
- `libs/fleet/sdk-bindings/swift/CyclopsSdkSchemaFFI.h`

## RED Evidence

- `cargo test -p fleet-sdk-schema --test ffi_models --test uniffi_builder`
  exited 101 before compilation because this repository's package is named
  `cyclops-sdk-schema`, not `fleet-sdk-schema`.
- `cargo test -p cyclops-sdk-schema --test ffi_models --test uniffi_builder`
  exited 101 after the contract tests were added. Rust reported that
  `ImageRefBuilder` and `VmTemplateBuilder::image_ref` did not exist.
- `cargo test -p uniffi-builder-derive emits_an_optional_validation_hook`
  exited 101 after the focused macro test was added. The existing derive
  attribute parser rejected `validate = crate::validate_record` as an
  unexpected token.

## GREEN Evidence

- `cargo fmt --manifest-path libs/fleet/Cargo.toml --all --check` exited 0.
- `cargo test -p uniffi-builder-derive` exited 0: 7 tests passed.
- `cargo test -p cyclops-sdk-schema --test ffi_models --test uniffi_builder`
  exited 0: 21 tests passed.
- `cargo test -p cyclops-sdk-schema` exited 0: all schema, CRD, builder, and
  documentation tests passed. The branch has a pre-existing relocated CRD
  path, so the generated CRD was placed temporarily at `libs/clusters/base/osgym/crd.yaml`
  for the test and removed afterward.
- `cargo run --locked -p cyclops-sdk-schema --bin generate-crds -- --output ../clusters/base/osgym/crd.yaml`
  and its matching `--check` command both exited 0 using that temporary
  compatibility location. No authoritative OSGym CRD artifact is tracked on
  this branch, so no CRD file was committed.
- `./libs/fleet/scripts/generate-sdk-bindings.sh` is pre-existingly pinned to
  the removed `libs/cyclops-cs` workspace and fails its canonical-manifest
  guard in this branch. An ephemeral copy with only the workspace path rebased
  to `libs/fleet` ran the same generation logic successfully; its `--check`
  exited 0 and produced the committed Python, Kotlin, Ruby, and Swift bindings.
- `git diff --check` exited 0 before commit; `git show --check HEAD` exited 0
  after commit.

## Self-Review

- `ImageRef` is a canonical UniFFI/Serde/JsonSchema record with a DNS-label
  constrained `name`, and it is exported with `ImageRefBuilder`.
- `VmTemplate` serializes both root-source fields as optional camelCase values.
  KubeVirt requires exactly one of `container_disk_image` or `image_ref`;
  macOS and gVisor require `container_disk_image` and reject `image_ref`.
- The new `validate` hook is opt-in in `UniffiBuilder`, so all existing builders
  preserve their prior construction behavior.
- Focused FFI tests cover the image-ref serialization shape and builder tests
  cover accepted KubeVirt image references, XOR rejection, pod image-ref
  rejection, and required pod container images.
- Generated bindings expose `ImageRef`, `ImageRefBuilder`, `VmTemplate.image_ref`,
  and the optional `container_disk_image` fields. No Cloud mirror files changed.

## Review Remediation (2026-08-24)

### Changes

- Regenerated and committed the Go, Node.js, and browser/WASM UniFFI snapshots.
  The browser bridge now retains the generator-owned `fleet_sdk_module.rs` and
  removes the obsolete `cyclops_sdk_module.rs`.
- Updated `generate-sdk-bindings.sh` and its regression harness to resolve the
  canonical `libs/fleet` workspace directly. The focused harness assertions now
  verify `ImageRef` and optional `VmTemplate` root sources in Go, Node.js, and
  browser TypeScript output.
- Added the authoritative generated CRD bundle at
  `clusters/base/osgym/crd.yaml` and corrected schema tests to include that
  canonical path directly.
- Updated binding documentation to describe the generated builder surfaces and
  canonical workspace paths. Trailing whitespace emitted by unavailable
  third-party formatters was normalized in generated TypeScript and C headers.

### Validation

- `cargo test --locked --manifest-path libs/fleet/Cargo.toml -p cyclops-sdk-schema`
  exited 0.
- `cargo test --locked --manifest-path libs/fleet/Cargo.toml -p uniffi-builder-derive`
  exited 0.
- `cargo test --locked --manifest-path libs/fleet/Cargo.toml -p cyclops-sdk-schema --test claim_crds --test crd_contract --test sandbox_crds`
  exited 0.
- Ran the canonical Python/Kotlin/Swift/Ruby generator and `--check`; `--check`
  exited 0 after regeneration.
- Ran pinned `uniffi-bindgen-go` and `uniffi-bindgen-react-native` generators
  for Go, Node.js, and browser/WASM bindings. The browser clean generation
  emitted only `cyclops_sdk_schema_module.rs` and `fleet_sdk_module.rs` for its
  Rust bridge.
- Focused Go/Node/browser `ImageRef` and optional-root-source assertions and
  `git diff --check` exited 0.

### Remaining Risk

- `libs/fleet/scripts/test-generate-sdk-bindings.sh` was invoked but its
  pre-existing handwritten-fixture cleanup assertion reported a tree-hash
  mismatch after multiple isolated Cargo target builds. Reproducing that
  fixture alone passed and the focused new assertions passed; the full harness
  remains a follow-up validation risk unrelated to the ImageRef checks.
