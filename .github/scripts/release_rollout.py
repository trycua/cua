#!/usr/bin/env python3
"""Staged rollout of a Cua Spaces build: edit the updater feeds, nothing else.

Reads the feed files from --feed-dir (a copy of the assets on the rolling
`cua-spaces-latest` release), writes the changed ones to --out-dir and prints
a Markdown report with the diff. It never uploads; .github/workflows/release-rollout.yml
does that, unless it is a dry run.

electron-updater feeds (latest*.yml, beta*.yml for --channel beta): the top-level
`stagingPercentage` field. 10 and 50 set it, 100 removes it, pause sets 0 (no
install is inside 0%, so nobody is offered the version; installs that already
updated keep it). Pause and resume need no earlier feed file.

Sparkle appcast (macOS, cua-spaces-appcast.xml): the newest item of the channel
gets `sparkle:phasedRolloutInterval` and a `pubDate`. Sparkle has 7 groups, each
install picks one at random, and group k is offered the item once
now >= pubDate + k * interval. So the stages are:
  10   pubDate = now, interval = 10 years: group 0 only (1/7 = 14.3%), held
  50   pubDate = now - 3 weeks, interval = 1 week: groups 0-3 (4/7 = 57.1%); group 4 follows in a week
  100  the interval is removed: everyone
  pause  the item is removed (kept in cua-spaces-appcast-paused.xml); the next stage restores it
"""

from __future__ import annotations

import argparse
import difflib
import io
import re
import sys
import xml.etree.ElementTree as ET
from datetime import datetime, timedelta, timezone
from pathlib import Path

SP = "http://www.andymatuschak.org/xml-namespaces/sparkle"
ET.register_namespace("sparkle", SP)
APPCAST = "cua-spaces-appcast.xml"
PAUSED = "cua-spaces-appcast-paused.xml"
HOLD_SECONDS = 10 * 365 * 24 * 3600
WEEK = 7 * 24 * 3600
GROUPS = 7
STAGING = {"10": "10", "50": "50", "pause": "0"}

ELECTRON_FILES = {
    "windows": ["{c}.yml"],
    "linux": ["{c}-linux.yml", "{c}-linux-arm64.yml"],
    "macos": ["{c}-mac.yml"],  # Electron macOS only exists after the Sparkle cutover
}


class RolloutError(Exception):
    pass


def set_staging(text: str, value: str | None) -> str:
    """The feed with its top-level stagingPercentage set to value, or removed for None."""
    kept = [line for line in text.splitlines() if not re.match(r"stagingPercentage\s*:", line)]
    if value is not None:
        kept.append(f"stagingPercentage: {value}")
    return "\n".join(kept) + "\n"


def feed_version(text: str) -> str:
    m = re.search(r"^version:\s*['\"]?([^'\"\s]+)", text, re.M)
    if not m:
        raise RolloutError("feed has no version")
    return m.group(1)


def electron(feed_dir: Path, channel: str, platforms: list[str], stage: str) -> tuple[dict[str, str], list[str]]:
    changed: dict[str, str] = {}
    notes: list[str] = []
    prefix = "latest" if channel == "stable" else "beta"
    for platform in platforms:
        for pattern in ELECTRON_FILES[platform]:
            name = pattern.format(c=prefix)
            path = feed_dir / name
            if not path.exists():
                notes.append(f"`{name}`: not on the feed, skipped")
                continue
            before = path.read_text()
            version = feed_version(before)
            if channel == "stable" and "-" in version:
                raise RolloutError(f"{name} names {version}, a prerelease: the stable feed must not")
            changed[name] = set_staging(before, None if stage == "100" else STAGING[stage])
            notes.append(f"`{name}`: version {version}")
    return changed, notes


def _version_key(item: ET.Element) -> tuple[int, ...]:
    return tuple(int(p) for p in re.findall(r"\d+", item.findtext(f"{{{SP}}}version") or "0"))


def _channel_items(root: ET.Element, channel: str) -> list[ET.Element]:
    want = None if channel == "stable" else "beta"
    return [i for i in root.iter("item") if i.findtext(f"{{{SP}}}channel") == want]


def _write(tree: ET.ElementTree) -> str:
    ET.indent(tree, space="    ")
    buf = io.BytesIO()
    tree.write(buf, encoding="utf-8", xml_declaration=True)
    return buf.getvalue().decode()


def _fmt(dt: datetime) -> str:
    return dt.astimezone(timezone.utc).strftime("%a, %d %b %Y %H:%M:%S +0000")


def sparkle(feed_dir: Path, channel: str, stage: str, now: datetime) -> tuple[dict[str, str], list[str], list[str]]:
    path = feed_dir / APPCAST
    if not path.exists():
        return {}, [f"`{APPCAST}`: not on the feed, skipped"], []
    tree = ET.parse(path)
    root = tree.getroot()
    channel_el = root.find("channel")
    paused_path = feed_dir / PAUSED
    deletions: list[str] = []
    changed: dict[str, str] = {}
    notes: list[str] = []

    items = _channel_items(root, channel)
    if stage != "pause" and paused_path.exists():
        # Resume: put the paused item back before staging it.
        paused = _channel_items(ET.parse(paused_path).getroot(), channel)
        if paused and not any(i.findtext(f"{{{SP}}}version") == paused[0].findtext(f"{{{SP}}}version") for i in items):
            channel_el.insert(len([c for c in channel_el if c.tag != "item"]), paused[0])
            items = _channel_items(root, channel)
            notes.append(f"restored the paused item {paused[0].findtext('title')}")
        deletions.append(PAUSED)
    if not items:
        return {}, notes + [f"`{APPCAST}`: no {channel} item, skipped"], deletions
    item = max(items, key=_version_key)
    title = item.findtext("title")
    if channel == "stable" and title and "-" in title:
        raise RolloutError(f"newest stable appcast item is {title}, a prerelease")

    for el in item.findall(f"{{{SP}}}phasedRolloutInterval"):
        item.remove(el)

    if stage == "pause":
        channel_el.remove(item)
        keep = ET.ElementTree(ET.fromstring(f"<rss xmlns:sparkle='{SP}' version='2.0'><channel/></rss>"))
        keep.getroot().find("channel").append(item)
        changed[PAUSED] = _write(keep)
        notes.append(f"`{APPCAST}`: {title} removed, kept in `{PAUSED}`")
    elif stage == "100":
        notes.append(f"`{APPCAST}`: {title} phased rollout removed, every install is offered it")
    else:
        interval, pub_date = (HOLD_SECONDS, now) if stage == "10" else (WEEK, now - timedelta(seconds=3 * WEEK))
        pub = item.find("pubDate")
        if pub is None:
            pub = ET.Element("pubDate")
            item.insert(list(item).index(item.find("title")) + 1, pub)
        pub.text = _fmt(pub_date)
        ph = ET.Element(f"{{{SP}}}phasedRolloutInterval")
        ph.text = str(interval)
        item.insert(list(item).index(pub) + 1, ph)
        groups = 1 if stage == "10" else 4
        notes.append(f"`{APPCAST}`: {title}, {groups} of {GROUPS} groups ({groups / GROUPS:.1%}) are offered it now")
        notes.append(
            "held there until the next stage"
            if stage == "10"
            else "Sparkle adds a group every 7 days after this, so 100% arrives by itself within 3 weeks unless paused"
        )
    changed[APPCAST] = _write(tree)
    return changed, notes, deletions


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--feed-dir", required=True, type=Path)
    ap.add_argument("--out-dir", required=True, type=Path)
    ap.add_argument("--channel", choices=["stable", "beta"], default="stable")
    ap.add_argument("--platform", choices=["macos", "windows", "linux", "all"], required=True)
    ap.add_argument("--stage", choices=["10", "50", "100", "pause"], required=True)
    ap.add_argument("--now", help="ISO time (tests)")
    a = ap.parse_args(argv)
    now = datetime.fromisoformat(a.now) if a.now else datetime.now(timezone.utc)
    platforms = ["macos", "windows", "linux"] if a.platform == "all" else [a.platform]

    try:
        changed, notes = electron(a.feed_dir, a.channel, platforms, a.stage)
        deletions: list[str] = []
        if "macos" in platforms:
            sp_changed, sp_notes, deletions = sparkle(a.feed_dir, a.channel, a.stage, now)
            changed.update(sp_changed)
            notes += sp_notes
    except RolloutError as e:
        print(f"::error::{e}")
        return 1

    a.out_dir.mkdir(parents=True, exist_ok=True)
    out = [f"### {a.channel} feed, {a.platform}, stage {a.stage}", ""]
    out += [f"- {n}" for n in notes] + [""]
    any_diff = False
    for name, new in sorted(changed.items()):
        old_path = a.feed_dir / name
        old = old_path.read_text() if old_path.exists() else ""
        diff = list(difflib.unified_diff(old.splitlines(), new.splitlines(), f"a/{name}", f"b/{name}", lineterm="", n=2))
        if not diff:
            out.append(f"`{name}`: no change")
            continue
        any_diff = True
        (a.out_dir / name).write_text(new)
        out += [f"`{name}`", "", "```diff", *diff, "```", ""]
    (a.out_dir / "deletions.txt").write_text("".join(f"{d}\n" for d in deletions))
    if not any_diff and not deletions:
        out.append("Nothing to change.")
    print("\n".join(out))
    return 0


if __name__ == "__main__":
    sys.exit(main())
