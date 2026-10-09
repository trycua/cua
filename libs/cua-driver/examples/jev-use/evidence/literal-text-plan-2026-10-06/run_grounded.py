"""Same grounded literal plan, same executor/oracles; with vs without real JEV."""
import json,os,sys,time,subprocess
from pathlib import Path
from contextlib import ExitStack
HERE=Path(__file__).resolve().parent
BASE=Path(os.environ['REFERENCE_FIXTURE_SOURCE'])
sys.path[:0]=[str(BASE),str(BASE/'jev-matched'),str(BASE/'decision-layer')]
from run import MCP,Fixture,windows,wait_for,candidate_versions,verify_servers
from run_matched import TimedClient
from mcp_backend import MCPBackend
import importlib, os
_reference_module = importlib.import_module(os.environ['REFERENCE_MODULE'] + '.policies')
TypeSafeJevPolicy = getattr(_reference_module, 'TypeSafeJevPolicy')
import importlib, os
_reference_module = importlib.import_module(os.environ['REFERENCE_MODULE'] + '.models')
ActionKind = getattr(_reference_module, 'ActionKind')
from bounded import LiteralStep,perform
ALL_STEPS=(LiteralStep('Full name','TextField',ActionKind.SET_VALUE,'Synthetic Person'),LiteralStep('Email','TextField',ActionKind.SET_VALUE,'synthetic@example.invalid'),LiteralStep('Subscribe','CheckBox',ActionKind.CLICK))

STEPS=ALL_STEPS[:2] if os.environ.get('BOUNDED_TEXT_ONLY')=='1' else ALL_STEPS
OUT='grounded-text-results.json' if len(STEPS)==2 else 'grounded-results.json'

def run(client,policy,rep):
    fixture=None;offset=len(client.calls);provider=policy.transport.client if policy else None;request_offset=len(provider.requests) if provider else 0
    row={'decisions':[],'mode':'grounded_jev' if policy else 'grounded_literal','rep':rep,'passed':False,'terminal':'handoff'};started=time.perf_counter()
    try:
        fixture=Fixture();window=wait_for(lambda:windows(fixture.pid).get('Reference Bench Form'),8);backend=MCPBackend(client,'cua',fixture.pid,window)
        def verify_all():
            state=fixture.state()
            return all(state.get(k)==v for k,v in {'name':'Synthetic Person','email':'synthetic@example.invalid','subscribe':len(STEPS)==3,'submitted':0}.items())
        def verify_step(step):
            field={'Full name':'name','Email':'email','Subscribe':'subscribe'}[step.label];expected=True if field=='subscribe' else step.value
            try:wait_for(lambda:fixture.state().get(field)==expected,3)
            except TimeoutError:return False
            return fixture.state().get('submitted')==0
        started=time.perf_counter();row.update(perform(backend,STEPS,verify_step,verify_all,policy=policy,decision_log=row['decisions']));row['passed']=verify_all()
    except Exception as exc:row['error_type']=type(exc).__name__;row['error']=str(exc) if type(exc).__name__=='Handoff' else None
    finally:
        row['wall_ms']=(time.perf_counter()-started)*1000;row['calls']=client.calls[offset:];row['driver_calls']=len(row['calls']);row['driver_tool_ms']=sum(c['ms'] for c in row['calls'])
        row['provider_requests']=provider.requests[request_offset:] if provider else [];row['decision_provider_calls']=len(row['provider_requests']);row['provider_ms']=sum(r['ms'] for r in row['provider_requests'])
        if fixture:
            try:row['oracle_state']=fixture.state()
            finally:fixture.close()
    row['false_completion']=row['terminal']=='verified_complete' and not row['passed'];return row

def main():
    versions=candidate_versions();tag=versions['native_release'].rsplit('/',1)[-1];versions['native_release_commit']=subprocess.check_output(['gh','api',f'repos/trycua/cua/git/ref/tags/{tag}','--jq','.object.sha'],text=True).strip()
    key=os.environ.get('TYPESAFE_API_KEY')
    if not key:
        argv=json.loads(os.environ['JEV_CREDENTIAL_COMMAND'])
        if not isinstance(argv,list) or not argv or not all(isinstance(arg,str) for arg in argv):raise RuntimeError('Invalid external credential command')
        secret=subprocess.run(argv,capture_output=True,text=True,timeout=30)
        if secret.returncode or not secret.stdout.strip():raise RuntimeError('Runtime credential unavailable')
        key=secret.stdout.strip();provider=TimedClient();output={**versions,'results':[]}
    try:
        policy=TypeSafeJevPolicy(api_key=key,model='jev-latest',client=provider)
        with ExitStack() as stack:
            reference=MCP([sys.executable,'-m',os.environ['REFERENCE_MODULE'],'mcp'],'grounded-reference');stack.callback(reference.close)
            cua=MCP(['cua-driver','mcp','--socket',str(Path.home()/'Library/Caches/cua-driver/cua-driver.sock')],'grounded-cua');stack.callback(cua.close)
            verify_servers(versions,reference,cua);output['running_servers']={'reference':reference.server_info,'cua':cua.server_info}
            for rep in range(3):
                for candidate in ((policy,None) if rep%2==0 else (None,policy)):
                    row=run(cua,candidate,rep);output['results'].append(row)
                    models={r['reported_model'] for item in output['results'] for r in item['provider_requests']}
                    if len(models)>1:raise RuntimeError('Provider version changed')
                    output['resolved_models']=sorted(models)
                    encoded=json.dumps(output,indent=2)
                    if key in encoded:raise RuntimeError('Secret persistence refused')
                    (HERE/OUT).write_text(encoded+'\n');print(json.dumps({k:row[k] for k in ('mode','rep','passed','false_completion','wall_ms','decision_provider_calls')}),flush=True)
    finally:provider.close()
if __name__=='__main__':main()
