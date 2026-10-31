#!/usr/bin/env python3
"""Compute before/after CI metrics for the six-fix PR from real Actions data.

Every cell is labelled:

  measured   read out of GitHub Actions run / job / log data that exists now.
  projected  arithmetic on a measured quantity, with every assumption printed.
  n/a        not observable yet (fix not on main, or main has not run it).

The script never invents an "after" number. When a projection's inputs are
missing it prints n/a and says what is missing.

  python3 measure/fetch.py      # once: pull runs, jobs, logs
  python3 measure/measure.py    # report + markdown table
  python3 measure/measure.py --md          # table only
  python3 measure/measure.py --json out.json
"""
import argparse
import json
import re
import statistics
import sys
from datetime import datetime
from pathlib import Path

ROOT = Path(__file__).resolve().parent
DATA = ROOT / "data"
LOGS = DATA / 'logs'
TS = '%Y-%m-%dT%H:%M:%SZ'

# The repo's own ci_cache_audit.py assumes this many effective cores.
CORES = 3.4
CORES_SOURCE = ".github/scripts/ci_cache_audit.py assumes 3.4 effective cores"

# What this PR actually contains. Anything false here must not appear in the table
# as a claimed improvement.
SHIPPED = {
    'save_if_guards': True,
    'spacesd_push': False,      # already on main before this PR
    'gate_window_20s': False,   # removed: unsafe against measured fan-out
    'gate_timeout_180': True,
    'poll_cap_20s': False,      # removed: breaks the repo's API budget test
    'concurrency': False,       # removed: cancelling fails a required check
}

ANSI = re.compile(r'\x1b\[[0-9;]*[a-zA-Z]')
ENABLED_RE = re.compile(
    r'sccache enabled: family (?P<family>[^,]+), cap (?P<cap>\S+), '
    r'restored (?P<restored>\S+) in (?P<files>\d+) files \((?P<how>[^)]+)\), '
    r'this run saves: (?P<save>\w+)')
COUNTER_RE = re.compile(
    r'^(?:\S*Z\s+)?(?P<name>[A-Za-z][A-Za-z0-9 ()/+\-]*?)\s{2,}'
    r'(?P<value>\d+(?:\.\d+)?)\s*(?P<unit>%|s|GiB|MiB|KiB|B)?\s*$')


# ------------------------------------------------------------------ log parse

def parse_log(path):
    text = ANSI.sub('', Path(path).read_text(errors='replace'))
    out = {'family': None, 'restored': None, 'files': None, 'how': None,
           'save': None, 'counters': {}}
    m = ENABLED_RE.search(text)
    if m:
        out.update(family=m.group('family').strip(),
                   restored=m.group('restored'),
                   files=int(m.group('files')),
                   how=m.group('how').strip(),
                   save=m.group('save'))
    idx = text.rfind('Compile requests executed')
    if idx == -1:
        return out
    for line in text[idx:].splitlines()[:60]:
        mm = COUNTER_RE.match(line)
        if not mm:
            continue
        key = mm.group('name').strip()
        out['counters'].setdefault(key, float(mm.group('value')))
    return out


def counter(stats, name):
    return stats['counters'].get(name)


def hit_rate(stats):
    ex = counter(stats, 'Compile requests executed')
    hits = counter(stats, 'Cache hits')
    if ex and hits is not None:
        return 100.0 * hits / ex
    return None


# ------------------------------------------------------------------ run parse

def have_full_data():
    return (DATA / 'runs.json').exists() and (DATA / 'workflow_runs.json').exists()


def evidence_file():
    p = DATA / 'evidence.json'
    if not p.exists():
        sys.exit(f'missing {p}; run measure/fetch.py first')
    return json.loads(p.read_text())


def load(name):
    """Full fetched data (33 MB, git-ignored). Only used when present."""
    p = DATA / name
    if not p.exists():
        sys.exit(f'missing {p}; run measure/fetch.py first')
    return json.loads(p.read_text())


def mins(a, b):
    if not a or not b:
        return None
    try:
        return (datetime.strptime(b, TS) - datetime.strptime(a, TS)).total_seconds() / 60
    except ValueError:
        return None


def family_stats(needle):
    rows = []
    for log in sorted(LOGS.glob('*.log')):
        st = parse_log(log)
        if not st['family'] or needle not in st['family']:
            continue
        if counter(st, 'Compile requests executed') is None:
            continue
        rows.append({
            'log': log.name, 'family': st['family'], 'restored': st['restored'],
            'files': st['files'], 'how': st['how'], 'save': st['save'],
            'executed': counter(st, 'Compile requests executed'),
            'hits': counter(st, 'Cache hits'),
            'hit_rate': hit_rate(st),
            'avg_compiler': counter(st, 'Average compiler'),
        })
    return rows


def e2e_legs():
    """Every `gate / Linux E2E / Linux / shared Electron + Tauri` job we fetched."""
    if not have_full_data():
        return evidence_file()['e2e_legs']
    wf = load('workflow_runs.json')
    jobs = load('jobs.json')
    out = []
    for r in wf.get('.github/workflows/e2e-cua-driver-release-gate.yml', []):
        for j in jobs.get(str(r['id']), []):
            if not j['name'].endswith('Linux / shared Electron + Tauri'):
                continue
            d = mins(j.get('started_at'), j.get('completed_at'))
            if d:
                out.append({'job_id': j['id'], 'run_id': r['id'],
                            'head': r['head_sha'][:8], 'minutes': d,
                            'conclusion': j.get('conclusion')})
    return out


GATE_EVIDENCE = {}


def gate_runs():
    if not have_full_data():
        g = evidence_file()['gate']
        # The extract keeps summary statistics, not every raw duration; carry the
        # median and max so the report can still quote the worst observed gate.
        GATE_EVIDENCE.update(g)
        return [g['median'], g['max']] if g else []
    wf = load('workflow_runs.json')
    out = []
    for r in wf.get('.github/workflows/ci-gate.yml', []):
        d = mins(r.get('created_at'), r.get('updated_at'))
        if d is not None:
            out.append(d)
    return sorted(out)


def pr_spans():
    """Wall-clock per PR head: earliest run created -> latest run updated."""
    if not have_full_data():
        return evidence_file()['pr_spans']
    runs = load('runs.json')
    heads = {}
    for r in runs:
        if r.get('event') != 'pull_request':
            continue
        # A queued run has updated_at == created_at, which would read as a
        # zero-minute PR. Only completed runs contribute a real span.
        if not r.get('conclusion') or r['conclusion'] in ('action_required',):
            continue
        try:
            a = datetime.strptime(r['created_at'], TS)
            b = datetime.strptime(r['updated_at'], TS)
        except (KeyError, ValueError):
            continue
        h = heads.setdefault(r['head_sha'], {
            'start': a, 'end': b, 'names': set(),
            'branch': r.get('head_branch') or ''})
        h['start'] = min(h['start'], a)
        h['end'] = max(h['end'], b)
        h['names'].add(r.get('name') or '')
    out = []
    for sha, h in heads.items():
        out.append({'head': sha[:8], 'branch': h['branch'],
                    'minutes': (h['end'] - h['start']).total_seconds() / 60,
                    'workflows': len(h['names'])})
    return out


def duplicate_waste(name_contains):
    """Job-minutes spent in runs that overlapped an earlier run of the same head."""
    if not have_full_data():
        return evidence_file()['duplicates_release_metadata']
    runs = load('runs.json')
    heads = {}
    for r in runs:
        if name_contains.lower() not in (r.get('name') or '').lower():
            continue
        try:
            a = datetime.strptime(r['created_at'], TS)
            b = datetime.strptime(r['updated_at'], TS)
        except (KeyError, ValueError):
            continue
        heads.setdefault(r['head_sha'], []).append((a, b))
    dups, waste = 0, 0.0
    for v in heads.values():
        v.sort()
        for i in range(1, len(v)):
            if v[i][0] < max(x[1] for x in v[:i]):
                dups += 1
                waste += (v[i][1] - v[i][0]).total_seconds() / 60
    return {'heads': len(heads), 'duplicate_runs': dups,
            'wasted_job_minutes': waste}


# ------------------------------------------------------------------ report

def reduction(before, after):
    if not before or after is None:
        return None
    return 100.0 * (before - after) / before


def build():
    rep = {'rows': [], 'evidence': {}, 'assumptions': {
        'cores': CORES, 'cores_source': CORES_SOURCE}}

    drv = family_stats('libs-cua-driver-rust')
    spd = family_stats('libs-cua-spacesd')
    rep['evidence']['driver_rust'] = drv
    rep['evidence']['spacesd'] = spd

    legs = e2e_legs()
    rep['evidence']['e2e_legs'] = legs
    leg_med = statistics.median([x['minutes'] for x in legs]) if legs else None

    gates = gate_runs()
    rep['evidence']['gate_runs'] = {'n': len(gates),
                                    'median': statistics.median(gates) if gates else None,
                                    'max': max(gates) if gates else None}

    spans = pr_spans()
    heavy = sorted([s for s in spans if re.match(r'^(salvage-|refactor/|feat/|fix/|upstream/)', s['branch'])],
                   key=lambda x: x['minutes'])
    fast = sorted([s for s in spans if re.match(r'^(dependabot|renovate)/', s['branch'])],
                  key=lambda x: x['minutes'])
    rep['evidence']['heavy_prs'] = heavy[-3:]
    rep['evidence']['fast_prs'] = fast[-3:] or fast

    dup = duplicate_waste('Release metadata')
    rep['evidence']['release_metadata_duplicates'] = dup
    rep['evidence']['shipped'] = SHIPPED

    def row(metric, before, after, lb, la, note, ships=False):
        rep['rows'].append({'metric': metric, 'before': before, 'after': after,
                            'reduction_pct': reduction(before, after),
                            'label_before': lb, 'label_after': la, 'note': note,
                            'in_this_pr': ships})

    # --- cache hit rates (measured before; after needs a main run)
    if drv:
        row('sccache hit rate, libs-cua-driver-rust (%)',
            statistics.median([r['hit_rate'] for r in drv]), None, 'measured', 'n/a',
            'restores an exact hit and still misses, so the miss is not a scope problem; '
            'guards fixed here stop PR-scoped writes but cannot move this number alone')
    if spd:
        row('sccache hit rate, libs-cua-spacesd (%)',
            statistics.median([r['hit_rate'] for r in spd]), None, 'measured', 'n/a',
            'already seeded by main: push trigger present since before this PR')

    # --- E2E leg (measured before, projected after)
    proj = None
    src = None
    for r in drv:
        if 'exact hit' in (r['how'] or '') and r['avg_compiler']:
            src = r
            break
    if src:
        misses = src['executed'] - src['hits']
        work = misses * src['avg_compiler']
        proj = {'misses': misses, 'avg_compiler': src['avg_compiler'],
                'compiler_work_s': work, 'wall_min': work / CORES / 60}
        rep['evidence']['e2e_projection'] = proj
    if leg_med is not None and proj:
        row('gate / Linux E2E / shared Electron + Tauri (min)',
            leg_med, leg_med - proj['wall_min'], 'measured', 'projected',
            f"ceiling if this family cached: {proj['misses']:.0f} misses x "
            f"{proj['avg_compiler']:.3f}s / {CORES} cores = {proj['wall_min']:.1f} min. "
            'NOT attributable to this PR: the miss is a profile/input mismatch')

    # --- what this PR does change
    if gates:
        gmed = statistics.median(gates)
        row('ci-gate job duration (min)', gmed, gmed, 'measured', 'measured',
            'poll schedule deliberately unchanged (see BEFORE-AFTER.md)', ships=False)
        row('ci-gate timeout cap (min)', 360, 180, 'measured', 'measured',
            f'caps a wedged gate at 3h instead of 6h; worst gate observed {max(gates):.1f} min',
            ships=True)

    if heavy:
        h = statistics.median([x['minutes'] for x in heavy])
        row('heavy PR wall-clock, median (min)', h, h, 'measured', 'unchanged',
            'untouched by this PR; context for scale')
    if fast:
        fmed = statistics.median([x['minutes'] for x in fast])
        row('dependabot PR wall-clock, median (min)', fmed, fmed, 'measured', 'unchanged',
            'dominated by ci-gate; this PR leaves the wait window alone')
    return rep


def markdown(rep):
    w = ['| metric | before | after | reduction | label | note |',
         '|---|---|---|---|---|---|']
    for r in rep['rows']:
        b = '—' if r['before'] is None else f"{r['before']:.2f}"
        a = '—' if r['after'] is None else f"{r['after']:.2f}"
        d = '—' if r['reduction_pct'] is None else f"{r['reduction_pct']:.1f}%"
        lab = f"{r['label_before']} / {r['label_after']}"
        w.append(f"| {r['metric']} | {b} | {a} | {d} | {lab} | {r['note']} |")
    return '\n'.join(w)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--md', action='store_true')
    ap.add_argument('--json')
    args = ap.parse_args()
    rep = build()
    if args.json:
        Path(args.json).write_text(json.dumps(rep, indent=2, default=str))
    print(markdown(rep))
    if not args.md:
        print('\n--- assumptions ---')
        for k, v in rep['assumptions'].items():
            print(f'  {k}: {v}')
        print('\n--- evidence ---')
        print(json.dumps(rep['evidence'], indent=2, default=str))


if __name__ == '__main__':
    main()
