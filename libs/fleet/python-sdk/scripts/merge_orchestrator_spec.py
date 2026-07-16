#!/usr/bin/env python3
"""Merge the osgym orchestrator's OpenAPI spec into the parent SDK spec.

Rewrites orchestrator paths like /lanes → /api/gateway/{pool}/lanes
and adds a {pool} path parameter. Merges schemas into components.
Only merges /lanes endpoints (not /reset, /step, etc. which are
already covered by the batch driver).
"""
import argparse
import copy
import json
import sys

import yaml


# Paths from the orchestrator to include (prefix match)
INCLUDE_PREFIXES = ["/lanes"]

# Schemas to always skip (already in parent or not needed)
SKIP_SCHEMAS = {"HTTPValidationError", "ValidationError"}


def rewrite_path(path: str) -> str:
    """Rewrite /lanes/... → /api/gateway/{pool}/lanes/..."""
    return f"/api/gateway/{{pool}}{path}"


def add_pool_param(operation: dict) -> None:
    """Add {pool} path parameter to an operation if not present."""
    params = operation.setdefault("parameters", [])
    if any(p.get("name") == "pool" for p in params):
        return
    params.insert(0, {
        "name": "pool",
        "in": "path",
        "required": True,
        "schema": {"type": "string"},
        "description": "Pool name (DNS-1123 label)",
    })


def fix_ref(obj, old_prefix="#/components/schemas/", renames=None):
    """Recursively fix $ref pointers after schema renames."""
    if isinstance(obj, dict):
        if "$ref" in obj:
            ref = obj["$ref"]
            if ref.startswith(old_prefix) and renames:
                schema_name = ref[len(old_prefix):]
                if schema_name in renames:
                    obj["$ref"] = f"{old_prefix}{renames[schema_name]}"
        for v in obj.values():
            fix_ref(v, old_prefix, renames)
    elif isinstance(obj, list):
        for item in obj:
            fix_ref(item, old_prefix, renames)


def merge(parent_path: str, orch_path: str, output_path: str) -> None:
    with open(parent_path) as f:
        parent = yaml.safe_load(f)
    with open(orch_path) as f:
        orch = json.load(f)

    orch_paths = orch.get("paths", {})
    orch_schemas = orch.get("components", {}).get("schemas", {})

    # Determine which schemas are referenced by included paths
    included_paths = {}
    for path, methods in orch_paths.items():
        if not any(path.startswith(p) for p in INCLUDE_PREFIXES):
            continue
        included_paths[path] = methods

    # Rename schemas with module prefixes (e.g. lane_endpoints__ErrorResponse → LaneErrorResponse)
    renames = {}
    for name in orch_schemas:
        if "__" in name:
            # lane_endpoints__ErrorResponse → LaneErrorResponse
            new_name = name.split("__")[-1]
            # Prefix with "Lane" if it would conflict
            if new_name in orch_schemas and new_name != name:
                new_name = f"Lane{new_name}"
            renames[name] = new_name

    # Add rewritten paths to parent
    parent_paths = parent.setdefault("paths", {})
    for path, methods in included_paths.items():
        new_path = rewrite_path(path)
        methods = copy.deepcopy(methods)
        for method, operation in methods.items():
            if isinstance(operation, dict):
                add_pool_param(operation)
                # Tag as "lanes" for SDK grouping
                operation["tags"] = ["lanes"]
                fix_ref(operation, renames=renames)
        parent_paths[new_path] = methods

    # Add schemas to parent
    parent_schemas = parent.setdefault("components", {}).setdefault("schemas", {})
    for name, schema in orch_schemas.items():
        if name in SKIP_SCHEMAS:
            continue
        final_name = renames.get(name, name)
        if final_name not in parent_schemas:
            schema = copy.deepcopy(schema)
            fix_ref(schema, renames=renames)
            parent_schemas[final_name] = schema

    with open(output_path, "w") as f:
        yaml.dump(parent, f, default_flow_style=False, sort_keys=False, allow_unicode=True)

    added = len(included_paths)
    print(f"   Merged {added} path(s), {len(orch_schemas) - len(SKIP_SCHEMAS)} schema(s)")


if __name__ == "__main__":
    p = argparse.ArgumentParser()
    p.add_argument("--parent", required=True)
    p.add_argument("--orchestrator", required=True)
    p.add_argument("--output", required=True)
    args = p.parse_args()
    merge(args.parent, args.orchestrator, args.output)
