"""Derive qualified counts from immutable local traces; no driver/provider calls."""
import gzip,hashlib,json,statistics
from pathlib import Path
HERE=Path(__file__).resolve().parent
EXPECTED={'name':'Synthetic Person','email':'synthetic@example.invalid','subscribe':False,'submitted':0,'record':'Record A','dialog':False}

def total(values):
    values=list(values)
    return None if any(type(v) is not int for v in values) else sum(values)

def median(values):
    values=list(values)
    return statistics.median(values) if values else None

def canonical(calls):
    count=0
    for call in calls:
        if call['tool']!='experiment_action_observe': count+=1;continue
        result=call.get('result',{})
        dispatches=result.get('child_dispatches',result.get('operation',{}).get('child_dispatches'))
        if dispatches not in (['set_value'],['set_value','get_window_state']): return None
        count+=len(dispatches)
    return count

def rows(data):
    output=[]
    for row in data['results']:
        oracle=all(row.get('oracle_state',{}).get(k)==v for k,v in EXPECTED.items())
        strict=row.get('terminal')=='verified_complete' and row.get('passed') is True and row.get('oracle_passed') is True and oracle
        frontier=row.get('frontier') or {};usage=frontier.get('usage',[]);jev=row.get('jev',[])
        output.append({'mode':row['mode'],'rep':row['rep'],'smoke':row['smoke'],'terminal':row['terminal'],'strict_pass':strict,'independent_oracle_pass':oracle,'false_completion':row['terminal']=='verified_complete' and not oracle,'wall_ms':row['wall_ms'],'mcp_wall_ms':row.get('mcp_wall_ms'),
            'visible_mcp_calls':row.get('visible_mcp_calls'),'canonical_child_calls':canonical(row['calls']),'raw_canonical_child_calls':row.get('canonical_child_calls'),
            'frontier_ms':frontier.get('ms',0),'frontier_attempts':int(bool(frontier)),'frontier_completed_turns':frontier.get('turns',0),
            'frontier_input_tokens':total(u.get('input_tokens') for u in usage),'frontier_cached_input_tokens':total(u.get('cached_input_tokens') for u in usage),'frontier_output_tokens':total(u.get('output_tokens') for u in usage),'frontier_reasoning_tokens':total(u.get('reasoning_output_tokens') for u in usage),
            'jev_decision_calls':len(jev),'jev_http_calls':len(row.get('jev_http',[])),'jev_ms':sum(j['ms'] for j in jev),'jev_input_tokens':total(j['usage'].get('input_tokens') for j in jev),'jev_output_tokens':total(j['usage'].get('output_tokens') for j in jev)})
    return output

def summarize(data):
    derived=rows(data);groups={}
    for mode in dict.fromkeys(row['mode'] for row in derived):
        subset=[r for r in derived if r['mode']==mode];success=[r for r in subset if r['strict_pass']]
        groups[mode]={'trials':len(subset),'strict_passes':len(success),'oracle_passes':sum(r['independent_oracle_pass'] for r in subset),'false_completions':sum(r['false_completion'] for r in subset),'success_median_wall_ms':median(r['wall_ms'] for r in success),'all_median_wall_ms':median(r['wall_ms'] for r in subset),
            'median_mcp_wall_ms':median(r['mcp_wall_ms'] for r in success),'median_frontier_ms':median(r['frontier_ms'] for r in success),'median_jev_ms':median(r['jev_ms'] for r in success),
            'visible_mcp_calls':sorted({r['visible_mcp_calls'] for r in subset}),'canonical_child_calls':sorted({r['canonical_child_calls'] for r in subset},key=lambda v:-1 if v is None else v)}
        for field in ('frontier_attempts','frontier_completed_turns','frontier_input_tokens','frontier_cached_input_tokens','frontier_output_tokens','frontier_reasoning_tokens','jev_decision_calls','jev_http_calls','jev_input_tokens','jev_output_tokens'):
            groups[mode][field]=total(r[field] for r in subset)
    pairs=[]
    for rep in sorted({r['rep'] for r in derived}):
        bymode={r['mode']:r for r in derived if r['rep']==rep}
        for baseline,candidate in [('frontier_normal','frontier_literal'),('chooser_only','frontier_literal'),('frontier_literal','frontier_literal_composite'),('frontier_normal','frontier_literal_composite'),('chooser_only','frontier_literal_composite')]:
            if baseline in bymode and candidate in bymode:
                b,c=bymode[baseline],bymode[candidate];both=b['strict_pass'] and c['strict_pass']
                pairs.append({'rep':rep,'baseline':baseline,'candidate':candidate,'both_strict_pass':both,'candidate_minus_baseline_ms':c['wall_ms']-b['wall_ms'] if both else None})
    return {'schema':'cua.literal_normal_qualification_summary_v1','strict_criterion':'terminal verified_complete AND raw passed AND raw oracle_passed AND independently recomputed fixture oracle','canonical_count_provenance':'derive exact child_dispatches list from retained operation response; initial raw composite counters were null because runner expected nested telemetry','groups':groups,'pairs':pairs,'rows':derived}

if __name__=='__main__':
    for name in ('smoke-results','results'):
        path=HERE/(name+'.json')
        if path.exists() or path.with_suffix('.json.gz').exists():
            raw=path.read_bytes() if path.exists() else gzip.decompress(path.with_suffix('.json.gz').read_bytes())
            summary=summarize(json.loads(raw));summary['raw_sha256']=hashlib.sha256(raw).hexdigest()
            (HERE/(name+'-summary.json')).write_text(json.dumps(summary,indent=2)+'\n')
            print(json.dumps({'file':name,'groups':summary['groups']},indent=2))
