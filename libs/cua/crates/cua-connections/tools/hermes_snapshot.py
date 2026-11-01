#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
# Copyright (c) 2026 Cua AI, Inc.
"""Reads the provider set out of a Hermes checkout without importing it.

    hermes_snapshot.py <hermes-checkout> > hermes-pin.json

The snapshot holds, for every provider id Hermes can resolve, the environment
variables that carry its key, its default base URL and its wire. It is what
`cargo test -p cua-connections` compares the provider files against, so a
provider file never names a variable or URL the pinned Hermes does not use.
Only Python's standard library is needed; nothing from Hermes runs.

Sources, in the order Hermes itself layers them:
  hermes_cli/auth.py                      _REGISTRY_ROWS (the credential registry)
  plugins/model-providers/*/__init__.py   one profile per provider
"""
import ast
import json
import os
import re
import subprocess
import sys


def literal(node, consts):
    """A literal's value, resolving names through `consts`; None when it is computed."""
    try:
        return ast.literal_eval(node)
    except Exception:
        pass
    if isinstance(node, ast.Name):
        return consts.get(node.id)
    if isinstance(node, ast.Tuple) or isinstance(node, ast.List):
        out = [literal(e, consts) for e in node.elts]
        return out
    return None


def module_consts(path):
    """Top-level NAME = <literal> assignments of a module."""
    consts = {}
    tree = ast.parse(open(path, encoding="utf-8").read())
    for n in tree.body:
        if isinstance(n, ast.Assign) and len(n.targets) == 1 and isinstance(n.targets[0], ast.Name):
            v = literal(n.value, consts)
            if v is not None:
                consts[n.targets[0].id] = v
    return consts


def registry_rows(root):
    consts = module_consts(os.path.join(root, "hermes_cli/auth_constants.py"))
    path = os.path.join(root, "hermes_cli/auth.py")
    tree = ast.parse(open(path, encoding="utf-8").read())
    consts.update({k: v for k, v in module_consts(path).items()})
    rows = {}
    for n in ast.walk(tree):
        if not (isinstance(n, ast.AnnAssign) and getattr(n.target, "id", "") == "_REGISTRY_ROWS"):
            continue
        for e in n.value.elts:
            if isinstance(e, ast.Tuple):
                v = [literal(x, consts) for x in e.elts]
                v += [None] * (6 - len(v))
                pid, name, base, env, base_env, auth = v[:6]
                rows[pid] = dict(name=name, base_url=base or "", env=list(env or []),
                                 base_url_env=base_env or "", auth=auth or "api_key")
            elif isinstance(e, ast.Call):
                args = [literal(x, consts) for x in e.args]
                kw = {k.arg: literal(k.value, consts) for k in e.keywords}
                if len(args) < 3 or not isinstance(args[0], str):
                    continue
                pid, name, auth = args[0], args[1], args[2]
                rows[pid] = dict(name=name, base_url=kw.get("inference_base_url") or "",
                                 env=list(kw.get("api_key_env_vars") or []),
                                 base_url_env=kw.get("base_url_env_var") or "", auth=auth)
    return rows


def plugin_profiles(root):
    out = {}
    d = os.path.join(root, "plugins/model-providers")
    for name in sorted(os.listdir(d)):
        p = os.path.join(d, name, "__init__.py")
        if not os.path.exists(p):
            continue
        consts = module_consts(p)
        tree = ast.parse(open(p, encoding="utf-8").read())
        for n in ast.walk(tree):
            if not isinstance(n, ast.Call):
                continue
            kw = {k.arg: literal(k.value, consts) for k in n.keywords if k.arg}
            if "env_vars" not in kw or not isinstance(kw.get("name"), str):
                continue
            out[kw["name"]] = dict(
                display=kw.get("display_name"),
                env=list(kw.get("env_vars") or []),
                base_url=kw.get("base_url") or "",
                api_mode=kw.get("api_mode") or "chat_completions",
                auth=kw.get("auth_type") or "api_key",
                aliases=list(kw.get("aliases") or []),
                models=list(kw.get("fallback_models") or []),
                hidden=bool(kw.get("hidden", False)),
            )
    return out


def static_models(root):
    """The first model Hermes lists for each provider (its own default pick)."""
    path = os.path.join(root, "hermes_cli/models_catalog_static.py")
    consts = module_consts(path)
    tree = ast.parse(open(path, encoding="utf-8").read())
    out = {}
    for n in tree.body:
        target = getattr(n, "target", None) or (n.targets[0] if isinstance(n, ast.Assign) else None)
        name = getattr(target, "id", "")
        if name == "_PROVIDER_MODELS" and isinstance(n.value, ast.Dict):
            for k, v in zip(n.value.keys, n.value.values):
                key = literal(k, consts)
                first = None
                if isinstance(v, ast.List) and v.elts:
                    first = literal(v.elts[0], consts)
                elif isinstance(v, ast.BinOp) and isinstance(v.left, ast.List) and v.left.elts:
                    first = literal(v.left.elts[0], consts)
                if isinstance(key, str) and isinstance(first, str):
                    out[key] = first
        if name == "OPENROUTER_MODELS" and isinstance(n.value, ast.ListComp):
            it = n.value.generators[0].iter
            if isinstance(it, ast.Tuple) and it.elts:
                out["openrouter"] = literal(it.elts[0], consts)
    return out


def main():
    root = sys.argv[1]
    commit = subprocess.run(["git", "-C", root, "rev-parse", "HEAD"], capture_output=True, text=True).stdout.strip()
    rows = registry_rows(root)
    profiles = plugin_profiles(root)
    models_static = static_models(root)
    ids = sorted(set(rows) | set(profiles))
    providers = {}
    for pid in ids:
        r, p = rows.get(pid, {}), profiles.get(pid, {})
        env = list(dict.fromkeys(list(r.get("env", [])) + list(p.get("env", []))))
        # Plugin profiles list the base-URL override variable beside the key variables.
        base_env = r.get("base_url_env", "") or next((e for e in env if e.endswith("_BASE_URL")), "")
        env = [e for e in env if not e.endswith("_BASE_URL")]
        providers[pid] = dict(
            env=env,
            base_url=r.get("base_url") or p.get("base_url") or "",
            base_url_env=base_env,
            api_mode=p.get("api_mode", "chat_completions"),
            auth=r.get("auth") or p.get("auth") or "api_key",
            models=p.get("models", []),
            default_model=models_static.get(pid) or models_static.get(
                {"openai-api": "openai-api", "kimi-coding": "kimi-coding"}.get(pid, pid)) or (p.get("models") or [""])[0],
        )
    version = re.search(r"v?(\d{4}\.\d+\.\d+)", subprocess.run(
        ["git", "-C", root, "describe", "--tags", "--exact-match"], capture_output=True, text=True).stdout or "")
    json.dump({"commit": commit, "version": version.group(1) if version else None,
               "providers": providers}, sys.stdout, indent=1, sort_keys=True)
    print()


main()
