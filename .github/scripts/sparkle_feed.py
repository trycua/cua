#!/usr/bin/env python3
"""Read the Cua Spaces Sparkle appcast the way the app does (CUA-3131).

Sparkle offers an app on Stable only the items with no <sparkle:channel>; an
app on Beta also gets the `beta` items. So the newest stable version is the
newest *untagged* item by <sparkle:version>, not the first item in the file.

  python3 sparkle_feed.py check  FEED.xml   # exit 1 if any item is on the wrong channel
  python3 sparkle_feed.py stable FEED.xml   # print the version Stable apps are offered
  python3 sparkle_feed.py nightly FEED.xml  # exit 1 unless every item is a nightly (the nightly appcast)

`check` is the invariant that keeps prereleases off Stable: an X.Y.Z-nightly...
item must carry `nightly`, any other X.Y.Z-suffix item `beta`, and a plain
X.Y.Z item no channel at all (a tagged stable release would hide from Stable).
"""
import re
import sys
import xml.etree.ElementTree as ET

SP = "http://www.andymatuschak.org/xml-namespaces/sparkle"


def _items(xml_text):
    return list(ET.fromstring(xml_text).iter("item"))


def _short(item):
    return (item.findtext("{%s}shortVersionString" % SP) or item.findtext("title") or "").strip()


def _channel(item):
    c = item.findtext("{%s}channel" % SP)
    return c.strip() if c and c.strip() else None


def _key(item):
    return tuple(int(p) for p in re.findall(r"\d+", item.findtext("{%s}version" % SP) or "0"))


def want_channel(short_version):
    """The channel an item of this version must carry (None: the default, Stable)."""
    base = short_version.split("+", 1)[0]
    if "-nightly" in base:
        return "nightly"
    if "-" in base:
        return "beta"
    return None


def violations(xml_text):
    bad = []
    for item in _items(xml_text):
        v, have = _short(item), _channel(item)
        want = want_channel(v)
        if have != want:
            bad.append("%s has channel %s, want %s" % (v or "(no version)", have or "(stable)", want or "(stable)"))
    return bad


def nightly_violations(xml_text):
    """Why the nightly appcast (cua-spaces-nightly-appcast.xml) is wrong: it may
    only hold X.Y.Z-nightly items on the `nightly` channel, never a stable or
    beta item (an app on Nightly would take it as the newest nightly)."""
    bad = violations(xml_text)
    for item in _items(xml_text):
        v = _short(item)
        if want_channel(v) != "nightly":
            bad.append("%s is not a nightly" % (v or "(no version)"))
    return bad


def stable_version(xml_text):
    """The version an app on Stable is offered, or None."""
    items = [i for i in _items(xml_text) if _channel(i) is None]
    return _short(max(items, key=_key)) if items else None


def main(argv):
    if len(argv) != 3 or argv[1] not in ("check", "stable", "nightly"):
        print(__doc__.strip(), file=sys.stderr)
        return 2
    with open(argv[2], encoding="utf-8") as f:
        text = f.read()
    if argv[1] == "stable":
        print(stable_version(text) or "")
        return 0
    bad = nightly_violations(text) if argv[1] == "nightly" else violations(text)
    for b in bad:
        print("::error::Sparkle appcast: %s" % b)
    if bad:
        print("::error::a prerelease without its channel would update every Stable Mac to it; fix make-appcast.sh or the feed")
        return 1
    print("ok: every appcast item is on its channel; Stable is offered %s" % stable_version(text))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
