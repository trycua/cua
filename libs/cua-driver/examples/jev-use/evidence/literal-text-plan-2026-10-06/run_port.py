"""Actual Cua-native caller recipe, same two-step executor with/without real JEV."""
import asyncio,json,os,sys,time,subprocess,hashlib
from pathlib import Path
from contextlib import ExitStack
HERE=Path(__file__).resolve().parent;BASE=Path(os.environ['REFERENCE_FIXTURE_SOURCE'])
sys.path[:0]=[str(BASE),str(BASE/'jev-matched')]
from run import MCP,Fixture,windows,wait_for,candidate_versions,verify_servers
from run_matched import TimedClient
sys.path.insert(0,str(Path(os.environ['CUA_LITERAL_SOURCE'])/'libs/cua-driver/examples/jev-use/python'))
from literal_text_plan import LiteralTextStep,execute_literal_text_plan
import importlib, os
_reference_module = importlib.import_module(os.environ['REFERENCE_MODULE'] + '')
Subtask = getattr(_reference_module, 'Subtask')
import importlib, os
_reference_module = importlib.import_module(os.environ['REFERENCE_MODULE'] + '.models')
ActionKind = getattr(_reference_module, 'ActionKind')
ExecutableAction = getattr(_reference_module, 'ExecutableAction')
DesktopElement = getattr(_reference_module, 'DesktopElement')
DesktopSnapshot = getattr(_reference_module, 'DesktopSnapshot')
import importlib, os
_reference_module = importlib.import_module(os.environ['REFERENCE_MODULE'] + '.validation')
materialize_action = getattr(_reference_module, 'materialize_action')
import importlib, os
_reference_module = importlib.import_module(os.environ['REFERENCE_MODULE'] + '.policies')
TypeSafeJevPolicy = getattr(_reference_module, 'TypeSafeJevPolicy')
from native import eligible_controls
STEPS=(LiteralTextStep('Full name','Synthetic Person'),LiteralTextStep('Email','synthetic@example.invalid'))

class PublicDriver:
    def __init__(self,client):self.client=client
    async def call(self,name,args):
        result,error=await asyncio.to_thread(self.client.call,name,**args)
        if error or result.get('status')=='refused' or result.get('refusal'):raise RuntimeError('Public driver refusal')
        return result

async def trial(client,policy,rep):
    fixture=None;offset=len(client.calls);provider=policy.transport.client if policy else None;request_offset=len(provider.requests) if provider else 0
    row={'mode':'literal_with_jev' if policy else 'literal_without_jev','rep':rep,'passed':False,'terminal':'handoff','decisions':[]};started=time.perf_counter()
    try:
        fixture=Fixture();window=wait_for(lambda:windows(fixture.pid).get('Reference Bench Form'),8)
        def final_state():return all(fixture.state().get(k)==v for k,v in {'name':'Synthetic Person','email':'synthetic@example.invalid','subscribe':False,'submitted':0,'record':'Record A'}.items())
        async def verify_final():return final_state()
        async def verify_step(step):
            field={'Full name':'name','Email':'email'}[step.label]
            try:await asyncio.to_thread(wait_for,lambda:fixture.state().get(field)==step.value,3)
            except TimeoutError:return False
            return fixture.state().get('submitted')==0 and fixture.state().get('subscribe') is False
        def context_guard(observation):
            state=fixture.state()
            return state.get('record')=='Record A' and state.get('submitted')==0 and state.get('dialog') is False
        async def choose(candidate,observation):
            controls=[e for e in eligible_controls(observation,'macos').controls if e.id==candidate.id]
            if len(controls)!=1:return 'abstain'
            target=controls[0]
            element=DesktopElement(id=target.id,role='TextField',name=target.label,value=target.value,actions=(ActionKind.SET_VALUE,))
            snapshot=DesktopSnapshot(application='Owned fixture',window='Reference Bench Form',revision=observation.snapshot_id,elements=(element,))
            task=Subtask(goal=f'Set {target.label} to the supplied value by performing the single offered SET_VALUE action.',inputs={'value':candidate.arguments['value']},constraints=('Do not click any other control or submit.',),verification=(f'{target.label} equals the supplied value',),max_actions=1)
            decision=await asyncio.to_thread(policy.decide,subtask=task,snapshot=snapshot,history=[])
            row['decisions'].append({'label':target.label,'kind':decision.kind.value if decision.kind else None,'terminal':decision.terminal.value if decision.terminal else None,'confidence':decision.confidence})
            if decision.kind!=ActionKind.SET_VALUE or decision.target_id!=target.id:return 'abstain'
            action=materialize_action(decision,snapshot,task)
            expected=ExecutableAction(kind=ActionKind.SET_VALUE,target_id=target.id,target_guard=element.semantic_guard(),value=candidate.arguments['value'])
            return candidate.id if action==expected else 'abstain'
        started=time.perf_counter()
        result=await execute_literal_text_plan(PublicDriver(client),pid=fixture.pid,window_id=window,platform='macos',steps=STEPS,verify_step=verify_step,verify_final=verify_final,verify_context=context_guard,choose=choose if policy else None)
        row.update(terminal=result['status'],steps=result['steps'],passed=final_state())
    except Exception as exc:row['error_type']=type(exc).__name__
    finally:
        row['wall_ms']=(time.perf_counter()-started)*1000;row['calls']=client.calls[offset:];row['driver_calls']=len(row['calls']);row['driver_tool_ms']=sum(c['ms'] for c in row['calls']);row['provider_requests']=provider.requests[request_offset:] if provider else [];row['provider_calls']=len(row['provider_requests']);row['provider_ms']=sum(r['ms'] for r in row['provider_requests'])
        if fixture:
            try:row['oracle_state']=fixture.state()
            finally:fixture.close()
    row['false_completion']=row['terminal']=='verified_complete' and not row['passed'];return row

def main():
    versions=candidate_versions();tag=versions['native_release'].rsplit('/',1)[-1];versions['native_release_commit']=subprocess.check_output(['gh','api',f'repos/trycua/cua/git/ref/tags/{tag}','--jq','.object.sha'],text=True).strip();source=Path(os.environ['CUA_LITERAL_SOURCE']);versions['recipe_source']=subprocess.check_output(['git','-C',str(source),'rev-parse','HEAD'],text=True).strip();versions['recipe_file_sha256']=hashlib.sha256((source/'libs/cua-driver/examples/jev-use/python/literal_text_plan.py').read_bytes()).hexdigest();versions['recipe_patch']=subprocess.check_output(['git','-C',str(source),'diff','--','libs/cua-driver/examples/jev-use'],text=True)
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
            reference=MCP([sys.executable,'-m',os.environ['REFERENCE_MODULE'],'mcp'],'literal-port-reference');stack.callback(reference.close)
            cua=MCP(['cua-driver','mcp','--socket',str(Path.home()/'Library/Caches/cua-driver/cua-driver.sock')],'literal-port-cua');stack.callback(cua.close)
            verify_servers(versions,reference,cua);output['running_servers']={'reference':reference.server_info,'cua':cua.server_info}
            for rep in range(3):
                for candidate in ((policy,None) if rep%2==0 else (None,policy)):
                    row=asyncio.run(trial(cua,candidate,rep));output['results'].append(row);models={r['reported_model'] for item in output['results'] for r in item['provider_requests']}
                    if len(models)>1:raise RuntimeError('Provider version changed')
                    output['resolved_models']=sorted(models);encoded=json.dumps(output,indent=2)
                    if key in encoded:raise RuntimeError('Secret persistence refused')
                    (HERE/'port-results.json').write_text(encoded+'\n');print(json.dumps({k:row[k] for k in ('mode','rep','passed','false_completion','wall_ms','provider_calls','driver_calls')}),flush=True)
    finally:provider.close()
if __name__=='__main__':main()
