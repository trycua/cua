"""Owned-fixture experiment: literal caller-owned subtask, not general planning."""
import json,os,sys,time,subprocess
from pathlib import Path
from contextlib import ExitStack
HERE=Path(__file__).resolve().parent
BASE=Path(os.environ['OH_COMPARISON_SOURCE'])/'experiments/arc-cua-comparison-2026-10-05'
sys.path[:0]=[str(BASE),str(BASE/'jev-matched'),str(BASE/'decision-layer')]
from run import MCP,Fixture,windows,wait_for,candidate_versions,verify_servers,observe,find,act
from run_matched import TimedClient,trial
from arc_cua.policies import TypeSafeJevPolicy
STEPS=(('Full name','SET_VALUE','Synthetic Person','name'),('Email','SET_VALUE','synthetic@example.invalid','email'),('Subscribe','CLICK',None,'subscribe'))

def step(client,fixture,window,item):
    label,kind,value,field=item
    snapshot=observe(client,fixture,window)
    target=find(client,snapshot,label)
    if snapshot.get('pid')!=fixture.pid or snapshot.get('window_id')!=window:raise RuntimeError('Owner mismatch')
    if target.get('enabled') is False or target.get('visible') is False:raise RuntimeError('Target unavailable')
    # Check current desired checkbox state, not a blind toggle.
    if field=='subscribe' and str(target.get('value')) not in ('0','False','false'):raise RuntimeError('Checkbox state ambiguous or already checked')
    response,error=act(client,fixture,window,snapshot,target,kind,value)
    if error or (client.name=='arc' and response.get('status')!='done'):raise RuntimeError('Bound action refused; do not retry input')
    expected=True if field=='subscribe' else value
    state=wait_for(lambda:fixture.state() if fixture.state().get(field)==expected else None,3)
    if state.get('submitted')!=0:raise RuntimeError('Forbidden submission')
    return {'field':field,'verified':True}

def deterministic(client,mode,rep):
    fixture=None;offset=len(client.calls);start=time.perf_counter();row={'driver':client.name,'mode':mode,'rep':rep,'caller_invocations':0,'decision_provider_calls':0,'verified_steps':[]}
    try:
        fixture=Fixture();window=wait_for(lambda:windows(fixture.pid).get('Arc Bench Form'),8)
        start=time.perf_counter()
        if mode=='bounded':
            row['caller_invocations']+=1
            for item in STEPS:row['verified_steps'].append(step(client,fixture,window,item))
        else:
            for item in STEPS:
                row['caller_invocations']+=1
                row['verified_steps'].append(step(client,fixture,window,item))
        state=fixture.state()
        row['passed']=all(state.get(k)==v for k,v in {'name':'Synthetic Person','email':'synthetic@example.invalid','subscribe':True,'submitted':0}.items())
        row['terminal']='verified_complete' if row['passed'] else 'handoff'
    except Exception as exc:row.update(passed=False,terminal='handoff',error_type=type(exc).__name__)
    finally:
        row['wall_ms']=(time.perf_counter()-start)*1000
        row['calls']=client.calls[offset:];row['driver_tool_ms']=sum(c['ms'] for c in row['calls']);row['driver_calls']=len(row['calls'])
        if fixture:
            row['oracle_state']=fixture.state()
            try:
                if client.name=='arc':client.call('release',pid=fixture.pid)
            finally:fixture.close()
    row['false_completion']=row['terminal']=='verified_complete' and not row['passed']
    return row

def main():
    versions=candidate_versions();tag=versions['native_release'].rsplit('/',1)[-1]
    versions['native_release_commit']=subprocess.check_output(['gh','api',f'repos/trycua/cua/git/ref/tags/{tag}','--jq','.object.sha'],text=True).strip()
    key=os.environ.get('TYPESAFE_API_KEY')
    if not key:
        argv=json.loads(os.environ['JEV_CREDENTIAL_COMMAND'])
        if not isinstance(argv,list) or not argv or not all(isinstance(arg,str) for arg in argv):raise RuntimeError('Invalid external credential command')
        secret=subprocess.run(argv,capture_output=True,text=True,timeout=30)
        if secret.returncode or not secret.stdout.strip():raise RuntimeError('Runtime credential unavailable')
        key=secret.stdout.strip();provider=TimedClient()
    output={**versions,'scope':'literal form subtask: deterministic boundary ablation and real JEV negative control','results':[]}
    try:
        policy=TypeSafeJevPolicy(api_key=key,model='jev-latest',client=provider)
        with ExitStack() as stack:
            arc=MCP([sys.executable,'-m','arc_cua','mcp'],'bounded-arc');stack.callback(arc.close)
            cua=MCP(['cua-driver','mcp','--socket',str(Path.home()/'Library/Caches/cua-driver/cua-driver.sock')],'bounded-cua');stack.callback(cua.close)
            verify_servers(versions,arc,cua);output['running_servers']={'arc':arc.server_info,'cua':cua.server_info}
            for rep in range(3):
                for mode in (('stepwise','bounded','jev') if rep%2==0 else ('jev','bounded','stepwise')):
                    if mode=='jev':
                        row=trial(cua,'cua','native_form',rep,policy);row.update(mode='jev',decision_provider_calls=len(row['provider_requests']),caller_invocations=1)
                    else:row=deterministic(cua,mode,rep)
                    output['results'].append(row)
                    encoded=json.dumps(output,indent=2)
                    if key in encoded:raise RuntimeError('Secret persistence refused')
                    (HERE/'results.json').write_text(encoded+'\n')
                    print(json.dumps({k:row.get(k) for k in ('mode','rep','passed','false_completion','wall_ms','driver_calls','decision_provider_calls')}),flush=True)
    finally:provider.close()
if __name__=='__main__':main()
