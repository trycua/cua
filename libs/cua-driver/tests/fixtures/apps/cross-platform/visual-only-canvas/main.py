#!/usr/bin/env python3
"""Custom-painted visual fixture with a loopback-only behavioral oracle."""

from __future__ import annotations

import argparse
import json
import tkinter as tk
from typing import Optional
import urllib.parse
import urllib.request


WIDTH = 760
HEIGHT = 460
CARDS = (
    {"id": "ember", "label": "EMBER", "bounds": (72, 132, 276, 310), "color": "#e85d3f"},
    {"id": "tide", "label": "TIDE", "bounds": (292, 132, 496, 310), "color": "#1e8b99"},
    {"id": "moss", "label": "MOSS", "bounds": (512, 132, 716, 310), "color": "#6f8f3d"},
)


def card_at(x: int, y: int) -> Optional[str]:
    for card in CARDS:
        left, top, right, bottom = card["bounds"]
        if left <= x <= right and top <= y <= bottom:
            return str(card["id"])
    return None


def oracle_state(selected: Optional[str], action_count: int) -> dict[str, object]:
    return {
        "fixture": "visual-only-canvas/v1",
        "ready": True,
        "selected": selected,
        "action_count": action_count,
    }


def validate_journal_url(url: str) -> None:
    parsed = urllib.parse.urlparse(url)
    if parsed.scheme != "http" or parsed.hostname not in {"127.0.0.1", "::1", "localhost"}:
        raise ValueError("fixture journal must be an HTTP loopback endpoint")


def post_oracle(url: str, state: dict[str, object]) -> None:
    validate_journal_url(url)
    payload = json.dumps(state, separators=(",", ":")).encode("utf-8")
    request = urllib.request.Request(
        url,
        data=payload,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(request, timeout=1) as response:
        if response.status != 204:
            raise RuntimeError(f"fixture journal returned HTTP {response.status}")


class VisualFixture:
    def __init__(self, journal_url: str) -> None:
        self.journal_url = journal_url
        self.selected: Optional[str] = None
        self.action_count = 0
        self.root = tk.Tk()
        self.root.title("Cua Visual-Only Canvas Fixture")
        self.root.geometry(f"{WIDTH}x{HEIGHT}")
        self.root.resizable(False, False)
        self.canvas = tk.Canvas(
            self.root,
            width=WIDTH,
            height=HEIGHT,
            background="#f3ead7",
            highlightthickness=0,
            takefocus=0,
        )
        self.canvas.pack(fill="both", expand=True)
        self.canvas.bind("<Button-1>", self.on_click)
        self.paint()
        self.root.after(50, self.publish)

    def paint(self) -> None:
        self.canvas.delete("all")
        self.canvas.create_rectangle(0, 0, WIDTH, 94, fill="#172b36", outline="")
        self.canvas.create_text(
            42, 34, anchor="nw", text="CHOOSE A SIGNAL", fill="#fff8e8", font=("Helvetica", 25, "bold")
        )
        self.canvas.create_text(
            44, 72, anchor="nw", text="A painted surface: pixels in, coordinates out", fill="#b8c9c8", font=("Helvetica", 11)
        )
        for card in CARDS:
            left, top, right, bottom = card["bounds"]
            selected = self.selected == card["id"]
            border = "#172b36" if selected else "#cfbea0"
            width = 7 if selected else 2
            self.canvas.create_rectangle(left, top, right, bottom, fill=card["color"], outline=border, width=width)
            self.canvas.create_oval(left + 61, top + 42, left + 143, top + 124, fill="#fff8e8", outline="")
            self.canvas.create_text(
                (left + right) // 2, bottom - 28, text=card["label"], fill="#fff8e8", font=("Helvetica", 18, "bold")
            )
        status = "WAITING FOR A VISUAL CHOICE" if self.selected is None else f"SELECTED: {self.selected.upper()}"
        self.canvas.create_text(380, 386, text=status, fill="#172b36", font=("Helvetica", 15, "bold"))

    def on_click(self, event: tk.Event) -> None:
        selected = card_at(int(event.x), int(event.y))
        if selected is None:
            return
        self.selected = selected
        self.action_count += 1
        self.paint()
        self.publish()

    def publish(self) -> None:
        try:
            post_oracle(self.journal_url, oracle_state(self.selected, self.action_count))
        except OSError:
            self.root.after(100, self.publish)

    def run(self) -> None:
        self.root.mainloop()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--journal-url")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        assert card_at(174, 220) == "ember"
        assert card_at(394, 220) == "tide"
        assert card_at(614, 220) == "moss"
        assert card_at(20, 20) is None
        assert oracle_state("tide", 2)["action_count"] == 2
        return 0
    if not args.journal_url:
        parser.error("--journal-url is required unless --self-test is used")
    validate_journal_url(args.journal_url)
    VisualFixture(args.journal_url).run()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
