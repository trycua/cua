#!/usr/bin/env python3
"""Decide whether the Cua Spaces staged rollout advances, waits or pauses.

Read-only. It reads the live feeds, works out the current stage and when it
started, checks the health gates and prints one JSON decision. The workflow
.github/workflows/release-rollout-auto.yml acts on it (dispatches
release-rollout.yml, posts the alert); this script never changes anything.

Stage and start, from the feeds (nothing else to keep in step):
  stage 10  latest.yml stagingPercentage 10, newest stable appcast item has a 10-year
            phasedRolloutInterval; started at its pubDate (the rollout sets it to now)
  stage 50  stagingPercentage 50, interval 1 week; started at pubDate + 3 weeks
  100 / paused / anything that disagrees between the feeds: no action

Decision (the Play Store / Firebase / Sentry release-health pattern):
  hold      a human said hold: never advance (a failing gate still pauses)
  wait      the stage is younger than --min-age-hours, or a gate has too little data
  pause     a gate is past its threshold: the workflow pauses and alerts
  advance   old enough, every gate has enough data and passes: 10 -> 50 -> 100

Gates, PostHog project 56041 (EU), spaces_app events of the new version versus the
previous stable (the most active non-prerelease version in the week before the stage):
  adoption     at least --min-installs installs of the new version sent cua_app_active
               since the stage began (no data is not healthy, it waits)
  space_create error share of cua_space_create on the new version is at most
               --max-create-error-delta points above the previous version's over the
               same window (needs --min-create-events on both sides)
  update       share of failed among found/failed cua_app_update from the previous version
               since the stage began (the version that tries to install the new one) is at
               most --max-update-failure-delta points above its share the week before
               (needs --min-update-events)
  incident     no active incident.io incident with "spaces" in its name (needs
               INCIDENT_IO_API_KEY; without it the gate is reported as skipped)
  crash        Sentry release health (org and project from config/spaces-crash-reporting.json,
               release cua-spaces-desktop@<version>, environment stable): crash-free sessions of
               the new version since the stage began are at most --max-crash-free-drop points
               below the previous version's over the same window (needs --min-sessions on the
               new version). A previous version with fewer than --min-sessions sessions (it
               shipped before crash reporting) is no baseline: the new version must then be at
               least --min-crash-free percent. Without SENTRY_READ_TOKEN the gate is reported as skipped;
               if Sentry cannot be read (unreachable, 401/403, bad config) it waits and says why.

Standard library only. Keys come from the environment: POSTHOG_API_KEY (query read),
INCIDENT_IO_API_KEY (optional), SENTRY_READ_TOKEN (optional, an Internal Integration token with
Project, Issue & Event and Organization read; the SENTRY_AUTH_TOKEN org:ci token cannot read stats).
SENTRY_API_HOST overrides the Sentry host (default https://de.sentry.io, the EU region of org cua-ai).
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
import urllib.error
import urllib.parse
import urllib.request
import xml.etree.ElementTree as ET
from datetime import datetime, timedelta, timezone
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import release_rollout as rr  # noqa: E402

POSTHOG = "https://eu.posthog.com/api/projects/56041/query/"
SENTRY_DEFAULT_HOST = "https://de.sentry.io"
SENTRY_CONFIG = Path(__file__).resolve().parents[2] / "config" / "spaces-crash-reporting.json"
NEXT = {"10": "50", "50": "100"}


def _num(text: str | None) -> int | None:
    return int(text) if text and text.strip().isdigit() else None


def read_state(feed_dir: Path) -> dict:
    """{version, stage ('10'|'50'|'100'|'pause'), since (datetime|None), problem (str|None)}."""
    out: dict = {"version": None, "stage": None, "since": None, "problem": None}
    yml = feed_dir / "latest.yml"
    if not yml.exists():
        out["problem"] = "latest.yml is not on the feed"
        return out
    text = yml.read_text()
    out["version"] = rr.feed_version(text)
    m = re.search(r"^stagingPercentage:\s*(\d+)\s*$", text, re.M)
    staging = m.group(1) if m else None
    if staging == "0":
        out["stage"] = "pause"
        return out
    if staging is None:
        out["stage"] = "100"
        return out
    if staging not in ("10", "50"):
        out["problem"] = f"stagingPercentage {staging} is not a stage"
        return out
    path = feed_dir / rr.APPCAST
    if not path.exists():
        out["problem"] = "no appcast, cannot tell when the stage started"
        return out
    items = rr._channel_items(ET.parse(path).getroot(), "stable")
    if not items:
        out["problem"] = "the appcast has no stable item (paused?) while latest.yml is staged"
        return out
    item = max(items, key=rr._version_key)
    if item.findtext("title") != out["version"]:
        out["problem"] = f"appcast newest is {item.findtext('title')}, latest.yml is {out['version']}"
        return out
    interval = _num(item.findtext(f"{{{rr.SP}}}phasedRolloutInterval"))
    pub = item.findtext("pubDate")
    want = rr.HOLD_SECONDS if staging == "10" else rr.WEEK
    if interval != want or not pub:
        out["problem"] = f"latest.yml is at {staging}% but the appcast item has interval {interval}: the feeds disagree"
        return out
    start = parsedate(pub)
    out["stage"] = staging
    out["since"] = start if staging == "10" else start + timedelta(seconds=3 * rr.WEEK)
    return out


def parsedate(text: str) -> datetime:
    return datetime.strptime(text.strip(), "%a, %d %b %Y %H:%M:%S %z").astimezone(timezone.utc)


def posthog(query: str, key: str) -> list[list]:
    req = urllib.request.Request(
        POSTHOG,
        json.dumps({"query": {"kind": "HogQLQuery", "query": query}}).encode(),
        {"Authorization": "Bearer " + key, "Content-Type": "application/json"},
    )
    with urllib.request.urlopen(req, timeout=120) as r:
        return json.load(r).get("results", [])


def lit(ts: datetime) -> str:
    return "toDateTime('%s', 'UTC')" % ts.strftime("%Y-%m-%d %H:%M:%S")


def previous_version(q, new: str, since: datetime) -> str | None:
    rows = q(
        "select properties.product_version v, count(distinct distinct_id) n from events "
        "where event = 'cua_app_active' and properties.product = 'spaces_app' "
        f"and timestamp >= {lit(since - timedelta(days=7))} and timestamp < {lit(since)} "
        f"and properties.product_version != '{new}' and properties.product_version not like '%-%' "
        "group by v order by n desc limit 1"
    )
    return rows[0][0] if rows else None


def share(bad: int, total: int) -> float:
    return 100.0 * bad / total if total else 0.0


def gate_adoption(q, new, since, now, a) -> dict:
    n = int(q(
        "select count(distinct distinct_id) from events where event = 'cua_app_active' "
        f"and properties.product = 'spaces_app' and properties.product_version = '{new}' "
        f"and timestamp >= {lit(since)} and timestamp < {lit(now)}"
    )[0][0])
    ok = n >= a.min_installs
    return {"gate": "adoption", "status": "pass" if ok else "wait",
            "detail": f"{n} installs of {new} active since the stage began (need {a.min_installs})"}


def gate_space_create(q, new, prev, since, now, a) -> dict:
    def counts(v):
        rows = q(
            "select properties.outcome o, count() from events where event = 'cua_space_create' "
            f"and properties.product = 'spaces_app' and properties.product_version = '{v}' "
            f"and timestamp >= {lit(since)} and timestamp < {lit(now)} group by o"
        )
        total = sum(int(r[1]) for r in rows)
        return sum(int(r[1]) for r in rows if r[0] == "error"), total

    (eb, et), (pb, pt) = counts(new), (counts(prev) if prev else (0, 0))
    if et < a.min_create_events or pt < a.min_create_events:
        return {"gate": "space_create", "status": "wait",
                "detail": f"{et} Space creates on {new}, {pt} on {prev} (need {a.min_create_events} each)"}
    d = share(eb, et) - share(pb, pt)
    return {"gate": "space_create", "status": "fail" if d > a.max_create_error_delta else "pass",
            "detail": f"create errors {share(eb, et):.1f}% on {new} ({eb}/{et}) vs {share(pb, pt):.1f}% on {prev} ({pb}/{pt}), "
                      f"delta {d:+.1f} points (limit +{a.max_create_error_delta})"}


def gate_update(q, prev, since, now, a) -> dict:
    if not prev:
        return {"gate": "update", "status": "wait", "detail": "no previous version to compare"}

    def counts(lo, hi):
        rows = q(
            "select properties.action a, count() from events where event = 'cua_app_update' "
            f"and properties.product = 'spaces_app' and properties.product_version = '{prev}' "
            "and properties.channel = 'stable' and properties.action in ('found', 'failed') "
            f"and timestamp >= {lit(lo)} and timestamp < {lit(hi)} group by a"
        )
        d = {r[0]: int(r[1]) for r in rows}
        return d.get("failed", 0), d.get("found", 0) + d.get("failed", 0)

    (fb, ft), (bb, bt) = counts(since, now), counts(since - timedelta(days=7), since)
    if ft < a.min_update_events:
        return {"gate": "update", "status": "wait",
                "detail": f"{ft} update attempts from {prev} since the stage began (need {a.min_update_events})"}
    d = share(fb, ft) - share(bb, bt)
    return {"gate": "update", "status": "fail" if d > a.max_update_failure_delta else "pass",
            "detail": f"update failures {share(fb, ft):.1f}% ({fb}/{ft}) vs {share(bb, bt):.1f}% the week before ({bb}/{bt}), "
                      f"delta {d:+.1f} points (limit +{a.max_update_failure_delta})"}


def gate_incident(key: str | None, fetch=None) -> dict:
    if not key:
        return {"gate": "incident", "status": "skip", "detail": "INCIDENT_IO_API_KEY not set, gate skipped"}
    try:
        if fetch:
            data = fetch()
        else:
            req = urllib.request.Request(
                "https://api.incident.io/v2/incidents?status_category%5Bone_of%5D=active&page_size=100",
                headers={"Authorization": "Bearer " + key})
            with urllib.request.urlopen(req, timeout=30) as r:
                data = json.load(r)
    except (urllib.error.URLError, OSError, ValueError) as e:
        return {"gate": "incident", "status": "wait", "detail": f"incident.io unreachable ({e}); not advancing blind"}
    hits = [i for i in data.get("incidents", []) if "spaces" in (i.get("name") or "").lower()]
    if hits:
        names = ", ".join(f"{i.get('reference', '?')} {i.get('name')}" for i in hits[:3])
        return {"gate": "incident", "status": "fail", "detail": f"active Spaces incident: {names}"}
    return {"gate": "incident", "status": "pass", "detail": "no active incident naming Spaces"}


def sentry_base() -> str:
    return (os.environ.get("SENTRY_API_HOST") or SENTRY_DEFAULT_HOST).rstrip("/") + "/api/0"


def sentry_get(path: str, token: str, params: list[tuple[str, str]] | None = None) -> dict:
    url = sentry_base() + path + ("?" + urllib.parse.urlencode(params) if params else "")
    req = urllib.request.Request(url, headers={"Authorization": "Bearer " + token})
    with urllib.request.urlopen(req, timeout=60) as r:
        return json.load(r)


def sentry_sessions(token: str, cfg: dict, project_id: str, version: str, since: datetime, now: datetime) -> tuple[int, float | None]:
    """(sessions, crash-free fraction or None) of one release on the stable environment in a window."""
    try:
        data = sentry_get(
            "/organizations/%s/sessions/" % cfg["org"], token,
            [("project", project_id), ("environment", "stable"),
             ("field", "sum(session)"), ("field", "crash_free_rate(session)"),
             ("query", 'release:"cua-spaces-desktop@%s"' % version),
             ("start", since.strftime("%Y-%m-%dT%H:%M:%S")), ("end", now.strftime("%Y-%m-%dT%H:%M:%S")),
             ("interval", "1d")])
    except urllib.error.HTTPError as e:
        # Sentry answers 400/404 "Unknown environments selected" until a stable session exists: no data yet.
        if e.code in (400, 404):
            return 0, None
        raise
    total, rate = 0, None
    for g in data.get("groups", []):
        t = g.get("totals", {})
        total += int(t.get("sum(session)") or 0)
        rate = t.get("crash_free_rate(session)")
    return total, rate


def _why(e: Exception) -> str:
    if isinstance(e, urllib.error.HTTPError):
        return f"HTTP {e.code} from {(e.filename or '').split('/api/')[0]}"
    return f"{type(e).__name__}: {e}"


def gate_crash(token: str | None, new: str, prev: str | None, since: datetime, now: datetime, a, cfg: dict | None = None, fetch=None) -> dict:
    """Crash-free sessions of the new version against the previous one (Sentry release health)."""
    if not token:
        return {"gate": "crash", "status": "skip", "detail": "SENTRY_READ_TOKEN not set, gate skipped"}
    try:
        cfg = cfg or json.loads(SENTRY_CONFIG.read_text())
        if fetch is None:
            project_id = str(sentry_get("/projects/%s/%s/" % (cfg["org"], cfg["project"]), token)["id"])
            fetch = lambda v: sentry_sessions(token, cfg, project_id, v, since, now)  # noqa: E731
        n_new, r_new = fetch(new)
        n_prev, r_prev = fetch(prev) if prev else (0, None)
    except (urllib.error.URLError, OSError, ValueError, KeyError) as e:
        return {"gate": "crash", "status": "wait",
                "detail": f"cannot read crash stats from Sentry ({_why(e)}); keeping the stage, not advancing blind. "
                          "Check SENTRY_READ_TOKEN and SENTRY_API_HOST"}
    if n_new < a.min_sessions or r_new is None:
        return {"gate": "crash", "status": "wait",
                "detail": f"{n_new} sessions of {new} reported to Sentry since the stage began (need {a.min_sessions})"}
    new_pct = 100.0 * r_new
    if n_prev < a.min_sessions or r_prev is None:
        ok = new_pct >= a.min_crash_free
        return {"gate": "crash", "status": "pass" if ok else "fail",
                "detail": f"crash-free sessions {new_pct:.2f}% on {new} ({n_new} sessions); no baseline ({n_prev} sessions on {prev}), "
                          f"floor {a.min_crash_free}%"}
    prev_pct = 100.0 * r_prev
    drop = prev_pct - new_pct
    return {"gate": "crash", "status": "fail" if drop > a.max_crash_free_drop else "pass",
            "detail": f"crash-free sessions {new_pct:.2f}% on {new} ({n_new}) vs {prev_pct:.2f}% on {prev} ({n_prev}), "
                      f"drop {drop:+.2f} points (limit {a.max_crash_free_drop})"}


def decide(state: dict, now: datetime, a, q=None, incident=None, hold: str = "", crash=None) -> dict:
    d: dict = {"version": state["version"], "stage": state["stage"],
               "since": state["since"].isoformat() if state["since"] else None,
               "action": "none", "next": None, "gates": [], "why": ""}
    if state["problem"]:
        d["why"] = "cannot decide: " + state["problem"]
        return d
    if state["stage"] not in NEXT:
        d["why"] = {"100": "fully rolled out", "pause": "paused: resuming is a person's call"}.get(state["stage"], "nothing to do")
        return d
    since = state["since"]
    age = (now - since).total_seconds() / 3600
    d["age_hours"] = round(age, 1)
    if age < 0:
        d["why"] = "the stage start is in the future"
        return d
    new = state["version"]
    prev = previous_version(q, new, since)
    d["previous"] = prev
    d["gates"] = [gate_adoption(q, new, since, now, a), gate_space_create(q, new, prev, since, now, a),
                  gate_update(q, prev, since, now, a), incident or gate_incident(os.environ.get("INCIDENT_IO_API_KEY")),
                  crash or gate_crash(os.environ.get("SENTRY_READ_TOKEN"), new, prev, since, now, a)]
    failed = [g for g in d["gates"] if g["status"] == "fail"]
    waiting = [g for g in d["gates"] if g["status"] == "wait"]
    if failed:
        d["action"] = "pause"
        d["why"] = "gate failed: " + "; ".join(g["detail"] for g in failed)
    elif hold:
        d["action"] = "hold"
        d["why"] = "hold is set (" + hold + "): not advancing"
    elif age < a.min_age_hours:
        d["action"] = "wait"
        d["why"] = f"stage is {age:.1f}h old, needs {a.min_age_hours}h"
    elif waiting:
        d["action"] = "wait"
        d["why"] = "not enough data: " + "; ".join(g["detail"] for g in waiting)
    else:
        d["action"] = "advance"
        d["next"] = NEXT[state["stage"]]
        d["why"] = f"{age:.1f}h at {state['stage']}%, every gate passes"
    return d


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--feed-dir", required=True, type=Path)
    ap.add_argument("--now", help="ISO time (tests)")
    ap.add_argument("--hold", default=os.environ.get("ROLLOUT_HOLD", ""), help="non-empty: do not advance")
    ap.add_argument("--min-age-hours", type=float, default=24)
    ap.add_argument("--min-installs", type=int, default=15)
    ap.add_argument("--min-create-events", type=int, default=30)
    ap.add_argument("--max-create-error-delta", type=float, default=10.0)
    ap.add_argument("--min-update-events", type=int, default=20)
    ap.add_argument("--max-update-failure-delta", type=float, default=15.0)
    ap.add_argument("--min-sessions", type=int, default=100)
    ap.add_argument("--max-crash-free-drop", type=float, default=0.5)
    ap.add_argument("--min-crash-free", type=float, default=99.0)
    a = ap.parse_args(argv)
    now = datetime.fromisoformat(a.now) if a.now else datetime.now(timezone.utc)
    key = os.environ.get("POSTHOG_API_KEY", "")
    state = read_state(a.feed_dir)
    if not key and state["stage"] in NEXT and not state["problem"]:
        print("::error::POSTHOG_API_KEY is not set", file=sys.stderr)
        return 2
    d = decide(state, now, a, q=lambda s: posthog(s, key), hold=a.hold.strip())
    print(json.dumps(d, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
