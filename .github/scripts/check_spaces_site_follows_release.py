#!/usr/bin/env python3
"""Post-publish gate: cua.ai must follow a stable Cua Spaces release (CUA-3128).

Waits up to --timeout seconds (the site rebuild and CDN take minutes) for all of:
  - the release is published (not draft, not prerelease)
  - cua.ai/download and cua.ai/download/info show the version
  - cua.ai/download/{mac,windows,linux} redirect to an asset of that version that answers 200
  - the in-app updater feeds (Sparkle appcast, electron latest.yml / latest-linux.yml) advertise it
Exits 1 with the remaining mismatches when the time runs out.
"""
import argparse
import json
import re
import sys
import time
import urllib.error
import urllib.request

import sparkle_feed


class _NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *a, **k):
        return None


_plain = urllib.request.build_opener(_NoRedirect)
_follow = urllib.request.build_opener()


def fetch(url, method="GET", follow=True, headers=None):
    req = urllib.request.Request(url, method=method, headers={"User-Agent": "cua-spaces-release-gate/1", **(headers or {})})
    try:
        with (_follow if follow else _plain).open(req, timeout=30) as r:
            return r.status, r.headers.get("Location", ""), r.read(1 << 20) if method == "GET" else b""
    except urllib.error.HTTPError as e:
        return e.code, e.headers.get("Location", ""), b""


FEEDS = {
    # Sparkle: the newest item with no channel (what Stable apps get), not the
    # first item: a beta item sits on top of the feed (CUA-3131).
    "sparkle": ("cua-spaces-appcast.xml", None),
    "electron-win": ("latest.yml", r"(?m)^version:\s*(\S+)"),
    "electron-linux": ("latest-linux.yml", r"(?m)^version:\s*(\S+)"),
}


def mismatches(repo, ver, tag, feeds, token):
    bad = []
    gh = {"Authorization": "Bearer " + token} if token else {}
    s, _, b = fetch("https://api.github.com/repos/%s/releases/tags/%s" % (repo, tag), headers=gh)
    rel = json.loads(b) if s == 200 else {}
    if s != 200 or rel.get("draft") or rel.get("prerelease"):
        bad.append("release %s is not published as stable (status %s, prerelease=%s)" % (tag, s, rel.get("prerelease")))
    s, _, b = fetch("https://cua.ai/download")
    shown = sorted(set(re.findall(r"data-dl-version[^>]*>([^<]+)<", b.decode("utf-8", "replace"))))
    if s != 200 or shown != [ver]:
        bad.append("cua.ai/download shows %s (status %s)" % (",".join(shown) or "no version", s))
    s, _, b = fetch("https://cua.ai/download/info")
    try:
        v = json.loads(b).get("version")
    except ValueError:
        v = None
    if s != 200 or v != ver:
        bad.append("cua.ai/download/info status %s version %s" % (s, v))
    for osn in ("mac", "windows", "linux"):
        s, loc, _ = fetch("https://cua.ai/download/" + osn, follow=False)
        if s not in (301, 302, 307, 308) or "/%s/cua-spaces-%s-" % (tag, ver) not in loc:
            bad.append("cua.ai/download/%s -> %s %s" % (osn, s, loc.split("?")[0][-70:] or "no redirect"))
            continue
        s = fetch(loc, method="HEAD")[0]
        if s != 200:
            bad.append("cua.ai/download/%s asset answers %s" % (osn, s))
    base = "https://github.com/%s/releases/download/cua-spaces-latest/" % repo
    for name in feeds:
        fname, rx = FEEDS[name]
        s, _, b = fetch(base + fname)
        text = b.decode("utf-8", "replace")
        if rx is None:
            try:
                fv = sparkle_feed.stable_version(text)
                bad += ["updater feed %s: %s" % (name, v) for v in sparkle_feed.violations(text)]
            except Exception as e:  # noqa: BLE001 - unparsable feed
                fv = "unparsable (%s)" % str(e)[:40]
        else:
            m = re.search(rx, text)
            fv = m.group(1) if m else None
        if fv != ver:
            bad.append("updater feed %s advertises %s (status %s)" % (name, fv, s))
    return bad


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--version", required=True)
    ap.add_argument("--repo", default="trycua/cua")
    ap.add_argument("--feeds", default=",".join(FEEDS), help="comma list; a platform the release skipped keeps its old feed")
    ap.add_argument("--timeout", type=int, default=900)
    ap.add_argument("--interval", type=int, default=30)
    a = ap.parse_args()
    import os
    token = os.environ.get("GH_TOKEN", "")
    tag = "cua-spaces-v" + a.version
    feeds = [f for f in a.feeds.split(",") if f]
    deadline = time.time() + a.timeout
    while True:
        try:
            bad = mismatches(a.repo, a.version, tag, feeds, token)
        except Exception as e:  # network blip: retry until the deadline
            bad = ["check error: %s" % e]
        if not bad:
            print("ok: cua.ai/download, /download/info, 3 download links and %d updater feeds all serve %s" % (len(feeds), a.version))
            return 0
        print("waiting (%ds left): %s" % (max(0, deadline - time.time()), "; ".join(bad)), flush=True)
        if time.time() >= deadline:
            for b in bad:
                print("::error::%s" % b)
            print("::error::Cua Spaces %s is published but cua.ai does not serve it. Fix the site or links now." % a.version)
            return 1
        time.sleep(a.interval)


if __name__ == "__main__":
    sys.exit(main())
