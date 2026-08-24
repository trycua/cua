# Task 5 Report: Presigned Image Upload API

## Scope Delivered

- Added `POST /api/image-uploads/presign` with bounded request validation,
  stable opaque references, fail-closed namespace authorization, and injected
  `ImageObjectStore` support.
- Added the ambient-credential AWS SDK v2 S3 adapter. The Rust SDK accepts no
  AWS credentials and exposes only the Cyclops control-plane request/response
  records.
- Added image upload configuration, route/policy registration, policy plan and
  authorization-table goldens, SDK method/records, and regenerated canonical
  bindings.

## RED Evidence

1. `go test ./handlers ./config -run 'ImageUpload|ImageUploads' -count=1`
   initially failed on the missing image upload request types, object-store
   interface, configuration, and handler method.
2. `go test ./config -run ImageUpload -count=1` failed because
   `Configuration.ImageUploads` did not exist.
3. `cargo test -p cyclops-sdk --test image_upload_flow` failed on missing
   `ImageUploadFileRequest`, `ImageUploadRequest`, and
   `CyclopsClient.presign_image_uploads`.
4. `go test ./auth -run TestRouteAuthorizationCharacterization -count=1`
   reported only the expected added rows for `/api/image-uploads/presign`.
5. `./libs/fleet/scripts/generate-sdk-bindings.sh --check` failed while the
   bindings were stale; generation also exposed the intentional Ruby future
   wrapper invariant increase from 26 to 27.
6. A final handler test for a 64-character namespace failed with `403`, proving
   it reached RBAC before local validation; the implementation was then
   tightened to reject it with `400` before any store call.

## GREEN Evidence

- `go test ./handlers ./config ./auth -count=1` passed.
- `cargo test --manifest-path libs/fleet/Cargo.toml -p cyclops-sdk --test image_upload_flow --test binding_generation` passed.
- `./libs/fleet/scripts/generate-sdk-bindings.sh` completed and
  `./libs/fleet/scripts/generate-sdk-bindings.sh --check` passed.
- `git diff --check` passed.

## Environment Note

The linked worktree lacks the untracked local replacement expected by
`libs/fleet/backend/go.mod` at `libs/pkg/featureflags`. A temporary symlink to
its clean sibling-worktree copy was used only while running Go tests and was
removed before this report and commit. The optional root-package command
`go test . -count=1` remains blocked by a separate missing fixture:
`clusters/kopf-k3s/cyclops-cs/backend-deployment.yaml`.

## Self-Review

- Validation runs before `Exists` or `PresignPut`: DNS label (including 63-char
  maximum), digest format, nonzero/count bounds, and configured size bounds.
- Namespace access allows a token grant first and otherwise uses the existing
  RBAC probe; a negative or probe error denies before any object-store call.
- Client references are stable and opaque; backend object keys are namespace-rooted
  and are never returned as the `reference` field.
- Existing objects return the same reference without an upload instruction.
- S3 uses only AWS SDK v2 in the backend; the SDK request and generated bindings
  contain no AWS credential fields.

## Review Findings Follow-up (2026-08-24)

### RED Evidence

1. `go test ./handlers -run 'PresignImageUploads' -count=1` failed with:
   - an own-namespace `key-*` principal receiving `403`;
   - an out-of-namespace `key-*` principal receiving `200` when an unrelated
     `AllowedNamespaces` entry matched;
   - references and backend keys depending on the service-account subject; and
   - invalid empty, separator-bearing, dot, control-character, oversized, and
     raw-invalid-UTF-8 names reaching the object store instead of returning
     `400`.
2. `go test ./config . -run 'RequiresImageUploadBucket|NewImageObjectStoreRejectsMissingBucket' -count=1`
   failed because empty buckets were accepted and the startup object-store
   constructor did not exist.
3. A focused compatibility assertion failed because
   `sdk-bindings/ts-uniffi/fleet_sdk.ts` omitted `ImageUploadRequest`; the same
   stale generation affected the Go and browser TypeScript compatibility
   surfaces.

### GREEN Evidence

- `go test ./handlers ./config ./auth -count=1` passed, including authoritative
  `key-*` namespace-claim authorization, credential-stability, strict filename,
  and zero-store-call validation cases.
- `go test . -run 'NewImageObjectStoreRejectsMissingBucket' -count=1` passed.
- `cargo test --manifest-path libs/fleet/Cargo.toml -p cyclops-sdk --test image_upload_flow --test binding_generation`
  passed.
- `./libs/fleet/scripts/generate-sdk-bindings.sh` and
  `./libs/fleet/scripts/generate-sdk-bindings.sh --check` passed.
- `./libs/fleet/scripts/test-generate-sdk-bindings.sh` passed after regenerating
  Go, Node TypeScript, and browser TypeScript compatibility bindings and adding
  focused record/method drift assertions.

### Review Resolution

- Per-key principals now fail closed against the authoritative `namespace`
  claim. Tenant references and backend-only S3 keys derive from the validated
  tenant namespace, never from a rotating service-account key subject.
- File names are validated before authorization or object-store access as
  non-empty UTF-8 base names of at most 255 bytes, excluding separators, dot
  path components, and control characters.
- `IMAGE_UPLOAD_BUCKET` is required by server configuration, and startup creates
  the S3-backed store unconditionally after configuration succeeds. This repo
  intentionally fails closed instead of silently registering a disabled route.
- Canonical deployment environment wiring lives downstream in the Cloud
  deployment repository; this change makes that downstream wiring an explicit
  startup requirement rather than silently disabling uploads here.
- The SDK remains control-plane only: generated records contain no AWS
  credentials, and S3 object keys remain backend-only.

### Self-Review

- Confirmed validation order is body UTF-8/JSON, namespace/count/digest/size/name,
  authentication/authorization, then `Exists`/`PresignPut`.
- Confirmed a `key-*` token cannot escape its namespace claim even if other
  grants or RBAC would otherwise allow the requested namespace.
- Confirmed two service-account subjects for the same tenant produce identical
  references and S3 keys, while the client response never exposes the key.
- Confirmed missing or whitespace-only buckets stop configuration/startup before
  router construction and AWS credential discovery.
