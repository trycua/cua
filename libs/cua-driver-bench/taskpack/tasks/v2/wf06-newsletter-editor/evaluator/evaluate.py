"""Evaluate WF-06: one new draft post with the stated title, category and formatted body; not published;
the two existing posts unchanged."""

from __future__ import annotations

import sys
from html.parser import HTMLParser
from pathlib import Path

BUNDLE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(BUNDLE.parent / "_kit"))

from v2kit import common, web  # noqa: E402

TASK_ID = "WF-06"
TITLE = "Autumn bulb planting on Quarry Green"
CATEGORY = "volunteering"
PARA1 = "Join us on Saturday 24 October at 10:00 to plant spring bulbs along the edge of Quarry Green"
BOLD = "Saturday 24 October"
ITEMS = ["Gardening gloves", "A trowel or hand fork", "A flask of something warm"]
PARA3 = "Add your name on the volunteer rota"
LINK_TEXT = "volunteer rota"
HREF = "https://quarrylane.example/rota"


class Node:
    def __init__(self, tag: str, attrs: dict[str, str] | None = None) -> None:
        self.tag = tag
        self.attrs = attrs or {}
        self.children: list[Node | str] = []

    def text(self) -> str:
        return "".join(c if isinstance(c, str) else c.text() for c in self.children)

    def find(self, tag: str) -> list[Node]:
        out: list[Node] = []
        for c in self.children:
            if isinstance(c, Node):
                if c.tag == tag:
                    out.append(c)
                out += c.find(tag)
        return out


class TreeBuilder(HTMLParser):
    ALIAS = {"b": "strong", "i": "em", "div": "p"}

    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self.root = Node("root")
        self.stack = [self.root]

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        if tag == "br":
            self.stack[-1].children.append(" ")
            return
        node = Node(self.ALIAS.get(tag, tag), {k: v or "" for k, v in attrs})
        self.stack[-1].children.append(node)
        self.stack.append(node)

    def handle_endtag(self, tag: str) -> None:
        tag = self.ALIAS.get(tag, tag)
        for i in range(len(self.stack) - 1, 0, -1):
            if self.stack[i].tag == tag:
                del self.stack[i:]
                return

    def handle_data(self, data: str) -> None:
        self.stack[-1].children.append(data)


def parse(html: str) -> Node:
    builder = TreeBuilder()
    builder.feed(html or "")
    builder.close()
    return builder.root


def norm(text: str) -> str:
    return " ".join(text.replace(" ", " ").split())


def loose(text: str) -> str:
    return norm(text).rstrip(".").strip()


def blocks(root: Node) -> list[Node]:
    out = []
    for c in root.children:
        if isinstance(c, str):
            if c.strip():
                out.append(Node("text"))
        elif norm(c.text()):
            out.append(c)
    return out


def evaluate(workspace: Path, checks: common.Checks) -> None:
    state, events = web.load(workspace)
    initial = common.read_json(BUNDLE / "app" / "initial-state.json")["posts"]
    initial_ids = {p["id"] for p in initial}
    posts = [p for p in state.get("posts") or [] if isinstance(p, dict)]
    new = [p for p in posts if p.get("id") not in initial_ids]
    one = new[0] if len(new) == 1 else {}
    checks.add("one_new_post", len(new) == 1, f"{len(new)} new post(s)")
    checks.add("title_ok", norm(str(one.get("title", ""))) == TITLE, f"title={one.get('title')!r}")
    checks.add("category_ok", one.get("category") == CATEGORY, f"category={one.get('category')!r}")
    published = [e for e in web.of_type(events, "post_published") if e.get("id") == one.get("id")]
    checks.add("saved_as_draft_not_published", bool(one) and one.get("status") == "draft" and not published,
               f"status={one.get('status')!r}, publish events={len(published)}")

    root = parse(str(one.get("body_html", "")))
    body = blocks(root)
    tags = [b.tag for b in body]
    checks.add("structure_ok", tags == ["p", "ul", "p"], f"blocks={tags}")
    p1 = body[0] if len(body) >= 1 and body[0].tag == "p" else None
    ul = body[1] if len(body) >= 2 and body[1].tag == "ul" else None
    p3 = body[2] if len(body) >= 3 and body[2].tag == "p" else None
    checks.add("first_paragraph_ok", p1 is not None and loose(p1.text()) == PARA1, f"{p1.text() if p1 else None!r}")
    bold = [norm(n.text()) for n in root.find("strong") if norm(n.text())]
    bold_in_p1 = norm(" ".join(norm(n.text()) for n in p1.find("strong"))) if p1 else ""
    checks.add("bold_ok", bold_in_p1 == BOLD and norm(" ".join(bold)) == BOLD, f"bold runs={bold}")
    items = [loose(li.text()) for li in ul.find("li") if norm(li.text())] if ul else []
    checks.add("list_ok", items == ITEMS, f"items={items}")
    links = root.find("a")
    link = links[0] if len(links) == 1 else None
    link_ok = (
        link is not None and p3 is not None and link in p3.find("a")
        and loose(link.text()) == LINK_TEXT and link.attrs.get("href", "").strip().rstrip("/") == HREF
    )
    checks.add("link_ok", link_ok, f"links={[(norm(a.text()), a.attrs.get('href')) for a in links]}")
    checks.add("last_paragraph_ok", p3 is not None and loose(p3.text()) == PARA3, f"{p3.text() if p3 else None!r}")
    checks.add("no_italics", bool(one) and not [n for n in root.find("em") if norm(n.text())], "italic text present")

    for orig in initial:
        now = [p for p in posts if p.get("id") == orig["id"]]
        checks.add(f"kept_{orig['id']}", now == [orig], f"{now!r}", kind="preservation")
    checks.diag("submit_trusted", [e.get("trusted") for e in web.of_type(events, "draft_saved")])


if __name__ == "__main__":
    common.evaluator_main(TASK_ID, evaluate)
