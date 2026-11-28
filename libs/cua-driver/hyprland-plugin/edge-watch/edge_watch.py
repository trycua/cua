#!/usr/bin/env python3
"""Omarchy edge watch: change detection, config lint, pin bump, issue text.

Used by .github/workflows/omarchy-edge-watch.yml, release-omarchy-plugin.yml
and omarchy-outreach.yml. Package detection itself is requalify/channels.py;
this file only adds what the watch needs on top of its snapshot.

  check    snapshot + omarchy-qualified.json -> what changed (watch.json)
  lint     keys in Omarchy's Hyprland config the pinned Hyprland does not know
  bump     record a green run in omarchy-qualified.json
  report   open or update ONE tracking issue for a red run
  outreach the text of the Omarchy update request (never posts)
"""

import argparse
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
PLUGIN_DIR = HERE.parent
sys.path.insert(0, str(PLUGIN_DIR / "requalify"))
import channels  # noqa: E402

QUALIFIED = PLUGIN_DIR / "omarchy-qualified.json"
QUALIFIED_REL = "libs/cua-driver/hyprland-plugin/omarchy-qualified.json"
OMARCHY_EDGE = "https://pkgs.omarchy.org/edge/x86_64"
ARCH_ARCHIVE = "https://archive.archlinux.org/packages"
ISSUE_TITLE = "Omarchy edge watch failing"
ISSUE_LABEL = "omarchy-edge-watch"
# Verified 2 Nov 2026: author of "Add Cua to Install > AI" (basecamp/omarchy#11342).
OMARCHY_CONTACT = ("Spencer Bull", "spencerbull")


def base_version(version):
    """0.33.2-4 -> 0.33.2"""
    return version.rsplit("-", 1)[0]


def load(path):
    return json.loads(Path(path).read_text())


def dump(path, data):
    Path(path).write_text(json.dumps(data, indent=2) + "\n")


def omarchy_db(fetcher=channels.fetch):
    return channels.parse_db(fetcher(f"{OMARCHY_EDGE}/omarchy.db"))


def check(snapshot, qualified, released=None, db=None):
    """Compare the edge channel with the pin and the last qualified versions."""
    edge = snapshot["channels"]["omarchy-edge"]
    plugin = edge.get("omarchy_plugin", {})
    db = db or {}
    current = {
        "hyprland": edge["packages"]["hyprland"]["version"],
        "omarchy_dev": db.get("omarchy-dev", {}).get("version", ""),
        "omarchy_settings_dev": db.get("omarchy-settings-dev", {}).get("version", ""),
        "omarchy_plugin": plugin.get("version", ""),
        "omarchy_settings_file": db.get("omarchy-settings-dev", {}).get("filename", ""),
    }
    last, pin = qualified["last_qualified"], qualified["plugin_pin"]["hyprland"]
    reasons = []
    for key, label in (("hyprland", "Hyprland"), ("omarchy_dev", "omarchy-dev"),
                       ("omarchy_settings_dev", "omarchy-settings-dev"),
                       ("omarchy_plugin", "Omarchy's cua-hyprland-plugin")):
        # An empty value means the repository was unreadable; that is not a change.
        if current[key] and current[key] != last.get(key):
            reasons.append(f"{label}: {last.get(key) or 'none'} -> {current[key]}")
    if current["hyprland"] != pin and not any(r.startswith("Hyprland") for r in reasons):
        reasons.append(f"Hyprland {current['hyprland']} differs from the plugin pin {pin}")
    post_update = bool(
        released and current["omarchy_plugin"]
        and base_version(current["omarchy_plugin"]) == released
        and current["omarchy_plugin"] != last.get("omarchy_plugin"))
    return {"changed": bool(reasons), "post_update": post_update, "reasons": reasons,
            "current": current, "pin": pin, "released_plugin": released or "",
            "channel_errors": snapshot.get("errors", {})}


def latest_released_plugin(run=subprocess.run):
    """Newest published Driver release that carries plugin assets, as a version."""
    out = run(["gh", "release", "list", "--limit", "40", "--json", "tagName,isDraft"],
              capture_output=True, text=True, check=True).stdout
    for release in json.loads(out):
        tag = release["tagName"]
        if release["isDraft"] or not tag.startswith("cua-driver-rs-v"):
            continue
        assets = run(["gh", "release", "view", tag, "--json", "assets", "--jq", ".assets[].name"],
                     capture_output=True, text=True, check=True).stdout
        if "cua-hyprland-plugin-" in assets:
            return tag[len("cua-driver-rs-v"):]
    return ""


# ---- config lint -----------------------------------------------------------

KEY_RE = re.compile(rb"(?<![ -~])([a-z_]+(?::[a-z_0-9.]+)+)(?![ -~])")
LUA_TOKEN = re.compile(r"[A-Za-z_][A-Za-z_0-9]*|[{}()=.]|\S")


def binary_keys(data):
    """`section:key` option names the Hyprland binary carries as C strings."""
    return {m.decode().replace(".", ":") for m in KEY_RE.findall(data)}


def strip_lua(text):
    text = re.sub(r"--\[(=*)\[.*?\]\1\]", " ", text, flags=re.S)
    text = re.sub(r"--[^\n]*", " ", text)
    return re.sub(r'"(?:\\.|[^"\\\n])*"|\'(?:\\.|[^\'\\\n])*\'|\[(=*)\[.*?\]\1\]', '""', text, flags=re.S)


def config_keys(text):
    """Option paths (joined with ':') set inside every hl.config({...}) call."""
    tokens = LUA_TOKEN.findall(strip_lua(text))
    found, i = [], 0
    while i < len(tokens):
        if tokens[i:i + 5] == ["hl", ".", "config", "(", "{"]:
            i += 4
            stack, depth = [], 0
            while i < len(tokens):
                t = tokens[i]
                if t == "{":
                    depth += 1
                    if depth > 1:
                        named = tokens[i - 1] == "=" and tokens[i - 2].isidentifier()
                        stack.append(tokens[i - 2] if named else None)
                elif t == "}":
                    depth -= 1
                    if depth == 0:
                        break
                    stack.pop()
                elif t == "=" and tokens[i - 1].isidentifier() and tokens[i + 1:i + 2] != ["{"]:
                    found.append(":".join([s for s in stack if s] + [tokens[i - 1]]))
                i += 1
        i += 1
    return found


def unknown_keys(files, known):
    prefixes = {k.split(":")[0] for k in known}
    bad = []
    for name, text in files.items():
        for key in config_keys(text):
            if key.split(":")[0] in prefixes and key not in known \
                    and not any(k.startswith(key + ":") for k in known):
                bad.append((name, key))
    return bad


def download(url, dest):
    Path(dest).write_bytes(channels.fetch(url))


def extract(pkg, dest, *members):
    subprocess.run(["tar", "--zstd", "-xf", str(pkg), "-C", str(dest), *members], check=True)


def lint(pin, settings_file, work):
    work = Path(work)
    work.mkdir(parents=True, exist_ok=True)
    hl, st = work / "hyprland.pkg.tar.zst", work / "settings.pkg.tar.zst"
    download(f"{ARCH_ARCHIVE}/h/hyprland/hyprland-{pin}-x86_64.pkg.tar.zst", hl)
    download(f"{OMARCHY_EDGE}/{settings_file}", st)
    extract(hl, work, "usr/bin/Hyprland")
    extract(st, work)  # a small config-only package
    known = binary_keys((work / "usr/bin/Hyprland").read_bytes())
    paths = sorted(work.glob("usr/share/omarchy/**/*.lua")) + sorted(work.glob("etc/skel/**/*.lua"))
    files = {str(p.relative_to(work)): p.read_text(errors="replace") for p in paths}
    return len(known), len(files), unknown_keys(files, known)


# ---- bump, report, outreach ------------------------------------------------

def bump(qualified, watch, run_url, today, drop_stopgaps=False):
    cur = watch["current"]
    qualified["plugin_pin"]["hyprland"] = cur["hyprland"]
    last = qualified["last_qualified"]
    for key in ("hyprland", "omarchy_dev", "omarchy_settings_dev", "omarchy_plugin"):
        if cur.get(key):
            last[key] = cur[key]
    last.update(qualified_at=today, run_url=run_url)
    if drop_stopgaps:
        qualified["stopgaps"] = []
    return qualified


def report(repo, run_id, run=subprocess.run, dry=False):
    def gh(*args, check=True):
        if dry:
            print("gh", *args)
            return ""
        return run(["gh", *args], capture_output=True, text=True, check=check).stdout

    run_url = f"https://github.com/{repo}/actions/runs/{run_id}"
    jobs = json.loads(gh("api", f"repos/{repo}/actions/runs/{run_id}/jobs?per_page=100") or '{"jobs": []}')["jobs"]
    bad = [j for j in jobs if j["conclusion"] == "failure"]
    failed = [(j["name"], s["name"]) for j in bad for s in j["steps"] if s["conclusion"] == "failure"]
    # The run is still in progress, so read the logs of its finished failed jobs.
    log = "\n".join(gh("api", f"repos/{repo}/actions/jobs/{j['id']}/logs", check=False)
                    for j in bad[:3])
    excerpt = "\n".join(log.splitlines()[-40:])[-6000:]
    arts = json.loads(gh("api", f"repos/{repo}/actions/runs/{run_id}/artifacts?per_page=100")
                      or '{"artifacts": []}')["artifacts"]
    links = [f"- [{a['name']}](https://github.com/{repo}/actions/runs/{run_id}/artifacts/{a['id']})"
             for a in arts if a["name"] == "omarchy-release" or a["name"].startswith("requalify-result-")]
    steps = "\n".join(f"- `{j}` / `{s}`" for j, s in failed) or "- (no failed step recorded; see the run)"
    body = (f"Omarchy edge watch run: {run_url}\n\n**Failing step**\n{steps}\n\n"
            f"**Log excerpt**\n```\n{excerpt}\n```\n\n**Artifacts** (the doctor screenshots are in "
            f"`omarchy-release`, under `evidence/`)\n{chr(10).join(links) or '- none uploaded'}\n")
    existing = gh("issue", "list", "--repo", repo, "--state", "open", "--label", ISSUE_LABEL,
                  "--json", "number,title", "--jq",
                  f'.[]|select(.title=="{ISSUE_TITLE}")|.number', check=False).split()
    if existing:
        gh("issue", "comment", existing[0], "--repo", repo, "--body", body)
        return f"commented on #{existing[0]}"
    gh("label", "create", ISSUE_LABEL, "--repo", repo, "--color", "B60205",
       "--description", "Omarchy edge watch failures", check=False)
    gh("issue", "create", "--repo", repo, "--title", ISSUE_TITLE, "--label", ISSUE_LABEL, "--body",
       body + "\nOne issue, a comment per failing run. Workflow: `Omarchy: edge watch`.")
    return "opened issue"


def git_out(*args, run=subprocess.run):
    done = run(["git", *args], capture_output=True, text=True)
    return done.stdout.strip() if done.returncode == 0 else None


def outreach(tag, repo, qualified, run=subprocess.run):
    version = tag.split("-v", 1)[1]
    url = f"https://github.com/{repo}/releases/tag/{tag}"
    prev = ""
    for t in (git_out("tag", "--list", "cua-driver-rs-v*", "--sort=-v:refname", run=run) or "").split():
        if t != tag and git_out("merge-base", "--is-ancestor", t, tag, run=run) is not None:
            prev = t
            break
    log = git_out("log", "--no-merges", "--format=- %s", f"{prev}..{tag}" if prev else tag, "--",
                  "libs/cua-driver/hyprland-plugin", run=run) or ""
    at_tag = git_out("show", f"{tag}:{QUALIFIED_REL}", run=run)
    pin = (json.loads(at_tag) if at_tag else qualified)["plugin_pin"]["hyprland"]
    name, login = OMARCHY_CONTACT
    title = f"Update cua-hyprland-plugin to {version} (Hyprland {pin})"
    fallback = "- No plugin source changes; the release re-pins the plugin for the Hyprland build above."
    body = f"""Hi @{login} ({name}),

A new `cua-hyprland-plugin` is out and Omarchy's package can move to it.

- Plugin version: {version}
- Built for Hyprland: {pin}
- Release (source archive, build kit and checksums): {url}

What changed in the plugin since {prev or 'the previous release'}:
{log or fallback}

Could the `cua-hyprland-plugin` package in the edge channel be updated to this release? We rerun our Omarchy
image and doctor checks against your edge packages every day and will confirm here once the new package is out.
"""
    return title, body


def main(argv=None):
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="cmd", required=True)
    c = sub.add_parser("check")
    c.add_argument("--snapshot", required=True)
    c.add_argument("--qualified", default=str(QUALIFIED))
    c.add_argument("--released", default=None, help="released plugin version; default: ask gh")
    c.add_argument("--output", required=True)
    n = sub.add_parser("lint")
    n.add_argument("--watch", required=True)
    n.add_argument("--against", choices=["pin", "candidate"], default="candidate",
                   help="Hyprland to lint against: the current plugin pin, or the edge build we would pin")
    n.add_argument("--work", default=None)
    b = sub.add_parser("bump")
    b.add_argument("--watch", required=True)
    b.add_argument("--qualified", default=str(QUALIFIED))
    b.add_argument("--run-url", required=True)
    b.add_argument("--today", required=True)
    b.add_argument("--drop-stopgaps", action="store_true")
    r = sub.add_parser("report")
    r.add_argument("--repo", required=True)
    r.add_argument("--run-id", required=True)
    r.add_argument("--dry-run", action="store_true")
    o = sub.add_parser("outreach")
    o.add_argument("--tag", required=True)
    o.add_argument("--repo", required=True)
    o.add_argument("--title-out", required=True)
    o.add_argument("--body-out", required=True)
    a = p.parse_args(argv)

    if a.cmd == "check":
        released = a.released if a.released is not None else latest_released_plugin()
        result = check(load(a.snapshot), load(a.qualified), released, omarchy_db())
        dump(a.output, result)
        print(json.dumps(result, indent=2))
    elif a.cmd == "lint":
        watch = load(a.watch)
        # "candidate" is the build a green run would pin; "pin" is what ships today
        # (keys it lacks are why the image needs a stopgap until the pin moves).
        target = watch["current"]["hyprland"] if a.against == "candidate" else watch["pin"]
        keys, files, bad = lint(target, watch["current"]["omarchy_settings_file"],
                                a.work or tempfile.mkdtemp())
        print(f"{keys} option names in Hyprland {target}; {files} Omarchy Lua config files")
        for name, key in bad:
            print(f"::error file={name}::{key} is not known to Hyprland {target}")
        sys.exit(1 if bad else 0)
    elif a.cmd == "bump":
        dump(a.qualified, bump(load(a.qualified), load(a.watch), a.run_url, a.today, a.drop_stopgaps))
    elif a.cmd == "report":
        print(report(a.repo, a.run_id, dry=a.dry_run))
    elif a.cmd == "outreach":
        title, body = outreach(a.tag, a.repo, load(QUALIFIED))
        Path(a.title_out).write_text(title)
        Path(a.body_out).write_text(body)
        print(f"# {title}\n\n{body}")


if __name__ == "__main__":
    main()
