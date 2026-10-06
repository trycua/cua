"""Owned synthetic fixture: normal Cua JEV choices versus validated frontier plan.

External config: OH_COMPARISON_SOURCE, ARC_EVAL_SOURCE, CUA_LITERAL_SOURCE,
JEV_CREDENTIAL_COMMAND (JSON argv) or TYPESAFE_API_KEY. No user apps.
"""
import asyncio, hashlib, importlib.util, json, os, subprocess, sys, time
from contextlib import ExitStack
from pathlib import Path
HERE = Path(__file__).resolve().parent
CUA = Path(os.environ['CUA_LITERAL_SOURCE'])
BASE = Path(os.environ['OH_COMPARISON_SOURCE']) / 'experiments/arc-cua-comparison-2026-10-05'
spec = importlib.util.spec_from_file_location('owned_fixture_harness', BASE/'run.py')
h = importlib.util.module_from_spec(spec); sys.modules[spec.name] = h; spec.loader.exec_module(h)
sys.path.insert(0, str(CUA/'libs/cua-driver/examples/jev-use/python'))
from literal_text_plan import LiteralTextStep, execute_literal_text_plan, validate_literal_text_plan, LiteralPlanHandoff
from native import NativeObservation
from sources import NativeAccessibilitySource
from native_tasks import NativeTask, WindowScope, TaskStep, native_choice_request
from tasks import TaskParameter, TaskSources
from choose_action import validate_request
from decision_models import DecisionRequest, TypeSafeDecisionModel, choose
from composite_driver import CompositeTextDriver
from typesafe_sdk import TypeSafeClient
import httpx2
STEPS = (LiteralTextStep('Full name','Synthetic Person'), LiteralTextStep('Email','synthetic@example.invalid'))
GOAL = 'First set Full name to Synthetic Person; then set Email to synthetic@example.invalid. Leave Subscribe unchecked, Record A selected, and do not submit or change any other field.'

class MetadataMCP(h.MCP):
    def request(self, method, params):
        result=super().request(method,params)
        if method=='initialize': self.backend_metadata=result.get('_meta',{}).get('driver_metadata')
        return result

def safe_evidence(value):
    if isinstance(value,list): return [safe_evidence(v) for v in value]
    if isinstance(value,dict):
        cleaned=h.sanitize_result(value)
        # Child MCP content duplicates its full structured payload, including pixels.
        return {k:safe_evidence(v) for k,v in cleaned.items() if not (k=='content' and isinstance(value.get('structuredContent'),dict))}
    return value

class PublicDriver:
    def __init__(self, client): self.client = client
    async def call(self, name, arguments):
        result, error = await asyncio.to_thread(self.client.call, name, **arguments)
        if error or result.get('status') == 'refused' or result.get('refusal'):
            raise RuntimeError('Public Driver refused')
        return result

class TimedHTTP(httpx2.Client):
    def __init__(self): super().__init__(timeout=25); self.events=[]
    def send(self, *args, **kwargs):
        start=time.perf_counter()
        try:
            response=super().send(*args, **kwargs)
            self.events.append({'ms':(time.perf_counter()-start)*1000,'status':response.status_code})
            return response
        except Exception as exc:
            self.events.append({'ms':(time.perf_counter()-start)*1000,'error_type':type(exc).__name__}); raise

class RecordingSDK:
    def __init__(self, client): self.client=client; self.events=[]
    def system_one(self, **request):
        start=time.perf_counter()
        try: response=self.client.system_one(**request)
        except Exception as exc:
            self.events.append({'ms':(time.perf_counter()-start)*1000,'error_type':type(exc).__name__,'model':None,'usage':{'input_tokens':None,'output_tokens':None}})
            raise
        usage=getattr(response,'usage',None)
        self.events.append({'ms':(time.perf_counter()-start)*1000,'model':getattr(response,'model',None),
            'usage':{k:getattr(usage,k,None) for k in ('input_tokens','output_tokens')}})
        return response

def frontier_plan(owner, source):
    controls=[{'label':c.label,'role':c.role_class} for c in source.controls]
    prompt='Compile the following authorized text-only task into a plan. Do not use tools or read files. Copy exact observed labels and exact user literals; no extra actions. Owner must remain exact.\n'+json.dumps({'owner':owner,'goal':GOAL,'observed_controls':controls})
    argv=['codex','exec','--ignore-user-config','--ephemeral','--sandbox','read-only','--skip-git-repo-check','--disable','shell_tool','--disable','unified_exec','-m','gpt-6-sol','-c','model_reasoning_effort="low"','--output-schema',str(HERE/'plan.schema.json'),'--json','-C','/tmp','-']
    start=time.perf_counter(); done=subprocess.run(argv,input=prompt,capture_output=True,text=True,timeout=90)
    events=[json.loads(line) for line in done.stdout.splitlines() if line.strip()]
    allowed={'thread.started','turn.started','turn.completed','item.completed'}
    report={'ms':(time.perf_counter()-start)*1000,'requested_model':'gpt-6-sol','resolved_model':None,'turns':sum(e.get('type')=='turn.completed' for e in events),'usage':[e['usage'] for e in events if e.get('type')=='turn.completed'],'events':events}
    if done.returncode or any(e.get('type') not in allowed for e in events): return None,report
    items=[e['item'] for e in events if e.get('type')=='item.completed']
    if any(item.get('type') not in {'agent_message','reasoning'} for item in items): return None,report
    messages=[item for item in items if item.get('type')=='agent_message']
    if len(messages)!=1: return None,report
    usage=[e['usage'] for e in events if e.get('type')=='turn.completed']
    if len(usage)!=1: return None,report
    return json.loads(messages[0]['text']), report

async def trial(client, sdk, http, mode, rep, smoke=False):
    fixture=None; offset=len(client.calls); so=len(sdk.events); ho=len(http.events)
    row={'mode':mode,'rep':rep,'smoke':smoke,'passed':False,'terminal':'handoff','requests':[],'decisions':[],'frontier':None}
    start=time.perf_counter()
    try:
        fixture=h.Fixture(); driver=PublicDriver(client)
        window = None
        for attempt in range(20):
            listed = await driver.call('list_windows', {'pid':fixture.pid})
            matches = [w for w in listed.get('windows',[]) if w.get('title')=='Arc Bench Form' and w.get('is_on_screen') is True]
            if len(matches)==1:
                candidate_window = matches[0].get('window_id')
                if type(candidate_window) is int:
                    probe = await driver.call('get_window_state', {'pid':fixture.pid,'window_id':candidate_window,'include_accessibility_tree':True,'include_screenshot':False,'timeout_ms':1000})
                    observed = NativeObservation.from_window_state(probe,expected_pid=fixture.pid,expected_window_id=candidate_window)
                    visible_source = NativeAccessibilitySource.from_observation(observed,'macos')
                    if all(visible_source.find('text_input',step.label) is not None for step in STEPS):
                        window=candidate_window;break
            await asyncio.sleep(.1)
        if window is None: raise LiteralPlanHandoff('Owned fixture window did not become actionable')
        offset=len(client.calls)  # setup readiness is excluded from timed task in all arms
        owner={'pid':fixture.pid,'window_id':window}
        def safe():
            state=fixture.state()
            return state.get('record')=='Record A' and state.get('submitted')==0 and state.get('subscribe') is False and state.get('dialog') is False
        def final(): return safe() and fixture.state().get('name')=='Synthetic Person' and fixture.state().get('email')=='synthetic@example.invalid'
        async def verify_final(): return final()
        history=[]
        async def verify_step(step):
            field={'Full name':'name','Email':'email'}[step.label]
            try: await asyncio.to_thread(h.wait_for,lambda:fixture.state().get(field)==step.value,3)
            except TimeoutError: return False
            if not safe(): return False
            if row['decisions']: history.append(task.history_entry(len(history)+1,row['decisions'][-1]['selected_id']))
            return True
        start=time.perf_counter()
        payload=await driver.call('get_window_state',{**owner,'include_accessibility_tree':True,'include_screenshot':True,'timeout_ms':1000})
        observation=NativeObservation.from_window_state(payload,expected_pid=fixture.pid,expected_window_id=window)
        source=NativeAccessibilitySource.from_observation(observation,'macos')
        if observation.truncated or (observation.partial and not source.controls) or not safe(): raise LiteralPlanHandoff('Initial planning observation unavailable')
        for step in STEPS:
            control=source.find('text_input',step.label)
            if control is None or control.handle.risk: raise LiteralPlanHandoff('Initial selector unavailable')
        if mode=='chooser_only': plan=STEPS
        else:
            generated,row['frontier']=await asyncio.to_thread(frontier_plan,owner,source)
            if generated is None: raise LiteralPlanHandoff('Frontier planning failed')
            plan=validate_literal_text_plan(generated,**owner,expected=STEPS)
        names={'Full name':'name','Email':'email'}
        task=NativeTask(id='owned-literal-fields',goal='First set Full name to parameter name; then set Email to parameter email. Leave Subscribe unchecked and Record A selected. Do not submit.',scope=WindowScope('Arc Bench Form'),allowed_actions=frozenset({'set_text'}),oracle=None,check=lambda s:'verified' if final() else 'pending',parameters=tuple(TaskParameter(names[s.label],s.value) for s in plan),steps=tuple(TaskStep('Set '+s.label,source.find('text_input',s.label).handle.id+':set:'+names[s.label]) for s in plan))
        async def normal_choice(expected, obs):
            sources=TaskSources(ax=NativeAccessibilitySource.from_observation(obs,'macos'))
            native_step=task.plan(sources); request=native_choice_request(task,sources,native_step,history)
            validated=validate_request(request); row['requests'].append(request)
            decision=await asyncio.to_thread(choose,TypeSafeDecisionModel(sdk),DecisionRequest.from_validated(validated))
            row['decisions'].append({'selected_id':decision.selected_id,'model':decision.model,'confidence':decision.confidence})
            selected=next((c for c in native_step.candidates if c.id==decision.selected_id),None)
            # Common caller intent guard: chooser alternatives cannot broaden authority.
            if selected is None or selected.tool!=expected.tool or dict(selected.arguments)!=dict(expected.arguments): return 'abstain'
            return expected.id
        execution_driver=CompositeTextDriver(driver,fixture.pid,window) if mode=='frontier_literal_composite' else driver
        result=await execute_literal_text_plan(execution_driver,**owner,platform='macos',steps=plan,verify_step=verify_step,verify_final=verify_final,verify_context=lambda obs:safe(),choose=None if mode.startswith('frontier_literal') else normal_choice)
        if isinstance(execution_driver,CompositeTextDriver): row['composite_receipts']=execution_driver.receipts
        row.update(terminal=result['status'],passed=final())
    except Exception as exc: row['error_type']=type(exc).__name__; row['error']=str(exc)[:160] if isinstance(exc,LiteralPlanHandoff) else None
    finally:
        row['wall_ms']=(time.perf_counter()-start)*1000;row['calls']=safe_evidence(client.calls[offset:]);row['visible_mcp_calls']=len(row['calls']);row['canonical_child_calls']=0
        for call in row['calls']:
            if call['tool']!='experiment_action_observe':
                row['canonical_child_calls']+=1
            else:
                dispatches=call.get('result',{}).get('operation',{}).get('child_dispatches')
                if isinstance(dispatches,list) and dispatches in (['set_value'],['set_value','get_window_state']):
                    row['canonical_child_calls']+=len(dispatches)
                else:
                    row['canonical_child_calls']=None;break
        row['mcp_wall_ms']=sum(c['ms'] for c in row['calls']);row['jev']=sdk.events[so:];row['jev_http']=http.events[ho:]
        if fixture:
            try:
                row['oracle_state']=fixture.state()
                row['oracle_passed']=all(row['oracle_state'].get(k)==v for k,v in {'name':'Synthetic Person','email':'synthetic@example.invalid','subscribe':False,'submitted':0,'record':'Record A','dialog':False}.items())
            finally: fixture.close()
    row['false_completion']=row['terminal']=='verified_complete' and not row['passed']
    return row

def main():
    import Quartz
    session = Quartz.CGSessionCopyCurrentDictionary() or {}
    if session.get('CGSSessionScreenIsLocked'):
        raise RuntimeError('Live evaluation requires the unlocked desktop; no fixture/input started')
    subprocess.run(['git','-C',str(CUA),'fetch','origin','main'],check=True,capture_output=True); versions=h.candidate_versions();versions['cua_source_head']=subprocess.check_output(['git','-C',str(CUA),'rev-parse','HEAD'],text=True).strip();versions['cua_upstream_head']=subprocess.check_output(['git','-C',str(CUA),'rev-parse','origin/main'],text=True).strip();subprocess.run(['git','-C',str(CUA),'merge-base','--is-ancestor',versions['cua_upstream_head'],'HEAD'],check=True);versions['recipe_file_sha256']=hashlib.sha256((CUA/'libs/cua-driver/examples/jev-use/python/literal_text_plan.py').read_bytes()).hexdigest();versions['runner_sha256']=hashlib.sha256(Path(__file__).read_bytes()).hexdigest();versions['native_release_commit']=subprocess.check_output(['gh','api','repos/trycua/cua/git/ref/tags/cua-driver-rs-v'+versions['native_version'],'--jq','.object.sha'],text=True).strip();versions['codex_cli']=subprocess.check_output(['codex','--version'],text=True).strip()
    key=os.environ.get('TYPESAFE_API_KEY')
    if not key:
        command=json.loads(os.environ['JEV_CREDENTIAL_COMMAND']); fetched=subprocess.run(command,capture_output=True,text=True,timeout=30)
        if fetched.returncode or not fetched.stdout.strip(): raise RuntimeError('Runtime credential unavailable')
        key=fetched.stdout.strip()
    http=TimedHTTP();out={**versions,'results':[]}
    try:
        sdk=RecordingSDK(TypeSafeClient(api_key=key,model='jev-latest',http_client=http))
        with ExitStack() as stack:
            arc=h.MCP([sys.executable,'-m','arc_cua','mcp'],'agent-normal-arc'); stack.callback(arc.close)
            cua=h.MCP(['cua-driver','mcp','--socket',str(Path.home()/'Library/Caches/cua-driver/cua-driver.sock')],'agent-normal-cua');stack.callback(cua.close)
            host_path=os.environ.get('ACTION_OBSERVE_HOST')
            if not host_path: raise RuntimeError('ACTION_OBSERVE_HOST required for actual daemon metadata preflight')
            out['operation_host_sha256']=hashlib.sha256(Path(host_path).read_bytes()).hexdigest()
            host=MetadataMCP(['env','ACTION_OBSERVE_BACKEND=daemon',host_path],'agent-normal-composite');stack.callback(host.close)
            metadata=host.backend_metadata
            if not isinstance(metadata,dict) or metadata.get('driver_version')!=versions['native_version'] or metadata.get('embedded') is not False or type(metadata.get('pid')) is not int:
                raise RuntimeError('Actual running daemon metadata differs from latest release or is unavailable')
            out['daemon_metadata']=metadata;out['operation_server_info']=host.server_info
            h.verify_servers(versions,arc,cua);out['servers']={'arc':arc.server_info,'cua':cua.server_info}
            smoke=os.environ.get('SMOKE_ONLY')=='1'
            for rep in range(1 if smoke else 5):
                modes=['chooser_only','frontier_normal','frontier_literal','frontier_literal_composite']
                if rep%2: modes.reverse()
                for mode in modes:
                    row=asyncio.run(trial(host if mode=='frontier_literal_composite' else cua,sdk,http,mode,rep,smoke));out['results'].append(row)
                    resolved={event['model'] for item in out['results'] for event in item['jev'] if event.get('model')}
                    out['resolved_jev_models']=sorted(resolved)
                    encoded=json.dumps(out,indent=2)
                    if key in encoded: raise RuntimeError('Secret persistence refused')
                    (HERE/('smoke-results.json' if smoke else 'results.json')).write_text(encoded+'\n')
                    print(json.dumps({k:row[k] for k in ('mode','rep','passed','error_type','wall_ms') if k in row}),flush=True)
                    if len(resolved)>1: raise RuntimeError('JEV server model changed during comparison; evidence retained')
    finally: http.close()
if __name__=='__main__': main()
