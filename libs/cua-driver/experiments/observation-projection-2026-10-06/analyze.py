"""Recompute retained result medians; never treat a failed task as a speed win."""
import gzip,json,statistics
from pathlib import Path
HERE=Path(__file__).resolve().parent

def load(name):
    path=HERE/name
    return json.loads(path.read_text()) if path.exists() else json.loads(gzip.decompress((HERE/(name+'.gz')).read_bytes()))

def observations(data):
    rows=[r for r in data['results'] if not r['warmup']]
    if not all(r['qualified'] and r.get('pair_semantics_equal') and r.get('all_arm_semantics_equal') for r in rows):raise ValueError('Unqualified observation matrix')
    groups=sorted({(r['case'],r['arm'],r['mode']) for r in rows})
    return [{**dict(zip(('case','arm','mode'),group)),'n':len(selected),**{key:statistics.median(r[key] for r in selected) for key in ('wall_ms','serialized_result_bytes','image_bytes','model_json_bytes','public_calls')}} for group in groups for selected in [[r for r in rows if (r['case'],r['arm'],r['mode'])==group]]]

def tasks(data):
    rows=data['results'];groups=[]
    for case in ('small','large'):
        selected=[r for r in rows if r['case']==case]
        pairs=[]
        for rep in sorted({r['rep'] for r in selected}):
            pair={r['arm']:r for r in selected if r['rep']==rep}
            if set(pair)!={'baseline','broad'}:continue
            if not all(r['strict_pass'] and r['terminal']=='verified_complete' and r.get('final_values_verified') and r.get('oracle_passed') for r in pair.values()):continue
            pairs.append(pair['broad']['wall_ms']-pair['baseline']['wall_ms'])
        arms={}
        for arm in ('baseline','broad'):
            succeeded=[r for r in selected if r['arm']==arm and r['strict_pass']]
            arms[arm]={'strict_success':len(succeeded),'attempted':sum(r['arm']==arm for r in selected),**{key:statistics.median(r[key] for r in succeeded) if succeeded else None for key in ('wall_ms','mcp_ms','serialized_result_bytes','image_bytes','public_calls')}}
        groups.append({'case':case,'arms':arms,'successful_pairs':len(pairs),'combined_faster_pairs':sum(d<0 for d in pairs),'paired_delta_ms':pairs,'median_paired_delta_ms':statistics.median(pairs) if pairs else None})
    controls=[{'case':r['case'],'arm':r['arm'],'terminal':r['terminal'],'handoff_before_input':r['terminal']=='handoff' and not any(c['tool']=='set_value' for c in r['calls']),'error_type':r.get('error_type'),'error':r.get('error')} for r in data.get('controls',[])]
    if not all(r['handoff_before_input'] for r in controls):raise ValueError('Wrong-record control entered input')
    return {'tasks':groups,'false_completion':sum(r['false_completion'] for r in rows),'controls':controls}
if __name__=='__main__':
    out={'observations':observations(load('observation-results.json'))}
    if (HERE/'task-results.json').exists() or (HERE/'task-results.json.gz').exists():out.update(tasks(load('task-results.json')))
    (HERE/'summary.json').write_text(json.dumps(out,indent=2)+'\n');print(json.dumps(out,indent=2))
