#!/usr/bin/env python3
"""Docs-as-tests: every fenced code block in the docs declares what it is.

Each fence carries exactly one disposition in its meta string (fumadocs and
rehype-pretty-code ignore unknown attributes)::

    ```python test="docs" id="images-base"        # runs in lane `docs`; stable id
    ```python test="docs,fleet" id="omarchy"      # runs in several lanes
    ```python test="docs" session="images"        # cumulative with earlier blocks
    ```python test="docs" prelude="fakefleet"     # hidden harness setup
    ```text output                                # program output / not code
    ```bash skip="host-install"                   # reason from the allowlist

Lanes and skip reasons live in ``docs/code-block-policy.json``. Blocks on
generated pages (``AUTO-GENERATED`` banner) or inside ``GENERATED:<name>``
regions are exempt: their generator's ``--check`` owns them.

Ratchet: blocks that predate the rule are listed by fingerprint (page + hash
of lang and code) in ``docs/code-block-baseline.json``. A new or edited block
must be decided; the baseline may only shrink (``--baseline-base``).

Usage::

    extract.py [--docs DIR] [--json]        # list test= blocks (DIR defaults to
                                            # $CUA_E2E_DOCS_ROOT or docs/content/docs)
    extract.py --write OUT_DIR              # write each test= block to a file
    extract.py --lint [--baseline-base F]   # enforce dispositions (CI)
    extract.py --update-baseline            # drop decided/removed baseline entries
    extract.py --annotate                   # mechanical dispositions (see annotate())
    extract.py --inventory                  # every fence as JSON (coverage join)
    extract.py --sync-excerpts              # copy source regions into test="excerpt" blocks

Stdlib only; imported by ../python/test_docs_blocks.py.
"""

from __future__ import annotations

import argparse
import datetime as _dt
import hashlib
import json
import os
import re
import sys
import textwrap
from dataclasses import asdict, dataclass, field
from pathlib import Path

REPO = Path(__file__).resolve().parents[4]
# The docs root is configurable: the published site may be built from another
# repository's content tree. CUA_E2E_DOCS_ROOT re-points extraction there.
DOCS = Path(os.environ.get("CUA_E2E_DOCS_ROOT") or REPO / "docs" / "content" / "docs")
POLICY = REPO / "docs" / "code-block-policy.json"
BASELINE = REPO / "docs" / "code-block-baseline.json"

# Guides with self-contained runnable blocks; `--strict` (and the suite's
# test_guides_are_tagged) fails if one loses its tags. The Omarchy noVNC and
# Minecraft flows only have fragments or in-guest shell steps; their behaviour
# is covered by the SDK scenario tests instead. The Fleets quickstart downloads
# its example from docs/public/scripts (a pinned, published cua-sandbox).
GUIDES = [
    "fleets/guides/terraform.mdx",
    "fleets/guides/capacity-and-claims.mdx",
    "cua-sdk/guides/agent-frameworks.mdx",
    "cua-sdk/guides/images.mdx",
    "cua-sdk/reference/sandbox/index.mdx",
]

FENCE = re.compile(r"^(?P<indent>[ \t]*)(?P<fence>```+|~~~+)(?P<info>.*)$")
ATTR = re.compile(r'([A-Za-z][\w-]*)="([^"]*)"')
BLOCK_ID = re.compile(r"^[a-z0-9][a-z0-9-]*$")
GENERATED_PAGE = "AUTO-GENERATED"
REGION_START = re.compile(r"\{/\*\s*GENERATED:([\w-]+):start\s*\*/\}")
REGION_END = re.compile(r"\{/\*\s*GENERATED:([\w-]+):end\s*\*/\}")
PLACEHOLDER = re.compile(r"<[^<>\n]+>|\bYOUR_[A-Z0-9_]+|\.\.\.|…")
LANG_ALIASES = {
    "sh": "bash",
    "shell": "bash",
    "zsh": "bash",
    "ts": "typescript",
    "tsx": "typescript",
    "js": "javascript",
    "py": "python",
    "yml": "yaml",
    "jsonc": "json",
    "kt": "kotlin",
    "terraform": "hcl",
    "ps1": "powershell",
    "pwsh": "powershell",
}


def norm_lang(lang: str) -> str:
    lang = lang.split("{")[0].lower()
    return LANG_ALIASES.get(lang, lang)


@dataclass
class Fence:
    """One fenced code block, whatever its disposition."""

    page: str  # path relative to the docs root
    index: int  # 0-based among the page's fences
    line: int  # 1-based line of the opening fence
    indent: str
    lang: str  # as written (first word of the info string)
    info: str  # the full info string
    attrs: dict[str, str]
    flags: set[str]  # bare words after the language (e.g. `output`)
    code: str
    generated: bool  # generated page or GENERATED region

    @property
    def disposition(self) -> str | None:
        kinds = [k for k in ("test", "skip") if k in self.attrs]
        if "output" in self.flags:
            kinds.append("output")
        return kinds[0] if len(kinds) == 1 else ("conflict" if kinds else None)

    @property
    def fingerprint(self) -> str:
        digest = hashlib.sha256(f"{norm_lang(self.lang)}\n{self.code}".encode()).hexdigest()
        return digest[:16]

    @property
    def block_id(self) -> str:
        return self.attrs.get("id", "")

    @property
    def qualified_id(self) -> str:
        """``<page without .mdx>#<id>``: stable across edits above the block."""
        return f"{self.page.removesuffix('.mdx')}#{self.block_id or self.index}"


@dataclass
class Block:
    """A block tagged ``test="<lane>[,<lane>...]"`` (the runnable ones)."""

    guide: str
    index: int  # 0-based among the guide's fenced blocks (ordering only)
    line: int  # 1-based line of the opening fence
    lang: str
    title: str
    lane: str  # the raw test= value
    code: str
    block_id: str = ""
    session: str = ""
    prelude: str = ""
    env: str = ""
    attrs: dict[str, str] = field(default_factory=dict)

    @property
    def lanes(self) -> list[str]:
        return [x.strip() for x in self.lane.split(",") if x.strip()]

    @property
    def id(self) -> str:
        return f"{self.guide.removesuffix('.mdx')}#{self.block_id or self.index}"


def fences(path: Path, rel: str) -> list[Fence]:
    text = path.read_text(encoding="utf-8")
    page_generated = GENERATED_PAGE in text[:600]
    lines = text.splitlines()
    out: list[Fence] = []
    i, index, region = 0, 0, None
    while i < len(lines):
        start_m, end_m = REGION_START.search(lines[i]), REGION_END.search(lines[i])
        if start_m:
            region = start_m[1]
        if end_m and end_m[1] == region:
            region = None
        m = FENCE.match(lines[i])
        if not m:
            i += 1
            continue
        fence, info, indent = m["fence"], m["info"].strip(), m["indent"]
        start = i
        body = []
        i += 1
        while i < len(lines) and not lines[i].strip().startswith(fence):
            body.append(lines[i][len(indent) :] if lines[i].startswith(indent) else lines[i])
            i += 1
        i += 1  # closing fence
        lang = info.split()[0] if info and "=" not in info.split()[0] else ""
        rest = ATTR.sub("", info[len(lang) :]) if info else ""
        out.append(
            Fence(
                page=rel,
                index=index,
                line=start + 1,
                indent=indent,
                lang=lang,
                info=info,
                attrs=dict(ATTR.findall(info)),
                flags=set(rest.split()),
                code="\n".join(body) + "\n",
                generated=page_generated or region is not None,
            )
        )
        index += 1
    return out


def all_fences(docs: Path = DOCS) -> list[Fence]:
    found: list[Fence] = []
    for path in sorted(docs.rglob("*.mdx")):
        found.extend(fences(path, path.relative_to(docs).as_posix()))
    return found


def blocks(path: Path, rel: str) -> list[Block]:
    return [_block(f) for f in fences(path, rel) if "test" in f.attrs]


def _block(f: Fence) -> Block:
    return Block(
        guide=f.page,
        index=f.index,
        line=f.line,
        lang=f.lang,
        title=f.attrs.get("title", ""),
        lane=f.attrs["test"],
        code=f.code,
        block_id=f.block_id,
        session=f.attrs.get("session", ""),
        prelude=f.attrs.get("prelude", ""),
        env=f.attrs.get("env", ""),
        attrs=f.attrs,
    )


def all_blocks(docs: Path = DOCS) -> list[Block]:
    return [_block(f) for f in all_fences(docs) if "test" in f.attrs]


# ---------------------------------------------------------------- policy


def load_policy(path: Path = POLICY) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


def load_baseline(path: Path = BASELINE) -> set[tuple[str, str]]:
    if not path.exists():
        return set()
    data = json.loads(path.read_text(encoding="utf-8"))
    return {(b["page"], b["fingerprint"]) for b in data["blocks"]}


def write_baseline(entries: list[Fence], path: Path = BASELINE) -> None:
    rows = sorted(
        {(f.page, norm_lang(f.lang) or "(none)", f.fingerprint) for f in entries},
        key=lambda r: (r[0], r[2]),
    )
    data = {
        "$comment": (
            "Docs code blocks that predate the disposition rule (see "
            "tests/e2e/cua-sdk/docs/extract.py). This list may only shrink: decide "
            "a block (test=, skip= or output), then run extract.py --update-baseline."
        ),
        "count": len(rows),
        "blocks": [{"page": p, "lang": lang, "fingerprint": fp} for p, lang, fp in rows],
    }
    path.write_text(json.dumps(data, indent=1) + "\n", encoding="utf-8")


def lint(
    found: list[Fence],
    policy: dict,
    baseline: set[tuple[str, str]],
    today: _dt.date | None = None,
    repo: Path = REPO,
) -> tuple[list[str], list[Fence]]:
    """Returns (problems, undecided-but-baselined fences)."""
    today = today or _dt.date.today()
    lanes = set(policy["lanes"])
    reasons = policy["skip"]
    problems: list[str] = []
    pending: list[Fence] = []
    ids: dict[tuple[str, str], int] = {}
    for f in found:
        where = f"{f.page}:{f.line}"
        if f.generated:
            continue
        d = f.disposition
        if d == "conflict":
            problems.append(f"{where}: more than one disposition (test=, skip=, output): {f.info}")
            continue
        if d is None:
            if (f.page, f.fingerprint) in baseline:
                pending.append(f)
            else:
                problems.append(
                    f"{where}: [{f.lang or 'no lang'}] block has no disposition; add "
                    'test="<lane>" id="<id>", skip="<reason>" or output'
                )
            continue
        if d == "test":
            bad = [x for x in f.attrs["test"].split(",") if x.strip() not in lanes]
            if bad or not f.attrs["test"].strip():
                problems.append(f"{where}: unknown lane(s) {bad or ['(empty)']}; see {POLICY.name}")
            if not f.block_id:
                problems.append(f'{where}: test= block needs a stable id="<id>"')
            elif not BLOCK_ID.match(f.block_id):
                problems.append(f"{where}: id {f.block_id!r} must match {BLOCK_ID.pattern}")
            else:
                key = (f.page, f.block_id)
                if key in ids:
                    problems.append(f"{where}: duplicate id {f.block_id!r} (line {ids[key]})")
                ids[key] = f.line
        elif d == "skip":
            reason = f.attrs["skip"]
            spec = reasons.get(reason)
            if spec is None:
                problems.append(f"{where}: skip reason {reason!r} is not in {POLICY.name}")
                continue
            expires = spec.get("expires")
            if expires and _dt.date.fromisoformat(expires) < today:
                problems.append(f"{where}: skip reason {reason!r} expired on {expires}")
            if spec.get("requires_placeholder") and not PLACEHOLDER.search(f.code):
                problems.append(
                    f'{where}: skip="{reason}" needs a visible placeholder such as <name> or ...'
                )
    for f in found:
        of = f.attrs.get("output-of")
        if of and (f.page, of) not in ids:
            problems.append(
                f"{f.page}:{f.line}: output-of={of!r} names no test= block on this page"
            )
    for f in found:
        if not f.generated and f.disposition == "test" and "excerpt" in f.attrs["test"].split(","):
            problems += excerpt_problems(f, repo)
    undecided = {(f.page, f.fingerprint) for f in pending}
    stale = sorted(baseline - undecided)
    if stale:
        problems.append(
            f"{len(stale)} baseline entr{'y is' if len(stale) == 1 else 'ies are'} no longer "
            "undecided blocks (decided, edited or removed); run extract.py --update-baseline. "
            f"First: {stale[0][0]} {stale[0][1]}"
        )
    return problems, pending


def carry_baseline(undecided: list[Fence], baseline: set[tuple[str, str]]) -> list[Fence]:
    """The undecided blocks that stay baselined: those still listed, plus a
    block whose unchanged code moved to another page (its old entry is gone),
    so moving a page does not force deciding every block on it."""
    keep = [f for f in undecided if (f.page, f.fingerprint) in baseline]
    live = {(f.page, f.fingerprint) for f in undecided}
    moved: dict[str, int] = {}
    for page, fp in baseline - live:
        moved[fp] = moved.get(fp, 0) + 1
    for f in undecided:
        if (f.page, f.fingerprint) not in baseline and moved.get(f.fingerprint, 0) > 0:
            moved[f.fingerprint] -= 1
            keep.append(f)
    return keep


REGION = re.compile(r"^\s*(?://|#)\s*#?(end)?region docs:([a-z0-9][a-z0-9-]*)\s*$")


def region(path: Path, region_id: str) -> str | None:
    """The lines between ``// #region docs:<id>`` and ``// #endregion docs:<id>``
    (``#`` comments too), dedented; None when the file or region is missing."""
    if not path.is_file():
        return None
    body: list[str] | None = None
    for line in path.read_text(encoding="utf-8").splitlines():
        m = REGION.match(line)
        if m and m[2] == region_id:
            if m[1] and body is not None:
                return textwrap.dedent("\n".join(body) + "\n")
            if not m[1]:
                body = []
            continue
        if body is not None:
            body.append(line)
    return None


def excerpt_problems(f: Fence, repo: Path = REPO) -> list[str]:
    """A test="excerpt" block must be byte-identical (after dedent) to the
    region ``source="<repo path>#<region id>"`` of a file CI builds."""
    where = f"{f.page}:{f.line}"
    source = f.attrs.get("source", "")
    rel, _, region_id = source.partition("#")
    if not rel or not region_id:
        return [f'{where}: test="excerpt" needs source="<repo path>#<region id>"']
    code = region(repo / rel, region_id)
    if code is None:
        return [f"{where}: no region docs:{region_id} in {rel}"]
    if code.strip("\n") != textwrap.dedent(f.code).strip("\n"):
        return [f"{where}: excerpt differs from {source}; copy the region into the page"]
    return []


def sync_excerpts(docs: Path, repo: Path = REPO) -> int:
    """Rewrites each test="excerpt" block with its source region (the page
    follows the sample CI builds). Returns the number of blocks changed."""
    changed = 0
    for path in sorted(docs.rglob("*.mdx")):
        rel = path.relative_to(docs).as_posix()
        todo = []
        for f in fences(path, rel):
            if f.disposition != "test" or "excerpt" not in f.attrs["test"].split(","):
                continue
            file, _, rid = f.attrs.get("source", "").partition("#")
            code = region(repo / file, rid) if file and rid else None
            if code is not None and code.strip("\n") != textwrap.dedent(f.code).strip("\n"):
                todo.append((f, code))
        if not todo:
            continue
        lines = path.read_text(encoding="utf-8").split("\n")
        for f, code in sorted(todo, key=lambda t: -t[0].line):
            start = f.line  # index of the first body line
            end = start + len(f.code.rstrip("\n").split("\n")) if f.code.strip("\n") else start
            body = [(f.indent + ln) if ln else "" for ln in code.rstrip("\n").split("\n")]
            lines[start:end] = body
            changed += 1
        path.write_text("\n".join(lines), encoding="utf-8")
    return changed


def shrink_problems(current: set[tuple[str, str]], base: set[tuple[str, str]]) -> list[str]:
    """The baseline may only shrink. Blocks may move between pages with their
    code unchanged, so this compares code fingerprints, not pages."""
    base_fps = {fp for _, fp in base}
    grown = sorted((page, fp) for page, fp in current if fp not in base_fps)
    problems = [f"baseline grew: {page} {fp} is not in the base baseline" for page, fp in grown]
    if len(current) > len(base):
        problems.append(f"baseline grew from {len(base)} to {len(current)} entries")
    return problems


# ---------------------------------------------------------------- annotate

INSTALL = re.compile(
    r"^\s*(\$\s*)?(sudo\s+)?("
    r"(pip3?|uv pip|uv tool|pipx|python3? -m pip) install\b|uv add\b|"
    r"(npm|pnpm|yarn|bun) (install|i|add|ci)\b|npm\.cmd (install|ci)\b|"
    r"(brew|cargo|go|winget|choco|scoop|apt|apt-get|dnf|yum|pacman) (install|-S)\b|"
    r"curl\b.*\|\s*(sudo\s+)?(ba|z)?sh\b|(irm|iwr|Invoke-WebRequest)\b.*\|\s*iex\b"
    r")"
)
OUTPUT_LANGS = {"", "text", "txt", "console", "plaintext", "output", "log"}


def mechanical(f: Fence) -> str | None:
    """The disposition a block gets without a human decision, if any:

    * ``text``/``console``/no-language blocks are output or illustrations;
    * ``mermaid`` blocks are diagrams;
    * a shell block of at most four commands that all install something
      (package managers, ``curl | sh`` installers) is a host install.
    """
    lang = norm_lang(f.lang)
    if lang in OUTPUT_LANGS:
        return "output"
    if lang == "mermaid":
        return 'skip="diagram"'
    if lang in ("bash", "powershell"):
        code = f.code.strip()
        cmds = [ln for ln in code.splitlines() if ln.strip() and not ln.lstrip().startswith("#")]
        if cmds and len(cmds) <= 4 and all(INSTALL.match(ln) for ln in cmds):
            return 'skip="host-install"'
    return None


def annotate(docs: Path, only: set[tuple[str, str]] | None = None) -> int:
    """Adds mechanical dispositions to undecided blocks in place. Returns the count."""
    changed = 0
    for path in sorted(docs.rglob("*.mdx")):
        rel = path.relative_to(docs).as_posix()
        todo = {
            f.line: mechanical(f)
            for f in fences(path, rel)
            if not f.generated
            and f.disposition is None
            and (only is None or (f.page, f.fingerprint) in only)
            and mechanical(f)
        }
        if not todo:
            continue
        lines = path.read_text(encoding="utf-8").split("\n")
        for line_no, disp in todo.items():
            m = FENCE.match(lines[line_no - 1])
            assert m, (rel, line_no)
            info = m["info"].strip()
            if not info or "=" in info.split()[0]:
                info = f"text {info}".strip()
            lines[line_no - 1] = f"{m['indent']}{m['fence']}{info} {disp}"
            changed += 1
        path.write_text("\n".join(lines), encoding="utf-8")
    return changed


# ---------------------------------------------------------------- main


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--docs", type=Path, default=DOCS)
    ap.add_argument("--json", action="store_true")
    ap.add_argument("--write", type=Path)
    ap.add_argument(
        "--strict", action="store_true", help="fail if a covered guide has no tagged block"
    )
    ap.add_argument("--lint", action="store_true", help="enforce block dispositions")
    ap.add_argument("--policy", type=Path, default=POLICY)
    ap.add_argument("--baseline", type=Path, default=BASELINE)
    ap.add_argument(
        "--baseline-base",
        type=Path,
        help="with --lint: the base branch's baseline; fail if the baseline grew",
    )
    ap.add_argument("--update-baseline", action="store_true")
    ap.add_argument(
        "--init-baseline",
        action="store_true",
        help="record every undecided block (one-off, when adopting the rule)",
    )
    ap.add_argument("--annotate", action="store_true", help="apply mechanical dispositions")
    ap.add_argument("--inventory", action="store_true", help="every fence as JSON")
    ap.add_argument(
        "--sync-excerpts", action="store_true", help="copy source regions into excerpt blocks"
    )
    a = ap.parse_args()

    if a.sync_excerpts:
        print(f"synced {sync_excerpts(a.docs)} excerpt(s)")
        return 0
    if a.annotate:
        n = annotate(a.docs, load_baseline(a.baseline) or None)
        print(f"annotated {n} block(s)")
        return 0
    if a.init_baseline or a.update_baseline:
        found = all_fences(a.docs)
        undecided = [f for f in found if not f.generated and f.disposition is None]
        if a.update_baseline:
            undecided = carry_baseline(undecided, load_baseline(a.baseline))
        write_baseline(undecided, a.baseline)
        print(f"baseline: {len({(f.page, f.fingerprint) for f in undecided})} undecided block(s)")
        return 0
    if a.inventory:
        rows = [
            {
                "page": f.page,
                "line": f.line,
                "index": f.index,
                "lang": f.lang,
                "id": f.qualified_id,
                "disposition": "generated" if f.generated else (f.disposition or "baseline"),
                "lanes": [x for x in f.attrs.get("test", "").split(",") if x],
                "skip": f.attrs.get("skip", ""),
            }
            for f in all_fences(a.docs)
        ]
        print(json.dumps(rows, indent=1))
        return 0
    if a.lint:
        found = all_fences(a.docs)
        baseline = load_baseline(a.baseline)
        problems, pending = lint(found, load_policy(a.policy), baseline)
        if a.baseline_base:
            problems += shrink_problems(baseline, load_baseline(a.baseline_base))
        counts: dict[str, int] = {}
        for f in found:
            k = "generated" if f.generated else (f.disposition or "baseline")
            counts[k] = counts.get(k, 0) + 1
        summary = ", ".join(f"{k} {v}" for k, v in sorted(counts.items()))
        if problems:
            print("docs code blocks:", *problems, sep="\n  ", file=sys.stderr)
            print(f"\n{len(problems)} problem(s); blocks: {summary}", file=sys.stderr)
            return 1
        print(f"docs code blocks OK: {summary}")
        return 0

    found = all_blocks(a.docs)
    if a.strict:
        tagged = {b.guide for b in found}
        missing = [g for g in GUIDES if g not in tagged]
        if missing:
            print("guides without a tagged block:", *missing, sep="\n  ", file=sys.stderr)
            return 1
    if a.write:
        a.write.mkdir(parents=True, exist_ok=True)
        for b in found:
            ext = {
                "python": "py",
                "ts": "ts",
                "typescript": "ts",
                "bash": "sh",
                "hcl": "tf",
                "terraform": "tf",
            }.get(b.lang, "txt")
            (a.write / f"{b.id.replace('#', '_').replace('/', '_')}.{ext}").write_text(
                b.code, encoding="utf-8"
            )
    if a.json:
        print(json.dumps([asdict(b) | {"id": b.id, "lanes": b.lanes} for b in found], indent=2))
    elif not a.write:
        for b in found:
            print(f"{b.guide}:{b.line}  [{b.lang}] lane={b.lane}  {b.id}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
