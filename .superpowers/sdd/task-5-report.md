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
- Client references are stable and opaque; backend object keys are subject-rooted
  and are never returned as the `reference` field.
- Existing objects return the same reference without an upload instruction.
- S3 uses only AWS SDK v2 in the backend; the SDK request and generated bindings
  contain no AWS credential fields.
