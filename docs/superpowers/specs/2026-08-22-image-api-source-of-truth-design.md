# Image API Source Of Truth Design

**Date:** August 22, 2026
**Status:** Ready for written review

## Goal

Make `trycua/cua` the canonical owner of the Cua Image Kubernetes API. The
repository owns the Image `CustomResourceDefinition`, derives machine-readable
schemas and generated Pydantic models from it, and uses those generated models
to validate every custom-resource manifest emitted by
`Image.to_build_recipe(...)`.

Production clusters consume a versioned CRD artifact from `trycua/cua` while
environment-specific controller deployment, permissions, rollout, and
operations remain owned by `trycua/cloud`.

## Source Of Truth

The canonical API definition is:

```text
clusters/base/cua-images/crd.yaml
```

The CRD defines the complete structural OpenAPI schema for the namespaced
`images.cua.ai/v1alpha1` `Image` resource. Generated JSON Schema, generated
Pydantic code, SDK request construction, deployment artifacts, and API
documentation must all derive from this file.

No handwritten Python, Rust, TypeScript, or cloud-local schema may independently
define the Image fields. Generated outputs are checked in for package consumers
and reviewability, but CI treats them as derived artifacts and fails on drift.

## Repository Layout

`trycua/cua` owns the reusable API and client artifacts:

```text
clusters/base/cua-images/
├── crd.yaml
└── kustomization.yaml

libs/python/cua-sandbox/
├── cua_sandbox/generated/image_models.py
├── schemas/image-v1alpha1.schema.json
├── scripts/generate_image_models.py
└── tests/test_image_build_recipe.py
```

The existing `cua_sandbox.image.Image` class remains the customer-facing image
authoring object. The generated models are validation and transport types, not a
second image-building API.

`trycua/cloud` owns only the production consumption and controller wiring:

```text
clusters/kopf-k3s/flux-system/cua-image-api-source.yaml
clusters/kopf-k3s/flux-system/cua-image-crds-kustomization.yaml
clusters/kopf-k3s/flux-system/cua-image-controller-kustomization.yaml
```

The exact cloud filenames may follow the active Flux naming convention, but the
separation of ownership is fixed: `cua` publishes the API artifact and `cloud`
selects which immutable version production deploys.

## Image Custom Resource

The initial resource is namespaced and uses:

```yaml
apiVersion: images.cua.ai/v1alpha1
kind: Image
```

The CRD enables the status subresource and defines printer columns for phase,
ready state, observed generation, and age. The input schema contains:

- `spec.recipe`: the existing Cua Image recipe represented as structured data;
- `spec.metadata.tags`: bounded user metadata for discovery;
- `spec.build.timeoutSeconds`: a bounded positive build timeout;
- `spec.build.diskSize`: a Kubernetes quantity for the requested disk size;
- `status`: controller-owned observations, conditions, build identity, artifact
  references, and log references.

The first version accepts Linux VM recipes only. It rejects macOS, Windows,
Android, container recipes, local disk paths, snapshot sources, and direct
registry-only descriptors until their remote-build contracts are designed.

The CRD includes structural validation for required fields, enums, string and
collection lengths, recipe document size, destination paths, SHA-256 digests,
and build limits. Large file contents are never embedded in the resource.

## Recipe And File Contract

`spec.recipe` preserves the semantics of the existing immutable `Image`
builder: OS descriptor, ordered layers, environment variables, exposed ports,
and copied-file destinations.

An `Image.copy(source, destination)` entry becomes a manifest reference before
the custom resource is emitted. Each file entry contains:

```yaml
source:
  reference: opaque-tenant-authorized-reference
  digest: sha256:<hex>
  sizeBytes: 1234
destination: /absolute/guest/path
```

Local source paths and file bytes remain client-side and never appear in the
custom resource. `Image.to_build_recipe(...)` requires a resolved file
reference for every copied source and rejects missing, unused, duplicate, or
malformed references. The upload mechanism that creates these references is a
separate implementation phase.

The resource API version is the recipe contract version. The initial design
does not add a second nested schema-version field that can drift from
`apiVersion`.

## Schema Generation

The generator performs these deterministic steps:

1. Parse the CRD and select the served and storage version
   `images.cua.ai/v1alpha1`.
2. Extract `spec.versions[].schema.openAPIV3Schema`.
3. Normalize Kubernetes structural-schema extensions into a standalone JSON
   Schema document without weakening validation.
4. Add the standard custom-resource envelope fields `apiVersion`, `kind`, and
   the supported writable `metadata` subset.
5. Exclude controller-owned `status` from the create-input model while keeping
   separately generated read/status models available for later SDK work.
6. Write `schemas/image-v1alpha1.schema.json` with stable key ordering.
7. Run the pinned `datamodel-code-generator` tool to produce Pydantic v2 models.
8. Format the generated module using the repository's existing Python tooling.

The repository script is the only supported regeneration entry point. It pins
all generator options, model base classes, naming, aliases, optional-field
behavior, and Pydantic version so developer machines and CI produce identical
output.

Generated files contain a header identifying the source CRD path and its
SHA-256 digest. CI regenerates into a temporary directory and fails if either
the JSON Schema or Pydantic output differs from the checked-in files.

## Python API

The public method is added to the existing `Image` class:

```python
def to_build_recipe(
    self,
    *,
    name: str,
    namespace: str,
    tags: Mapping[str, str] | None = None,
    timeout_seconds: int | None = None,
    disk_size: str | None = None,
    file_references: Mapping[str, ImageFileReference] | None = None,
) -> dict[str, Any]:
    ...
```

The method translates the immutable `Image` state into the generated
write-model, validates the complete custom resource with Pydantic, and returns:

```python
validated.model_dump(by_alias=True, exclude_none=True, mode="json")
```

The returned dictionary is ready to send to the Kubernetes/Fleet API. It always
contains `apiVersion`, `kind`, `metadata`, and `spec`; it never contains
`status`. The method does not manually assemble an unchecked nested dictionary.

`Image.to_dict()` keeps its existing local-runtime compatibility behavior. It
is not the remote Image API contract and is not used by
`to_build_recipe(...)`.

## Determinism And Identity

Equivalent `Image` values and method arguments produce byte-equivalent
canonical JSON after model serialization. Recipe hashing uses UTF-8 JSON with
sorted keys and compact separators. The resulting SHA-256 covers `spec.recipe`,
not mutable Kubernetes metadata or controller-owned status.

The client-side digest is diagnostic input for the later controller contract.
Kubernetes Image UID and generation remain the authoritative logical build key;
the digest does not deduplicate builds or replace resource identity.

## Artifact Publication

The Cua release workflow packages `clusters/base/cua-images/` as the OCI
artifact:

```text
ghcr.io/trycua/cua-image-api
```

Every published artifact has an immutable digest and a release tag matching the
Image API version release. Publication runs only after CRD validation,
generation drift checks, Python tests, and Kustomize rendering pass.

Production does not follow the Cua repository's moving `main` branch. The cloud
Flux configuration uses an `OCIRepository` and pins the selected artifact by
digest. Updating production requires an explicit pull request changing that
digest.

The Image controller Kustomization depends on the Image CRD Kustomization, so
the API is established before the controller deployment is reconciled.

## Compatibility

- Existing `Image` constructors and chaining methods remain unchanged.
- Existing `Image.to_dict()` output remains unchanged.
- Existing local QEMU, container, and cloud-init consumers remain unchanged.
- Existing Fleet `containerDiskImage` requests remain the default and are not
  modified in this first slice.
- Generated model changes follow Kubernetes API compatibility rules. Breaking
  field changes require a new served CRD version and an explicit conversion or
  migration plan.
- Cloud pins an immutable API artifact, so publishing a new Cua release cannot
  silently change the production CRD.

## Error Handling

`Image.to_build_recipe(...)` raises a validation error before network activity
for unsupported image kinds, invalid Kubernetes names, absent namespace,
invalid tags, invalid build values, unresolved copied files, malformed file
references, or CRD schema violations.

Generator failures stop without partially replacing checked-in output. The
generation script writes temporary files, validates and formats them, then
atomically replaces generated artifacts only after every step succeeds.

Flux retains the previously reconciled CRD when a new OCI artifact cannot be
fetched or rendered. Controller rollout remains blocked by the CRD dependency
rather than starting against an absent API.

## Testing

The Cua repository adds tests for:

- CRD structural validity and namespaced scope;
- status subresource and required printer columns;
- JSON Schema extraction from the exact CRD version;
- deterministic JSON Schema and Pydantic regeneration;
- generated-model acceptance of a representative Linux VM Image;
- rejection of unsupported runtimes and malformed resources;
- `Image.to_build_recipe(...)` returning the complete validated resource;
- exclusion of `status` and local file paths;
- file-reference completeness and digest validation;
- stable recipe hashing;
- unchanged legacy `Image.to_dict()` behavior;
- Kustomize rendering of the published CRD bundle.

The cloud repository adds contract tests for:

- the Flux `OCIRepository` reference and digest pin;
- the CRD Kustomization path and pruning behavior;
- controller `dependsOn` ordering;
- rendered CRD identity matching `images.cua.ai/v1alpha1`;
- no vendored second copy of the Image CRD.

## Rollout

The first pull request in `trycua/cua` adds the CRD, generation pipeline,
generated Pydantic models, `Image.to_build_recipe(...)`, and focused tests. It
publishes no production artifact until the contract is reviewed.

A following Cua release publishes the first immutable OCI artifact. A separate
cloud pull request introduces the Flux source and CRD Kustomization pinned to
that artifact. The inert Image controller is added only after the CRD deployment
path is healthy.

## Non-Goals

- Implementing the Image controller or builder Job.
- Uploading files or choosing the durable upload service.
- Adding Fleet Image CRUD routes or remote execution.
- Generating or consuming EBS snapshots.
- Changing pool root-disk behavior.
- Supporting non-Linux or non-VM recipes.
- Deploying directly from a moving Cua Git branch.
- Duplicating or hand-maintaining the CRD in `trycua/cloud`.
