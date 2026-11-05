"""App-state helpers for the native-app tasks (Calendar, Reminders, Notes) through JavaScript for Automation.

These run only inside the macOS VM, as the bench user: from a task's reset (with --apply-app-state) and its
`semantics.export` step, never from tests on a workstation. They need Automation consent for the process
that runs them (granted once when the VM is provisioned).
"""

from __future__ import annotations

import json
import subprocess
from pathlib import Path
from typing import Any

OSASCRIPT = "/usr/bin/osascript"

JS_HELPERS = r"""
function pad(n) { return String(n).padStart(2, "0"); }
function local(d) {
  if (!d) return null;
  return d.getFullYear() + "-" + pad(d.getMonth() + 1) + "-" + pad(d.getDate()) + "T" + pad(d.getHours()) + ":" + pad(d.getMinutes());
}
function norm(s) { return String(s || "").split(/\s+/).filter(Boolean).join(" ").toLowerCase(); }
function parseLocal(s) { const [a, b] = s.split("T"); const [y, m, d] = a.split("-").map(Number); const [hh, mm] = (b || "00:00").split(":").map(Number); return new Date(y, m - 1, d, hh, mm, 0); }
"""


def jxa(script: str, timeout: int = 120) -> Any:
    """Run a JXA script whose last expression is JSON.stringify(...); return the parsed value."""
    done = subprocess.run([OSASCRIPT, "-l", "JavaScript", "-e", JS_HELPERS + script], capture_output=True, text=True, timeout=timeout)
    if done.returncode != 0:
        raise RuntimeError(f"osascript failed: {done.stderr.strip()[-500:]}")
    out = done.stdout.strip()
    return json.loads(out) if out else None


def quit_app(name: str) -> None:
    subprocess.run([OSASCRIPT, "-e", f'if application "{name}" is running then tell application "{name}" to quit'], capture_output=True, timeout=60)


# ------------------------------------------------------------------ Calendar

def calendar_reset(names: list[str], create: dict[str, list[dict[str, Any]]], purge: list[str] | None = None) -> None:
    """Delete the named calendars (if present; names compared whitespace-collapsed and case-insensitive, as the
    checkers do), delete every event titled like one of `purge` in any calendar (a trial may leave the target
    event in the default calendar, and the checkers count it everywhere), then create the calendars in `create`.

    Event dicts: summary, start, end ("YYYY-MM-DDTHH:MM" local), optional location, description, allday."""
    script = f"""
const app = Application("Calendar");
const drop = {json.dumps(names)};
const create = {json.dumps(create)};
// delete by name, one at a time: deleting while walking index specifiers fails
const dropN = drop.map(norm); const purgeN = {json.dumps([" ".join(str(p).split()).lower() for p in (purge or [])])};
for (let i = 0; i < 20; i++) {{ const hit = app.calendars().find(c => dropN.includes(norm(c.name()))); if (!hit) break; app.delete(app.calendars.byName(hit.name())); }}
if (purgeN.length) app.calendars().forEach(c => {{ let evs = []; try {{ evs = c.events(); }} catch (x) {{ return; }}
  evs.forEach(e => {{ try {{ if (purgeN.includes(norm(e.summary()))) app.delete(e); }} catch (x) {{}} }}); }});
Object.keys(create).forEach(name => {{
  const cal = app.Calendar({{name: name}});
  app.calendars.push(cal);
  const target = app.calendars.byName(name);
  create[name].forEach(ev => {{
    const props = {{summary: ev.summary, startDate: parseLocal(ev.start), endDate: parseLocal(ev.end)}};
    if (ev.location) props.location = ev.location;
    if (ev.description) props.description = ev.description;
    if (ev.allday) props.alldayEvent = true;
    target.events.push(app.Event(props));
  }});
}});
JSON.stringify(app.calendars().map(c => c.name()));
"""
    jxa(script)


def calendar_export(names: list[str] | None = None) -> dict[str, Any]:
    """Every calendar's name; for the named calendars (all when None) their events with alarms."""
    script = f"""
const app = Application("Calendar");
const want = {json.dumps(names)};
const out = {{calendars: [], events: []}};
app.calendars().forEach(c => {{
  let name = null;
  try {{ name = c.name(); }} catch (x) {{ return; }}
  out.calendars.push(name);
  if (want && !want.includes(name)) return;
  let evs = [];
  try {{ evs = c.events(); }} catch (x) {{ out.errors = (out.errors || []).concat([name]); return; }}
  evs.forEach(e => {{
    const alarms = [];
    try {{ e.displayAlarms().forEach(a => alarms.push({{kind: "display", minutes: a.triggerInterval()}})); }} catch (x) {{}}
    try {{ e.soundAlarms().forEach(a => alarms.push({{kind: "sound", minutes: a.triggerInterval()}})); }} catch (x) {{}}
    try {{ e.mailAlarms().forEach(a => alarms.push({{kind: "mail", minutes: a.triggerInterval()}})); }} catch (x) {{}}
    let rec = null; try {{ rec = e.recurrence(); }} catch (x) {{}}
    out.events.push({{calendar: name, summary: e.summary(), start: local(e.startDate()), end: local(e.endDate()),
      location: e.location() || "", description: e.description() || "", allday: e.alldayEvent(), recurrence: rec || "",
      alarms: alarms, uid: e.uid()}});
  }});
}});
JSON.stringify(out);
"""
    return jxa(script)


# ------------------------------------------------------------------ Reminders

def reminders_reset(names: list[str], create: dict[str, list[dict[str, Any]]], purge: list[str] | None = None) -> None:
    """Delete the named lists (if present; names normalised as the checkers do), delete every reminder named like
    one of `purge` in any list (the checkers count it everywhere), then create lists with reminders.

    Reminder dicts: name, optional body, due ("YYYY-MM-DDTHH:MM"), priority (0 none, 1 high, 5 medium, 9 low),
    flagged, completed."""
    script = f"""
const app = Application("Reminders");
const drop = {json.dumps(names)};
const create = {json.dumps(create)};
const dropN = drop.map(norm); const purgeN = {json.dumps([" ".join(str(p).split()).lower() for p in (purge or [])])};
for (let i = 0; i < 20; i++) {{ const hit = app.lists().find(l => dropN.includes(norm(l.name()))); if (!hit) break; app.delete(app.lists.byName(hit.name())); }}
if (purgeN.length) app.lists().forEach(l => {{ l.reminders().forEach(r => {{ try {{ if (purgeN.includes(norm(r.name()))) app.delete(r); }} catch (x) {{}} }}); }});
Object.keys(create).forEach(name => {{
  app.lists.push(app.List({{name: name}}));
  const target = app.lists.byName(name);
  create[name].forEach(r => {{
    const props = {{name: r.name}};
    if (r.body) props.body = r.body;
    if (r.due) props.dueDate = parseLocal(r.due);
    if (r.priority) props.priority = r.priority;
    if (r.flagged) props.flagged = true;
    target.reminders.push(app.Reminder(props));
    if (r.completed) target.reminders.byName(r.name).completed = true;
  }});
}});
JSON.stringify(app.lists().map(l => l.name()));
"""
    jxa(script)


def reminders_export(names: list[str] | None = None) -> dict[str, Any]:
    script = f"""
const app = Application("Reminders");
const want = {json.dumps(names)};
const out = {{lists: [], reminders: []}};
app.lists().forEach(l => {{
  const name = l.name();
  out.lists.push(name);
  if (want && !want.includes(name)) return;
  l.reminders().forEach(r => {{
    let flagged = null; try {{ flagged = r.flagged(); }} catch (x) {{}}
    out.reminders.push({{list: name, name: r.name(), body: r.body() || "", completed: r.completed(),
      due: local(r.dueDate()), allday_due: local(r.alldayDueDate ? r.alldayDueDate() : null), priority: r.priority(), flagged: flagged}});
  }});
}});
JSON.stringify(out);
"""
    return jxa(script, timeout=180)


# ------------------------------------------------------------------ Notes

def notes_reset(folders: list[str], create: dict[str, list[dict[str, str]]], purge: list[str] | None = None) -> None:
    """Delete the named folders of the default account (if present; names normalised), delete every note titled
    like one of `purge` in any folder, empty Recently Deleted (the old notes land there and the agent would see
    them), then create folders with notes.

    Note dicts: name (the title, first line) and body (HTML)."""
    script = f"""
const app = Application("Notes");
const drop = {json.dumps(folders)};
const create = {json.dumps(create)};
const account = app.defaultAccount();
const dropN = drop.map(norm); const purgeN = {json.dumps([" ".join(str(p).split()).lower() for p in (purge or [])])};
for (let i = 0; i < 20; i++) {{ const hit = account.folders().find(f => dropN.includes(norm(f.name()))); if (!hit) break; app.delete(account.folders.byName(hit.name())); }}
if (purgeN.length) account.folders().forEach(f => {{ if (norm(f.name()) === "recently deleted") return; f.notes().forEach(x => {{ try {{ if (purgeN.includes(norm(x.name()))) app.delete(x); }} catch (e) {{}} }}); }});
for (let pass = 0; pass < 3; pass++) account.folders().forEach(f => {{ if (norm(f.name()) !== "recently deleted") return; f.notes().forEach(x => {{ try {{ app.delete(x); }} catch (e) {{}} }}); }});
Object.keys(create).forEach(name => {{
  account.folders.push(app.Folder({{name: name}}));
  const folder = account.folders.byName(name);
  create[name].forEach(n => folder.notes.push(app.Note({{body: n.body}})));  // Notes takes the title from the first line
}});
JSON.stringify(account.folders().map(f => f.name()));
"""
    jxa(script)


def notes_export(folders: list[str] | None = None) -> dict[str, Any]:
    script = f"""
const app = Application("Notes");
const want = {json.dumps(folders)};
const account = app.defaultAccount();
const out = {{folders: [], notes: [], deleted_count: null}};
account.folders().forEach(f => {{
  const name = f.name();
  out.folders.push(name);
  if (want && !want.includes(name)) return;
  f.notes().forEach(n => out.notes.push({{folder: name, name: n.name(), body: n.body(), plaintext: n.plaintext()}}));
}});
JSON.stringify(out);
"""
    return jxa(script, timeout=180)


def write_export(workspace: Path, name: str, data: Any) -> Path:
    from . import common

    path = workspace / "export" / f"{name}.json"
    common.write_json(path, data)
    return path


# ------------------------------------------------------------------ Finder

def finder_close_windows() -> None:
    """Close every Finder window (the macOS VM's Finder shows nothing else), so trials start alike."""
    jxa('const f = Application("Finder"); f.windows().forEach(w => w.close()); JSON.stringify(f.windows().length);')


# ------------------------------------------------------------------ reset verification: strays


def _n(s: object) -> str:
    return " ".join(str(s or "").split()).lower()


def calendar_strays(own: list[str], absent: list[str], purge: list[str]) -> list[str]:
    """Problems: an `own` calendar not there exactly once, an `absent` one present (names normalised as the
    checkers do), or an event titled like `purge` in any calendar."""
    data = calendar_export(None)
    problems = []
    for name in own:
        count = sum(1 for c in data["calendars"] if _n(c) == _n(name))
        if count != 1:
            problems.append(f"calendar {name!r} x{count}")
    for name in absent:
        if any(_n(c) == _n(name) for c in data["calendars"]):
            problems.append(f"calendar {name!r} present")
    want = {_n(p) for p in purge}
    mine = {_n(x) for x in own}  # the own calendars' contents are checked exactly by each task's verify
    problems += [f"stray event {e['summary']!r} in {e['calendar']!r}" for e in data["events"]
                 if _n(e.get("summary")) in want and _n(e.get("calendar")) not in mine]
    return problems


def reminders_strays(own: list[str], purge: list[str]) -> list[str]:
    data = reminders_export(None)
    want = {_n(p) for p in purge}
    problems = [f"list {name!r} x{sum(1 for l in data['lists'] if _n(l) == _n(name))}"
                for name in own if sum(1 for l in data["lists"] if _n(l) == _n(name)) != 1]
    mine = {_n(x) for x in own}
    problems += [f"stray reminder {r['name']!r} in {r['list']!r}" for r in data["reminders"]
                 if _n(r.get("name")) in want and _n(r.get("list")) not in mine]
    return problems


def notes_strays(own: list[str], purge: list[str]) -> list[str]:
    data = notes_export(None)
    want = {_n(p) for p in purge}
    problems = [f"folder {name!r} x{sum(1 for f in data['folders'] if _n(f) == _n(name))}"
                for name in own if sum(1 for f in data["folders"] if _n(f) == _n(name)) != 1]
    mine = {_n(x) for x in own}
    problems += [f"stray note {x['name']!r} in {x['folder']!r}" for x in data["notes"]
                 if _n(x.get("name")) in want and _n(x.get("folder")) not in mine]
    trash = sum(1 for x in data["notes"] if _n(x.get("folder")) == "recently deleted")
    if trash:
        problems.append(f"{trash} notes in Recently Deleted")
    return problems


def quit_saving(app: str, timeout_s: float = 10.0) -> bool:
    """Quit a document app the way a person would, saving open edits, and wait until it has exited.

    Used by export steps (after the agent, before the evaluator) so the app's own autosave timing cannot
    decide what the evaluator reads. Never raises: without Automation consent for the app, or on a hang, it
    reports to stderr and returns False; the evaluator then reads the files as they are."""
    import sys
    import time

    try:
        jxa(f'const a = Application("{app}"); if (a.running()) {{ a.quit({{saving: "yes"}}); }} JSON.stringify(true);', timeout=30)
        deadline = time.monotonic() + timeout_s
        while time.monotonic() < deadline and jxa(f'JSON.stringify(Application("{app}").running());', timeout=15):
            time.sleep(0.25)
        return True
    except Exception as error:  # noqa: BLE001
        print(f"could not quit {app} through Apple Events: {error}", file=sys.stderr)
        time.sleep(5)
        return False
