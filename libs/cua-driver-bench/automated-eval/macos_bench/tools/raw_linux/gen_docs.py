#!/usr/bin/env python3
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Deterministic API docs for the raw-linux docs arm (``pi-raw-linux-docs``), built from the desktop's own files.

Sources, all read from the machine the bench runs on (or fetched at the installed package versions):
- GObject-introspection ``.gir`` files (Atspi, Gdk, GdkX11, GdkPixbuf, Gtk, GLib, GObject, Gio, Wnck, xlib, xfixes,
  xrandr): classes, interfaces, records, enums, functions, methods, properties and signals with their doc text;
- C headers for X11/Xlib, XTest, XRandR, XFixes, XInput2, Xcomposite, Xdamage, XShm, Xinerama, Xrender, XKB and
  libxdo: prototypes, ``typedef struct`` bodies, ``#define`` constants and keysyms, with any doc comment above;
- D-Bus introspection XML (the AT-SPI bus interfaces, xdg-desktop-portal and every interface the image installs);
- Wayland protocol XML (core and wayland-protocols): requests, events and enums;
- Python docstrings and signatures of the bindings a script can import (python-xlib, pyatspi, dbus), read with
  ``ast`` so nothing is imported;
- the generic notes file next to this script (``notes.md``), after ``notes_lint`` passes.

Inputs the image does not install (``FETCH``) are fetched with ``apt-get download`` / ``apt-get source`` at the
version that matches an installed package, into ``--cache``; nothing is installed.

Output: ``<out>/<distro>-<release>-<sha12>/`` where sha12 hashes the sorted ``package=version`` list of every input.
It holds ``index.jsonl`` (one entry per line, sorted by id) and ``manifest.json`` (distro, packages, each input file's
sha256, entry counts). No timestamps, no environment in the output: the same inputs give byte-identical files.

Usage: gen_docs.py fetch --cache DIR; gen_docs.py build --cache DIR --out DIR (prints the output directory).
"""

from __future__ import annotations

import argparse
import ast
import hashlib
import json
import os
import re
import subprocess
import sys
import tarfile
import xml.etree.ElementTree as ET
from pathlib import Path
from typing import Iterable, Iterator

HERE = Path(__file__).resolve().parent
NOTES = HERE / "notes.md"
DOC_CAP = 1500  # characters of doc text kept per entry

GIR = (
    "Atspi-2.0", "Gdk-3.0", "GdkX11-3.0", "GdkPixbuf-2.0", "Gtk-3.0", "GLib-2.0", "GObject-2.0", "Gio-2.0",
    "Wnck-3.0", "xlib-2.0", "xfixes-4.0", "xrandr-1.3",
)
GIR_DIRS = ("/usr/share/gir-1.0", "/usr/lib/aarch64-linux-gnu/gir-1.0", "/usr/lib/x86_64-linux-gnu/gir-1.0")
HEADERS = (
    "X11/Xlib.h", "X11/Xutil.h", "X11/X.h", "X11/Xatom.h", "X11/XKBlib.h", "X11/keysymdef.h",
    "X11/extensions/XTest.h", "X11/extensions/Xrandr.h", "X11/extensions/randr.h", "X11/extensions/Xfixes.h",
    "X11/extensions/XInput2.h", "X11/extensions/XI2.h", "X11/extensions/Xcomposite.h", "X11/extensions/Xdamage.h",
    "X11/extensions/XShm.h", "X11/extensions/Xinerama.h", "X11/extensions/Xrender.h", "X11/extensions/shape.h",
    "xdo.h",
)
PY_PACKAGES = ("Xlib", "pyatspi", "dbus")
PY_DIR = "/usr/lib/python3/dist-packages"
# Not on the image: (package to fetch, installed package whose version it must match, members to keep)
FETCH = (
    ("libwnck-3-dev", "libwnck-3-0", ("usr/share/gir-1.0/Wnck-3.0.gir",)),
    ("xdg-desktop-portal-dev", "xdg-desktop-portal", ("usr/share/dbus-1/interfaces/",)),
)
FETCH_SOURCE = (("at-spi2-core", "at-spi2-core", "xml/"),)  # the AT-SPI bus interfaces ship only in the source


# ---------------------------------------------------------------- helpers


def text(s: str | None) -> str:
    return re.sub(r"\s+", " ", s or "").strip()


def cap(s: str) -> str:
    s = text(s)
    return s if len(s) <= DOC_CAP else s[: DOC_CAP - 1] + "…"


def entry(id_: str, kind: str, source: str, sig: str = "", doc: str = "") -> dict:
    return {"id": id_, "kind": kind, "source": source, "sig": text(sig), "doc": cap(doc)}


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def run(*argv: str, cwd: Path | None = None) -> str:
    return subprocess.run(argv, check=True, capture_output=True, text=True, cwd=cwd, env={
        "PATH": "/usr/bin:/bin", "LANG": "C.UTF-8", "LC_ALL": "C.UTF-8"}).stdout


def owner(path: str) -> str:
    """The installed package that owns PATH, as name=version (multiarch suffix dropped)."""
    name = run("dpkg-query", "-S", path).split(":")[0].strip().split(",")[0].strip()
    return f"{name}={installed(name)}"


def installed(name: str) -> str:
    return run("dpkg-query", "-W", "-f=${Version}", name).strip()


# ---------------------------------------------------------------- GIR

CORE = "{http://www.gtk.org/introspection/core/1.0}"
C_NS = "{http://www.gtk.org/introspection/c/1.0}"
GLIB_NS = "{http://www.gtk.org/introspection/glib/1.0}"


def _doc(node: ET.Element) -> str:
    d = node.find(CORE + "doc")
    return d.text if d is not None and d.text else ""


def _type(node: ET.Element) -> str:
    t = node.find(CORE + "type")
    if t is not None:
        return t.get("name") or t.get(C_NS + "type") or "?"
    a = node.find(CORE + "array")
    if a is not None:
        inner = a.find(CORE + "type")
        return f"[{inner.get('name') if inner is not None else '?'}]"
    return "..." if node.find(CORE + "varargs") is not None else "?"


def _callable(ns: str, owner_: str, node: ET.Element, kind: str, source: str) -> dict:
    name = node.get("name")
    params, docs = [], []
    ps = node.find(CORE + "parameters")
    for p in ps if ps is not None else []:
        if p.tag == CORE + "instance-parameter":
            continue
        direction = p.get("direction")
        label = f"{p.get('name')}: {_type(p)}" + (f" [{direction}]" if direction and direction != "in" else "")
        params.append(label)
        if _doc(p):
            docs.append(f"{p.get('name')}: {text(_doc(p))}")
    rv = node.find(CORE + "return-value")
    ret = _type(rv) if rv is not None else "none"
    if rv is not None and _doc(rv):
        docs.append(f"returns: {text(_doc(rv))}")
    flags = [f"C: {node.get(C_NS + 'identifier')}"] if node.get(C_NS + "identifier") else []
    if node.get("throws") == "1":
        flags.append("throws GLib.Error")
    if node.get("deprecated") == "1":
        flags.append("deprecated")
    full = f"{ns}.{owner_}.{name}" if owner_ else f"{ns}.{name}"
    sig = f"{full}({', '.join(params)}) -> {ret}" + (f"  [{'; '.join(flags)}]" if flags else "")
    return entry(f"gir:{full}", kind, source, sig, " ".join([text(_doc(node))] + docs))


def parse_gir(path: Path, source: str) -> Iterator[dict]:
    root = ET.parse(path).getroot()
    for ns_node in root.iter(CORE + "namespace"):
        ns = ns_node.get("name")
        for node in ns_node:
            tag = node.tag.removeprefix(CORE)
            name = node.get("name")
            if not name:
                continue
            if tag in ("function", "callback"):
                yield _callable(ns, "", node, tag, source)
            elif tag == "constant":
                value = node.get("value")
                yield entry(f"gir:{ns}.{name}", "constant", source, f"{ns}.{name} = {value}", _doc(node))
            elif tag in ("class", "interface", "record", "union"):
                parent = node.get("parent")
                impl = sorted(i.get("name") for i in node.findall(CORE + "implements"))
                sig = f"{tag} {ns}.{name}" + (f"({parent})" if parent else "") + (
                    f" implements {', '.join(impl)}" if impl else "")
                yield entry(f"gir:{ns}.{name}", tag, source, sig, _doc(node))
                for child in node:
                    ctag = child.tag.removeprefix(CORE)
                    if ctag in ("method", "constructor", "function"):
                        yield _callable(ns, name, child, ctag, source)
                    elif ctag == "property":
                        mode = "rw" if child.get("writable") == "1" else "r"
                        yield entry(f"gir:{ns}.{name}:{child.get('name')}", "property", source,
                                    f"{ns}.{name}:{child.get('name')}: {_type(child)} ({mode})", _doc(child))
                    elif ctag == "field" and tag in ("record", "union"):
                        yield entry(f"gir:{ns}.{name}.{child.get('name')}", "field", source,
                                    f"{ns}.{name}.{child.get('name')}: {_type(child)}", _doc(child))
                for sig_node in node.findall(GLIB_NS + "signal"):
                    s = _callable(ns, name, sig_node, "signal", source)
                    s["id"] = f"gir:{ns}.{name}::{sig_node.get('name')}"
                    s["sig"] = s["sig"].replace(f"{ns}.{name}.{sig_node.get('name')}",
                                                f"{ns}.{name}::{sig_node.get('name')}", 1)
                    yield s
            elif tag in ("enumeration", "bitfield"):
                members = [f"{m.get('name').upper()}={m.get('value')}" for m in node.findall(CORE + "member")]
                yield entry(f"gir:{ns}.{name}", tag, source, f"{tag} {ns}.{name}: {', '.join(members)}",
                            _doc(node))
                for m in node.findall(CORE + "member"):
                    if _doc(m):
                        yield entry(f"gir:{ns}.{name}.{m.get('name').upper()}", "member", source,
                                    f"{ns}.{name}.{m.get('name').upper()} = {m.get('value')}", _doc(m))


# ---------------------------------------------------------------- C headers

DOC_COMMENT = re.compile(r"/\*\*(.*?)\*/", re.S)
COMMENT = re.compile(r"/\*.*?\*/", re.S)
PARAM_NAME_COMMENT = re.compile(r"/\*\s*([A-Za-z_]\w*)\s*\*/")
MACROS = re.compile(r"\b(_X_DEPRECATED|_X_SENTINEL\(\d+\)|_X_NORETURN|_XFUNCPROTOBEGIN|_XFUNCPROTOEND|"
                    r"_Xconst|_X_HIDDEN|_X_EXPORT|_X_RESTRICT_KYWD)\b")
FUNC = re.compile(r"^(?:extern\s+)?(?!typedef\b)([\w\s\*]+?)\b([A-Za-z_]\w*)\s*\((.*)\)\s*$", re.S)
STRUCT = re.compile(r"typedef\s+struct\s*\w*\s*\{(.*?)\}\s*(\w+)\s*;", re.S)
DEFINE = re.compile(r"^#\s*define\s+([A-Za-z_]\w*)\s+(.+?)\s*$")


def _clean_doc(raw: str) -> str:
    return text(re.sub(r"^\s*\*+ ?", "", raw, flags=re.M))


def parse_header(path: Path, source: str, lib: str) -> Iterator[dict]:
    src = path.read_text("utf-8", errors="replace")
    # doc comments keyed by the offset where each ends, so a prototype can pick up the one right above it
    docs = {m.end(): _clean_doc(m.group(1)) for m in DOC_COMMENT.finditer(src)}
    seen: set[str] = set()
    for line in src.splitlines():
        m = DEFINE.match(line)
        if m and not m.group(1).startswith("_") and "(" not in m.group(1):
            value = text(COMMENT.sub("", m.group(2)))
            if value and m.group(1) not in seen:
                seen.add(m.group(1))
                yield entry(f"c:{m.group(1)}", "define", source, f"#define {m.group(1)} {value}", "")
    # declarations: drop preprocessor lines (keep offsets), then split at ';' outside braces
    body = re.sub(r"^\s*#.*?(?<!\\)$", lambda m: " " * len(m.group(0)), src, flags=re.M)
    flat = COMMENT.sub(lambda c: " " * len(c.group(0)), body)  # comments blanked, offsets kept
    for m in STRUCT.finditer(flat):
        fields = "; ".join(text(f) for f in m.group(1).split(";") if text(f))
        name = m.group(2)
        if name not in seen:
            seen.add(name)
            yield entry(f"c:{name}", "struct", source, f"typedef struct {{ {fields} }} {name}", "")
    depth, start = 0, 0
    for i, ch in enumerate(flat):
        if ch == "{":
            depth += 1
        elif ch == "}":
            depth -= 1
            start = i + 1
        elif ch == ";" and depth == 0:
            chunk = flat[start:i]
            begin = start + len(chunk) - len(chunk.lstrip())
            start = i + 1
            # the doc comment right above this declaration, if any: only blanks between them
            above = [end for end in docs if end <= begin and not flat[end:begin].strip()]
            doc = docs[max(above)] if above else ""
            decl = PARAM_NAME_COMMENT.sub(r" \1", body[begin:i])
            decl = MACROS.sub("", COMMENT.sub(" ", decl))
            decl = re.sub(r'extern\s+"C"\s*', "", decl).strip()
            fm = FUNC.match(decl)
            if not fm or "(" in fm.group(1) or fm.group(2) in seen:
                continue
            name = fm.group(2)
            if name in ("if", "while", "return", "sizeof"):
                continue
            seen.add(name)
            yield entry(f"c:{name}", "function", source, f"{text(fm.group(1))} {name}({text(fm.group(3))})"
                        + f"  [{lib}]", doc)


# ---------------------------------------------------------------- D-Bus and Wayland XML


def _dbus_args(node: ET.Element) -> str:
    out = []
    for a in node.findall("arg"):
        d = a.get("direction")
        out.append(f"{'out ' if d == 'out' else ''}{a.get('name') or '_'}: {a.get('type')}")
    return ", ".join(out)


def _xml_doc(node: ET.Element) -> str:
    """Doc text of a D-Bus node: gtk-doc style comments sit in XML comments right above, which ElementTree drops;
    org.gtk.GDBus.DocString annotations and <doc:doc> blocks are kept."""
    parts = [a.get("value") or "" for a in node.findall("annotation")
             if a.get("name") == "org.gtk.GDBus.DocString"]
    return " ".join(parts)


def parse_dbus(path: Path, source: str) -> Iterator[dict]:
    raw = path.read_text("utf-8", errors="replace")
    # gtk-doc comments (<!-- Name: ... -->) above each element, by element name, so they survive the parse
    comments = {}
    for m in re.finditer(r"<!--(.*?)-->\s*<(interface|method|signal|property)\s+name=\"([^\"]+)\"", raw, re.S):
        comments[(m.group(2), m.group(3))] = text(m.group(1))
    root = ET.fromstring(re.sub(r"<!DOCTYPE[^>]*>", "", raw))
    for iface in root.iter("interface"):
        iname = iface.get("name")
        yield entry(f"dbus:{iname}", "dbus-interface", source, f"interface {iname}",
                    comments.get(("interface", iname), "") or _xml_doc(iface))
        for tag in ("method", "signal", "property"):
            for node in iface.findall(tag):
                n = node.get("name")
                if tag == "property":
                    sig = f"{iname}.{n}: {node.get('type')} ({node.get('access')})"
                else:
                    sig = f"{tag} {iname}.{n}({_dbus_args(node)})"
                yield entry(f"dbus:{iname}.{n}", f"dbus-{tag}", source, sig,
                            comments.get((tag, n), "") or _xml_doc(node))


def parse_wayland(path: Path, source: str) -> Iterator[dict]:
    root = ET.parse(path).getroot()
    proto = root.get("name")

    def desc(node: ET.Element) -> str:
        d = node.find("description")
        return " ".join(x for x in ((d.get("summary") if d is not None else ""),
                                    (d.text if d is not None else "")) if x)

    for iface in root.findall("interface"):
        iname = iface.get("name")
        yield entry(f"wayland:{iname}", "wayland-interface", source,
                    f"interface {iname} v{iface.get('version')} (protocol {proto})", desc(iface))
        for tag in ("request", "event"):
            for node in iface.findall(tag):
                args = ", ".join(f"{a.get('name')}: {a.get('type')}"
                                 + (f"<{a.get('interface')}>" if a.get("interface") else "")
                                 for a in node.findall("arg"))
                yield entry(f"wayland:{iname}.{node.get('name')}", f"wayland-{tag}", source,
                            f"{tag} {iname}.{node.get('name')}({args})", desc(node))
        for enum in iface.findall("enum"):
            members = ", ".join(f"{e.get('name')}={e.get('value')}" for e in enum.findall("entry"))
            yield entry(f"wayland:{iname}.{enum.get('name')}", "wayland-enum", source,
                        f"enum {iname}.{enum.get('name')}: {members}", desc(enum))


# ---------------------------------------------------------------- Python bindings (ast, nothing imported)


def _py_sig(fn: ast.FunctionDef | ast.AsyncFunctionDef) -> str:
    return f"({ast.unparse(fn.args)})"


def parse_python(path: Path, module: str, source: str) -> Iterator[dict]:
    tree = ast.parse(path.read_text("utf-8", errors="replace"))
    mod_doc = ast.get_docstring(tree) or ""
    if mod_doc:
        yield entry(f"py:{module}", "module", source, f"module {module}", mod_doc)
    for node in tree.body:
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)) and not node.name.startswith("_"):
            yield entry(f"py:{module}.{node.name}", "function", source, f"{module}.{node.name}{_py_sig(node)}",
                        ast.get_docstring(node) or "")
        elif isinstance(node, ast.ClassDef) and not node.name.startswith("_"):
            bases = ", ".join(ast.unparse(b) for b in node.bases)
            yield entry(f"py:{module}.{node.name}", "class", source, f"class {module}.{node.name}({bases})",
                        ast.get_docstring(node) or "")
            for item in node.body:
                if isinstance(item, (ast.FunctionDef, ast.AsyncFunctionDef)) and (
                        not item.name.startswith("_") or item.name == "__init__"):
                    yield entry(f"py:{module}.{node.name}.{item.name}", "method", source,
                                f"{module}.{node.name}.{item.name}{_py_sig(item)}", ast.get_docstring(item) or "")


# ---------------------------------------------------------------- notes


def parse_notes(path: Path) -> Iterator[dict]:
    """``## Topic`` sections of the hand-written generic notes, one entry each."""
    title, lines = None, []
    for line in path.read_text("utf-8").splitlines() + ["## "]:
        if line.startswith("## "):
            if title:
                slug = re.sub(r"[^a-z0-9]+", "-", title.lower()).strip("-")
                yield {"id": f"note:{slug}", "kind": "note", "source": "notes.md", "sig": title,
                       "doc": "\n".join(lines).strip()}
            title, lines = line[3:].strip(), []
        elif title is not None:
            lines.append(line)


# ---------------------------------------------------------------- fetch and build


def fetch(cache: Path) -> None:
    """Download the inputs the image lacks at the installed versions, and unpack only the needed members."""
    cache.mkdir(parents=True, exist_ok=True)
    for pkg, match, members in FETCH:
        version = installed(match)
        dest = cache / f"{pkg}_{version}"
        if dest.is_dir():
            continue
        tmp = cache / "tmp"
        tmp.mkdir(exist_ok=True)
        run("apt-get", "download", f"{pkg}={version}", cwd=tmp)
        deb = next(tmp.glob(f"{pkg}_*.deb"))
        unpacked = tmp / "x"
        run("dpkg-deb", "-x", str(deb), str(unpacked))
        for member in members:
            for f in sorted((unpacked / member).rglob("*") if member.endswith("/") else [unpacked / member]):
                if f.is_file():
                    out = dest / f.relative_to(unpacked)
                    out.parent.mkdir(parents=True, exist_ok=True)
                    out.write_bytes(f.read_bytes())
        (dest / "SOURCE").write_text(f"{deb.name} sha256 {sha256(deb)}\n", "utf-8")
        run("rm", "-rf", str(tmp))
    for src, match, prefix in FETCH_SOURCE:
        version = installed(match)
        dest = cache / f"{src}-src_{version}"
        if dest.is_dir():
            continue
        tmp = cache / "tmp"
        tmp.mkdir(exist_ok=True)
        run("apt-get", "source", "--download-only", f"{src}={version}", cwd=tmp)
        orig = next(tmp.glob(f"{src}_*.orig.tar.*"))
        with tarfile.open(orig) as tar:
            for m in sorted(tar.getmembers(), key=lambda m: m.name):
                rel = m.name.split("/", 1)[1] if "/" in m.name else ""
                if m.isfile() and rel.startswith(prefix) and rel.endswith(".xml"):
                    out = dest / rel
                    out.parent.mkdir(parents=True, exist_ok=True)
                    out.write_bytes(tar.extractfile(m).read())
        (dest / "SOURCE").write_text(f"{orig.name} sha256 {sha256(orig)}\n", "utf-8")
        run("rm", "-rf", str(tmp))


def inputs(cache: Path) -> list[tuple[str, Path, str, str]]:
    """(kind, path, source label, package=version) for every input file, in a fixed order."""
    out: list[tuple[str, Path, str, str]] = []
    fetched = {pkg: cache / f"{pkg}_{installed(match)}" for pkg, match, _ in FETCH}
    for name in GIR:
        path = next((Path(d) / f"{name}.gir" for d in GIR_DIRS if (Path(d) / f"{name}.gir").is_file()), None)
        if path is not None:
            out.append(("gir", path, str(path), owner(str(path))))
        else:
            hit = fetched["libwnck-3-dev"] / "usr/share/gir-1.0" / f"{name}.gir"
            if not hit.is_file():
                raise FileNotFoundError(f"{name}.gir: not installed and not fetched (run fetch)")
            ver = installed("libwnck-3-0")
            out.append(("gir", hit, f"libwnck-3-dev={ver}:/usr/share/gir-1.0/{name}.gir", f"libwnck-3-dev={ver}"))
    for h in HEADERS:
        path = Path("/usr/include") / h
        out.append(("header", path, str(path), owner(str(path))))
    seen = set()
    portal = fetched["xdg-desktop-portal-dev"] / "usr/share/dbus-1/interfaces"
    pver = installed("xdg-desktop-portal")
    for path in sorted(portal.glob("*.xml")):
        seen.add(path.name)
        out.append(("dbus", path, f"xdg-desktop-portal-dev={pver}:/usr/share/dbus-1/interfaces/{path.name}",
                    f"xdg-desktop-portal-dev={pver}"))
    for path in sorted(Path("/usr/share/dbus-1/interfaces").glob("*.xml")):
        if path.name not in seen:
            out.append(("dbus", path, str(path), owner(str(path))))
    for src, match, _ in FETCH_SOURCE:
        ver = installed(match)
        for path in sorted((cache / f"{src}-src_{ver}").rglob("*.xml")):
            rel = path.relative_to(cache / f"{src}-src_{ver}")
            out.append(("dbus", path, f"{src}-source={ver}:{rel}", f"{src}-source={ver}"))
    for path in [Path("/usr/share/wayland/wayland.xml"), *sorted(Path("/usr/share/wayland-protocols").rglob("*.xml"))]:
        out.append(("wayland", path, str(path), owner(str(path))))
    for pkg in PY_PACKAGES:
        root = Path(PY_DIR) / pkg
        pkgver = owner(str(root))
        for path in sorted(root.rglob("*.py")):
            out.append(("python", path, str(path), pkgver))
    return out


def build(cache: Path, out_root: Path) -> Path:
    import notes_lint  # noqa: PLC0415 - next to this script

    problems = notes_lint.lint(NOTES.read_text("utf-8"))
    if problems:
        raise SystemExit("notes.md fails notes_lint: " + "; ".join(problems))
    files = inputs(cache)
    entries: dict[str, dict] = {}
    for kind, path, source, _ in files:
        if kind == "gir":
            items: Iterable[dict] = parse_gir(path, source)
        elif kind == "header":
            items = parse_header(path, source, "libxdo" if path.name == "xdo.h" else "X11")
        elif kind == "dbus":
            items = parse_dbus(path, source)
        elif kind == "wayland":
            items = parse_wayland(path, source)
        else:
            rel = path.relative_to(PY_DIR).with_suffix("")
            module = ".".join(rel.parts[:-1] if rel.name == "__init__" else rel.parts)
            items = parse_python(path, module, source)
        for e in items:
            entries.setdefault(e["id"], e)  # first source wins on a duplicate id (fixed input order)
    for e in parse_notes(NOTES):
        entries[e["id"]] = e
    os_release = dict(
        line.split("=", 1) for line in Path("/etc/os-release").read_text().splitlines() if "=" in line)
    distro = f"{os_release['ID']}-{os_release['VERSION_ID'].strip(chr(34))}"
    packages = sorted({p for *_, p in files} | {f"python3={installed('python3')}"})
    key = hashlib.sha256("\n".join(packages).encode()).hexdigest()[:12]
    dest = out_root / f"{distro}-{key}"
    dest.mkdir(parents=True, exist_ok=True)
    lines = [json.dumps(entries[i], ensure_ascii=False, sort_keys=True) for i in sorted(entries)]
    (dest / "index.jsonl").write_text("\n".join(lines) + "\n", "utf-8")
    kinds: dict[str, int] = {}
    for e in entries.values():
        kinds[e["kind"]] = kinds.get(e["kind"], 0) + 1
    manifest = {
        "distro": distro,
        "packages": packages,
        "inputs": [{"source": s, "sha256": sha256(p)} for _, p, s, _ in files]
        + [{"source": "notes.md", "sha256": sha256(NOTES)}],
        "generator_sha256": sha256(Path(__file__)),
        "entries": len(entries),
        "kinds": dict(sorted(kinds.items())),
    }
    (dest / "manifest.json").write_text(json.dumps(manifest, indent=1, sort_keys=True) + "\n", "utf-8")
    return dest


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("command", choices=("fetch", "build"))
    ap.add_argument("--cache", type=Path, required=True)
    ap.add_argument("--out", type=Path)
    args = ap.parse_args(argv)
    if args.command == "fetch":
        fetch(args.cache)
        return 0
    if args.out is None:
        ap.error("build needs --out")
    print(build(args.cache, args.out))
    return 0


if __name__ == "__main__":
    sys.path.insert(0, str(HERE))
    sys.exit(main())
