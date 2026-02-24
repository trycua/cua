"""Reusable KiCad netlist comparison for tasks that generate .net files.

Parse KiCad s-expression netlists and compare candidate output to a reference
by component set (ref, value, lib/part) and net connectivity. Use from
evaluate by passing task metadata with ``reference_netlist`` (path to the
reference .net file, relative to the task directory).
"""

from __future__ import annotations

import re
from pathlib import Path
from typing import Any


def _tokenize_sexpr(text: str) -> list[str]:
    """Tokenize s-expression: '(', ')', quoted strings, symbols."""
    tokens: list[str] = []
    i = 0
    n = len(text)
    while i < n:
        if text[i].isspace():
            i += 1
            continue
        if text[i] == "(":
            tokens.append("(")
            i += 1
            continue
        if text[i] == ")":
            tokens.append(")")
            i += 1
            continue
        if text[i] == '"':
            j = i + 1
            while j < n and text[j] != '"':
                if text[j] == "\\":
                    j += 2
                    continue
                j += 1
            tokens.append(text[i : j + 1])
            i = j + 1
            continue
        # symbol
        j = i
        while j < n and not text[j].isspace() and text[j] not in "()":
            j += 1
        tokens.append(text[i:j])
        i = j
    return tokens


def _parse_sexpr(tokens: list[str], pos: int) -> tuple[Any, int]:
    """Parse tokens into nested lists. Returns (tree, next_pos)."""
    if pos >= len(tokens):
        return [], pos
    if tokens[pos] != "(":
        return tokens[pos], pos + 1
    pos += 1
    result: list[Any] = []
    while pos < len(tokens) and tokens[pos] != ")":
        elem, pos = _parse_sexpr(tokens, pos)
        result.append(elem)
    return result, (pos + 1) if pos < len(tokens) else pos


def _find_in_tree(tree: Any, name: str) -> list[Any] | None:
    """Find (name ...) in tree. KiCad netlist has (export ... (design ...) (components ...) (nets ...))."""
    if not isinstance(tree, list) or not tree:
        return None
    if tree[0] == name:
        return tree
    for child in tree:
        found = _find_in_tree(child, name)
        if found is not None:
            return found
    return None


def _get_field(node: list[Any], key: str) -> str | None:
    """From (comp (ref "X") (value "Y") ...) get value for key (ref, value, etc.)."""
    for item in node[1:]:
        if isinstance(item, list) and len(item) >= 2 and item[0] == key:
            v = item[1]
            if isinstance(v, str) and v.startswith('"') and v.endswith('"'):
                return v[1:-1].replace('\\"', '"')
            return str(v) if v is not None else None
    return None


def _get_libsource(node: list[Any]) -> tuple[str | None, str | None]:
    """From (comp ... (libsource (lib "L") (part "P")) ...) return (lib, part)."""
    for item in node[1:]:
        if isinstance(item, list) and item and item[0] == "libsource":
            lib = _get_field(item, "lib")
            part = _get_field(item, "part")
            return (lib, part)
    return (None, None)


def _collect_comp_refs(node: list[Any], out: list[dict[str, Any]]) -> None:
    """Recursively find (comp ...) and append {ref, value, lib, part} to out."""
    if not isinstance(node, list):
        return
    if node and node[0] == "comp":
        ref = _get_field(node, "ref")
        value = _get_field(node, "value")
        lib, part = _get_libsource(node)
        if ref is not None:
            out.append({"ref": ref, "value": value or "", "lib": lib or "", "part": part or ""})
        return
    for child in node:
        _collect_comp_refs(child, out)


def parse_kicad_netlist(content: str) -> dict[str, Any]:
    """Parse KiCad .net (s-expression) content into components and nets.

    Returns:
        {
            "components": [{"ref": "R1", "value": "100", "lib": "Device", "part": "R_US"}, ...],
            "nets": [((ref, pin), (ref, pin), ...), ...]  # each net is sorted tuple of (ref, pin)
        }
    """
    content = content.strip()
    if not content:
        return {"components": [], "nets": []}

    # Handle (export ...) wrapper
    tokens = _tokenize_sexpr(content)
    if not tokens or tokens[0] != "(":
        return {"components": [], "nets": []}
    tree, _ = _parse_sexpr(tokens, 0)

    components: list[dict[str, Any]] = []
    comps_section = _find_in_tree(tree, "components")
    if comps_section is not None:
        _collect_comp_refs(comps_section, components)

    nets: list[tuple[tuple[str, str], ...]] = []
    nets_section = _find_in_tree(tree, "nets")
    if nets_section is not None:
        net_nodes: list[tuple[str, str]] = []
        for child in nets_section[1:]:
            if isinstance(child, list) and child and child[0] == "net":
                nodes: list[tuple[str, str]] = []
                for sub in child[1:]:
                    if isinstance(sub, list) and sub and sub[0] == "node":
                        r = _get_field(sub, "ref")
                        p = _get_field(sub, "pin")
                        if r is not None and p is not None:
                            nodes.append((r, p))
                if nodes:
                    nets.append(tuple(sorted(nodes)))

    return {"components": components, "nets": nets}


def _normalize_value(value: str) -> str:
    """Normalize component value for comparison (e.g. '100' vs '100Ω')."""
    if not value:
        return ""
    # Strip common suffixes and whitespace
    v = value.strip()
    v = re.sub(r"\s*[ΩohmOHM]\s*$", "", v, flags=re.IGNORECASE)
    return v


def _component_key(c: dict[str, Any]) -> tuple[str, str, str]:
    """Sort key for components: ref prefix (R, D, BT, etc.), normalized value, part."""
    ref = c.get("ref", "")
    prefix = ref.rstrip("0123456789") or ref
    value = _normalize_value(c.get("value", ""))
    part = (c.get("part") or c.get("value") or "").strip()
    return (prefix.upper(), value, part)


def _nets_match(ref_nets: list, cand_nets: list) -> bool:
    """True if both have the same set of nets (same (ref, pin) sets)."""
    ref_set = set(ref_nets)
    cand_set = set(cand_nets)
    return ref_set == cand_set


def compare_kicad_netlists(
    candidate_content: str,
    reference_content: str,
    *,
    require_same_components: bool = True,
    require_same_nets: bool = True,
) -> float:
    """Compare candidate KiCad netlist to reference. Returns score in [0.0, 1.0].

    - If both empty or invalid, returns 0.0.
    - Components are compared by set of (ref_prefix, normalized value, part).
      Order and ref numbers (R1 vs R2) can differ; count and types must match.
    - Nets are compared by set of connections: each net is a set of (ref, pin).
      Net names/codes are ignored; topology must match.

    Args:
        candidate_content: Raw content of the candidate .net file.
        reference_content: Raw content of the reference .net file.
        require_same_components: If True, component set must match for full score.
        require_same_nets: If True, net set must match for full score.

    Returns:
        Score 0.0 (no match) to 1.0 (exact match). If both components and nets
        are required, returns 1.0 only when both match; otherwise returns
        weighted average (0.5 each when both required).
    """
    ref = parse_kicad_netlist(reference_content)
    cand = parse_kicad_netlist(candidate_content)

    if not ref["components"] and not ref["nets"]:
        # Reference is empty/invalid: treat as no reference
        return 0.0 if (cand["components"] or cand["nets"]) else 1.0

    scores: list[float] = []

    if require_same_components:
        ref_keys = sorted(_component_key(c) for c in ref["components"])
        cand_keys = sorted(_component_key(c) for c in cand["components"])
        comp_ok = ref_keys == cand_keys
        scores.append(1.0 if comp_ok else 0.0)

    if require_same_nets:
        nets_ok = _nets_match(ref["nets"], cand["nets"])
        scores.append(1.0 if nets_ok else 0.0)

    if not scores:
        return 1.0
    return sum(scores) / len(scores)


def load_reference_netlist(reference_path: str | Path, task_dir: Path) -> str:
    """Load reference netlist file. Path can be absolute or relative to task_dir."""
    p = Path(reference_path)
    if not p.is_absolute():
        p = task_dir / p
    return p.read_text(encoding="utf-8", errors="replace")
