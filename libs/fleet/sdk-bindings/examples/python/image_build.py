#!/usr/bin/env python3
"""Upload local inputs and submit a generation-pinned Fleet Image build."""

import argparse
import asyncio
import copy
import json
import os
from pathlib import Path, PurePosixPath
import re
import sys


class BuildError(RuntimeError):
    pass


def image_identity(document):
    metadata = document.get("metadata", {})
    namespace, name = metadata.get("namespace", ""), metadata.get("name", "")
    label = r"[a-z0-9](?:[a-z0-9-]*[a-z0-9])?"
    if (
        not isinstance(namespace, str)
        or not isinstance(name, str)
        or not 1 <= len(namespace) <= 63
        or not 1 <= len(name) <= 253
        or not re.fullmatch(label, namespace)
        or not all(re.fullmatch(label, part) for part in name.split("."))
    ):
        raise BuildError("The Image must have valid metadata.namespace and metadata.name.")
    return namespace, name


def generation_identity(document):
    namespace, name = image_identity(document)
    metadata = document["metadata"]
    uid, generation = metadata.get("uid"), metadata.get("generation")
    if not isinstance(uid, str) or not uid or type(generation) is not int or generation < 1:
        raise BuildError("The API did not return an Image UID and generation.")
    return namespace, name, uid, generation


async def wait_for_image(client, created, wait_seconds, *, poll_seconds=2):
    namespace, name, uid, generation = generation_identity(created)
    if wait_seconds <= 0 or poll_seconds <= 0:
        raise BuildError("Wait and poll durations must be positive.")
    try:
        async with asyncio.timeout(wait_seconds):
            while True:
                document = json.loads((await client.get_image(namespace, name)).to_json())
                if generation_identity(document) != (namespace, name, uid, generation):
                    raise BuildError("The Image identity or generation changed while waiting.")
                if document["metadata"].get("deletionTimestamp"):
                    raise BuildError("The Image is being deleted.")
                status = document.get("status") or {}
                if status.get("observedGeneration") == generation:
                    conditions = status.get("conditions") or []
                    ready = next((item for item in conditions if item.get("type") == "Ready" and item.get("observedGeneration") == generation), {})
                    if ready.get("reason") == "BuildDisabled":
                        raise BuildError("Remote Image builds are disabled; the Image and uploaded inputs are retained.")
                    if status.get("phase") == "Failed":
                        raise BuildError("Image build failed; inspect its generation-pinned conditions.")
                    if status.get("phase") == "Ready" and ready.get("status") == "True":
                        artifacts = status.get("artifacts") or {}
                        oci = artifacts.get("oci") or {}
                        snapshot = artifacts.get("volumeSnapshot") or {}
                        digest, reference = oci.get("digest", ""), oci.get("reference", "")
                        if (
                            not isinstance(digest, str)
                            or not re.fullmatch(r"sha256:[0-9a-f]{64}", digest)
                            or not isinstance(reference, str)
                            or not reference.endswith("@" + digest)
                            or snapshot.get("namespace") != namespace
                            or not snapshot.get("name")
                        ):
                            raise BuildError("Ready Image is missing matching OCI/snapshot artifacts.")
                        return document
                await asyncio.sleep(poll_seconds)
    except TimeoutError as error:
        raise BuildError("Local wait timed out; the Image and build are retained. Inspect with get_image; do not resubmit blindly.") from error


async def run_image_build(client, manifest, local_files, *, wait_seconds=120):
    if not isinstance(manifest, dict) or manifest.get("apiVersion") != "images.cua.ai/v1alpha1" or manifest.get("kind") != "Image":
        raise BuildError("Supply an images.cua.ai/v1alpha1 Image manifest.")
    namespace, name = image_identity(manifest)
    recipe = (manifest.get("spec") or {}).get("recipe")
    if not isinstance(recipe, dict) or not isinstance(recipe.get("files", []), list):
        raise BuildError("Supply an object spec.recipe with an optional files array.")
    if wait_seconds < 0:
        raise BuildError("Wait duration cannot be negative.")
    uploads = []
    for local_path, destination in local_files:
        destination_path = PurePosixPath(destination)
        if not destination_path.is_absolute() or destination_path == PurePosixPath("/") or ".." in destination_path.parts:
            raise BuildError("File destinations must be absolute guest paths without traversal.")
        uploads.append((Path(local_path), destination))
    document = copy.deepcopy(manifest)
    files = document["spec"]["recipe"].setdefault("files", [])
    for path, destination in uploads:
        uploaded = await client.upload_image_file(namespace, path.name, path.read_bytes())
        files.append({
            "source": {"reference": uploaded.reference, "digest": uploaded.digest, "sizeBytes": uploaded.size_bytes},
            "destination": destination,
        })
    from fleet_sdk import PreservedJson

    created = json.loads((await client.create_image(namespace, PreservedJson.from_json(json.dumps(document)))).to_json())
    returned_namespace, returned_name, _, _ = generation_identity(created)
    if (returned_namespace, returned_name) != (namespace, name):
        raise BuildError("Created Image identity did not match the submitted manifest.")
    if wait_seconds == 0:
        return created
    return await wait_for_image(client, created, wait_seconds)


def connect_client():
    from fleet_sdk import CyclopsClient, CyclopsConfiguration, CyclopsCredentials, CyclopsTokenProviderConfiguration

    configuration = {
        "base_url": os.environ["CUA_BASE_URL"],
        "pool_poll_interval_ms": 2000, "pool_poll_limit": 60,
        "claim_poll_interval_ms": 2000, "claim_poll_limit": 60,
    }
    access_token = os.environ.get("CUA_ACCESS_TOKEN")
    if access_token:
        return CyclopsClient.connect_with_access_token_and_native_http_client(
            CyclopsTokenProviderConfiguration(**configuration), access_token,
        )
    return CyclopsClient.connect_with_native_http_client(CyclopsConfiguration(
        **configuration,
        token_url=os.environ["CUA_TOKEN_URL"],
        credentials=CyclopsCredentials(os.environ["CUA_CLIENT_ID"], os.environ["CUA_CLIENT_SECRET"]),
    ))


async def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifest", type=Path, help="Canonical Image JSON manifest")
    parser.add_argument("--file", nargs=2, action="append", default=[], metavar=("LOCAL_PATH", "GUEST_DESTINATION"))
    parser.add_argument("--wait-seconds", type=int, default=120, help="Local wait budget; 0 submits without waiting (default: 120)")
    arguments = parser.parse_args()
    client = connect_client()
    manifest = json.loads(arguments.manifest.read_text())
    result = await run_image_build(client, manifest, arguments.file, wait_seconds=arguments.wait_seconds)
    metadata = result["metadata"]
    status = result.get("status") or {}
    print(json.dumps({
        "namespace": metadata["namespace"], "name": metadata["name"],
        "uid": metadata["uid"], "generation": metadata["generation"],
        "phase": status.get("phase", "Pending"),
        "observedGeneration": status.get("observedGeneration"),
        "artifacts": status.get("artifacts"),
    }, indent=2))


if __name__ == "__main__":
    try:
        asyncio.run(main())
    except BuildError as error:
        print(str(error), file=sys.stderr)
        sys.exit(1)
    except Exception as error:
        status = getattr(error, "status", None)
        detail = f" (HTTP {status})" if isinstance(status, int) else ""
        print(f"Image submission failed: {type(error).__name__}{detail}. No automatic delete or resubmit was attempted.", file=sys.stderr)
        sys.exit(1)
