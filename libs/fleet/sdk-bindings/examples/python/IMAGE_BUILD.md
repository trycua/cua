# Submit an Image build from the Python SDK

This example uses the native SDK transport, hashes and uploads local files,
creates an Image only after all uploads succeed, and waits for that exact UID
and generation. It prints the OCI and VolumeSnapshot references when the
controller reports both artifacts Ready. It never deletes or resubmits an Image
automatically and does not run a full restore/boot test per build.

## Current delivery boundary

The SDK method is source functionality, not evidence that a released package or
production tenant controller is enabled. The deployment must provide tenant
Image authorization, an enabled upload bucket/signer, a pinned base catalog,
and an enabled Image controller. Missing upload configuration returns HTTP 503.
An Image with `BuildDisabled` stops the example immediately; it is not a
successful build. Snapshot-backed sandbox launch is a separate delivery gate.

## Local prerequisites

Use Python 3.11+ and the repository's pinned Rust toolchain. The repository
launcher builds the matching native library and stages generated Python bindings
in a temporary package; do not mix this branch's bindings with an older wheel.
Provide credentials authorized for the manifest's namespace through environment
variables, not command-line arguments or checked-in files. Use either an existing
Fleet bearer token in `CUA_ACCESS_TOKEN`, or the client-credentials configuration
below. A static access token is not refreshed automatically:

```bash
export REPO_ROOT=/absolute/path/to/cloud
export CUA_BASE_URL=https://your-fleet-api
export CUA_TOKEN_URL=https://your-identity-provider/token
export CUA_CLIENT_ID=your-client-id
# Supply CUA_CLIENT_SECRET from your normal local secret environment.
```

## Manifest and submission

Save a canonical Image JSON manifest as `image.json`. Use distro/version values
from the deployed, administrator-pinned base catalog. The Ubuntu values below
are illustrative, not a claim that this base is provisioned in your environment.
The current compiler requires a matching 40Gi base; it does not resize disks.

```json
{
  "apiVersion": "images.cua.ai/v1alpha1",
  "kind": "Image",
  "metadata": {"namespace": "your-namespace", "name": "local-build.v1"},
  "spec": {
    "recipe": {
      "osType": "linux",
      "distro": "ubuntu",
      "version": "24.04",
      "kind": "vm",
      "layers": [{"type": "run", "command": "cat /opt/build-input.txt"}]
    },
    "build": {"diskSize": "40Gi", "timeoutSeconds": 7200}
  }
}
```

Use only nonsensitive sample input: the sample command prints its contents to
build logs. Local files are read into memory by this small example. Repeat
`--file LOCAL_PATH GUEST_DESTINATION` to append multiple recipe files:

```bash
printf 'SDK upload example\n' > build-input.txt
"$REPO_ROOT/cyclops-cs/scripts/run-python-sdk-binding.sh" \
  "$REPO_ROOT/cyclops-cs/sdk-bindings/examples/python/image_build.py" \
  image.json --file build-input.txt /opt/build-input.txt --wait-seconds 120
```

The two-minute value is the **local polling budget**, not the server build
lifetime. `--wait-seconds 0` submits without waiting. Timeout leaves the Image
and remote build intact; it is not cancellation or proof of failure. A 409 on
create means you must inspect the existing Image instead of blindly retrying.
A changed UID or generation during polling aborts the wait rather than reporting
another build as the requested one.

## Inspect and clean up explicitly

With a client created using the same configuration:

```python
image = await client.get_image("your-namespace", "local-build.v1")
print(image.to_json())
# Only when you intend to request deletion:
# await client.delete_image("your-namespace", "local-build.v1")
```

Inspect `metadata.uid`, `metadata.generation`, `status.observedGeneration`,
conditions, `status.artifacts.oci` and `status.artifacts.volumeSnapshot`. Do not
publish the entire manifest: recipe environments or file details may be private.
Deletion acceptance is not evidence of backend storage cleanup; observe the
controller's lifecycle and retention behavior before assuming storage is gone.

## Fast local example checks

These checks use a fake SDK client and require no credentials or cluster:

```bash
python3 "$REPO_ROOT/cyclops-cs/sdk-bindings/examples/python/test_image_build.py"
```

They cover upload-before-create ordering, failed uploads, stale Ready status,
replacement/updates, BuildDisabled, bounded waits, and required same-namespace
artifacts. They do not prove S3 persistence, a real controller build, or sandbox
boot. The exact-digest release canary owns those end-to-end checks.
