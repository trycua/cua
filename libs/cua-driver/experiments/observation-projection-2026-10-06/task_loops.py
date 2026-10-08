"""Caller-literal task loops; no invented model latency or skipped guard reads."""
import asyncio,hashlib,json,os,subprocess,time
from contextlib import ExitStack
from pathlib import Path
import observe_matrix as matrix
from literal_text_plan import LiteralTextStep,execute_literal_text_plan
from tree_geometry import TreeGeometryDriver
STEPS=(LiteralTextStep('Full name','Synthetic Person'),LiteralTextStep('Email','synthetic@example.invalid'))
EXPECTED={'name':'Synthetic Person','email':'synthetic@example.invalid','record':'Record A','subscribe':False,'submitted':0,'dialog':False}
async def trial(client,case,arm,mode,rep,negative=False):
    h=matrix.h;fixture=None;saved=h.HERE;row={'case':case,'arm':arm,'mode':mode,'rep':rep,'terminal':'handoff','strict_pass':False};offset=len(client.calls);start=time.perf_counter()
    if case=='large':h.HERE=Path(os.environ.get('LARGE_FIXTURE_SOURCE',str(matrix.HERE/'fixtures-large')))
    try:
        fixture=h.Fixture();window=await matrix.window_ready(client,fixture)
        if negative:fixture.mutate('record')
        driver=matrix.Driver(client);reader=TreeGeometryDriver(driver) if mode=='tree_geometry' else driver
        offset=len(client.calls);start=time.perf_counter()
        def safe():
            state=fixture.state()
            return all(state.get(k)==v for k,v in EXPECTED.items() if k not in ('name','email'))
        async def step_verify(step):
            field={'Full name':'name','Email':'email'}[step.label]
            try:await asyncio.to_thread(h.wait_for,lambda:fixture.state().get(field)==step.value,3)
            except TimeoutError:return False
            return safe()
        def context(obs):
            return safe() and 'Record A' in obs.tree_markdown and 'Record B' not in obs.tree_markdown
        async def final_verify():
            snapshot=await reader.call('get_window_state',{'pid':fixture.pid,'window_id':window,'include_accessibility_tree':True,'include_screenshot':True,'timeout_ms':1000})
            proof=matrix.evidence(snapshot,fixture.pid,window)
            observation=matrix.NativeObservation.from_window_state(snapshot,expected_pid=fixture.pid,expected_window_id=window)
            source=matrix.NativeAccessibilitySource.from_observation(observation,'macos')
            values={step.label:(source.find('text_input',step.label).handle.value if source.find('text_input',step.label) is not None else None) for step in STEPS}
            row['final_observation_values']=values;row['final_values_verified']=values=={step.label:step.value for step in STEPS}
            row['final_snapshot_id']=snapshot.get('snapshot_id');row['final_observation_proof']=proof
            return row['final_values_verified'] and proof['required_fields_unique'] and proof['record_a_visible'] and not proof['truncated'] and all(fixture.state().get(k)==v for k,v in EXPECTED.items())
        result=await execute_literal_text_plan(reader,pid=fixture.pid,window_id=window,platform='macos',steps=STEPS,verify_step=step_verify,verify_final=final_verify,verify_context=context)
        row['terminal']=result['status'];row['steps']=result['steps']
        row['oracle_state']=fixture.state();row['oracle_passed']=all(row['oracle_state'].get(k)==v for k,v in EXPECTED.items());row['strict_pass']=row['terminal']=='verified_complete' and row.get('final_values_verified',False) and row['oracle_passed']
    except Exception as exc:row['error_type']=type(exc).__name__;row['error']=str(exc)[:160]
    finally:
        row['wall_ms']=(time.perf_counter()-start)*1000;row['calls']=client.calls[offset:]
        row['public_calls']=len(row['calls']);row['mcp_ms']=sum(c['ms'] for c in row['calls']);row['serialized_result_bytes']=sum(c['serialized_result_bytes'] for c in row['calls']);row['image_bytes']=sum(c['image_bytes'] for c in row['calls'])
        if isinstance(locals().get('reader'),TreeGeometryDriver):row['geometry_receipts']=reader.receipts
        try:
            if fixture:
                try:row['oracle_state']=fixture.state();row['oracle_passed']=all(row['oracle_state'].get(k)==v for k,v in EXPECTED.items())
                finally:fixture.close()
        finally:h.HERE=saved
        row['strict_pass']=row['terminal']=='verified_complete' and row.get('final_values_verified',False) and row.get('oracle_passed',False)
        row['false_completion']=row['terminal']=='verified_complete' and not row.get('oracle_passed',False)
    return row

def main():
    import Quartz
    if (Quartz.CGSessionCopyCurrentDictionary() or {}).get('CGSSessionScreenIsLocked'):raise RuntimeError('Locked desktop')
    subprocess.run(['git','-C',str(matrix.CUA),'fetch','origin','main'],check=True,capture_output=True)
    upstream=subprocess.check_output(['git','-C',str(matrix.CUA),'rev-parse','origin/main'],text=True).strip();subprocess.run(['git','-C',str(matrix.CUA),'merge-base','--is-ancestor',upstream,'HEAD'],check=True)
    versions=matrix.h.candidate_versions();out={**versions,'upstream_source':upstream,'checked_at':__import__('datetime').datetime.now(__import__('datetime').timezone.utc).isoformat(),'executor_source_sha256':hashlib.sha256((matrix.HERE/'literal_text_plan.py').read_bytes()).hexdigest(),'results':[],'controls':[],'servers':{}}
    arms=json.loads(os.environ['CUA_PROJECTION_ARMS']);selected=[a for a in arms if a['name'] in ('baseline','broad')]
    with ExitStack() as stack:
        clients={}
        for arm in selected:
            client=matrix.WireMCP(arm['argv'],'task-'+arm['name']);stack.callback(client.close);clients[arm['name']]=client
            metadata=getattr(client,'backend_metadata',None);manifest=json.loads(Path(arm['build_manifest']).read_text());actual=hashlib.sha256(Path(arm['argv'][0]).read_bytes()).hexdigest()
            if not isinstance(metadata,dict) or metadata.get('driver_version')!=versions['native_version'] or type(metadata.get('pid')) is not int or client.server_info['version']!=versions['native_version']:raise RuntimeError('Actual backend metadata mismatch')
            if manifest['binary_sha256']!=actual or manifest['base_source']!=upstream:raise RuntimeError('Stale build')
            subprocess.run(['git','-C',str(matrix.CUA),'merge-base','--is-ancestor',upstream,manifest['source_commit']],check=True)
            out['servers'][arm['name']]={'metadata':metadata,'build_manifest':manifest,'binary_sha256':actual}
        for case in ('small','large'):
            for rep in range(int(os.environ.get('CUA_TASK_REPS','3'))):
                order=[('baseline','full'),('broad','tree_geometry')]
                if rep%2:order.reverse()
                for arm,mode in order:
                    row=asyncio.run(trial(clients[arm],case,arm,mode,rep));out['results'].append(row);(matrix.HERE/'task-results.json').write_text(json.dumps(out,indent=2)+'\n');print(json.dumps({k:row.get(k) for k in ('case','arm','rep','terminal','strict_pass','wall_ms','error_type','error')}),flush=True)
            for arm,mode in [('baseline','full'),('broad','tree_geometry')]:
                row=asyncio.run(trial(clients[arm],case,arm,mode,-1,negative=True));row['control']='record_changed_before_plan';out['controls'].append(row);(matrix.HERE/'task-results.json').write_text(json.dumps(out,indent=2)+'\n')
if __name__=='__main__':main()
