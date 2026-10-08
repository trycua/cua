"""Repeated real Codex tool-agent qualification; no model timings or tokens are inferred."""
import argparse, hashlib, json, os, statistics, subprocess, time
from datetime import datetime, timezone
from pathlib import Path

HERE=Path(__file__).resolve().parent
PROMPT='Use only the fixture MCP tools to complete the owned synthetic form. Observe it, set Full name to Synthetic Person, set Email to synthetic@example.invalid, and enable Subscribe. Do not Submit. Choose exact current observed element tokens with pid/window. After every action, verify its desired effect through the returned fresh observation or a new observation before the next input. If a child receipt says its effect is unverifiable, observe the effect; never replay input. Stop on typed refusal or when a fresh read cannot verify the intended change. The final fresh observation must show all three requested values. Report completion only after those final values verify. No shell, other apps, files, or unrelated tools.'

def trial(root, condition, rep, model):
    directory=root/f'{rep}-{condition}'; directory.mkdir()
    variables={'ACTION_OBSERVE_CONDITION':condition,'ACTION_OBSERVE_AGENT_EVIDENCE':str(directory/'gateway.json'),'ACTION_OBSERVE_BINARY':os.environ['ACTION_OBSERVE_BINARY'],'REFERENCE_EVAL_SOURCE':os.environ['REFERENCE_EVAL_SOURCE'],'ACTION_OBSERVE_BACKEND':'daemon'}
    env_toml='{'+','.join(key+'='+json.dumps(value) for key,value in variables.items())+'}'
    cmd=['codex','exec','--ignore-user-config','--ephemeral','--sandbox','danger-full-access','--skip-git-repo-check','--disable','shell_tool','--disable','unified_exec','-m',model,'-c','model_reasoning_effort="low"','-c','mcp_servers.fixture.command='+json.dumps(os.environ['ACTION_OBSERVE_PYTHON']),'-c','mcp_servers.fixture.args='+json.dumps([str(HERE/'agent_gateway.py')]),'-c','mcp_servers.fixture.env='+env_toml,'--output-schema',str(root/'final-schema.json'),'--json','-C',str(directory),'-']
    started=time.perf_counter()
    with (directory/'events.jsonl').open('w') as out,(directory/'stderr.log').open('w') as err:
        result=subprocess.run(cmd,input=PROMPT,text=True,stdout=out,stderr=err,timeout=180)
    wall_ms=(time.perf_counter()-started)*1000
    events=[json.loads(line) for line in (directory/'events.jsonl').read_text().splitlines()]
    gateway=json.loads((directory/'gateway.json').read_text()) if (directory/'gateway.json').exists() else {'passed':False,'calls':[],'setup_failure':True}
    items=[e['item'] for e in events if e.get('type')=='item.completed']
    usage=[e['usage'] for e in events if e.get('type')=='turn.completed' and isinstance(e.get('usage'),dict)]
    final=[i.get('text','') for i in items if i.get('type')=='agent_message']
    try:reported_complete=json.loads(final[-1]).get('completed') is True
    except (IndexError,ValueError,AttributeError):reported_complete=False
    row={'reported_complete':reported_complete,'rep':rep,'condition':condition,'model':model,'returncode':result.returncode,'wall_ms':wall_ms,'passed':gateway['passed'] and reported_complete,'gateway':gateway,'turn_started_events':sum(e.get('type')=='turn.started' for e in events),'turn_completed_events':len(usage),'usage':usage,'mcp_items':sum(i.get('type')=='mcp_tool_call' for i in items),'other_tool_items':[i.get('type') for i in items if i.get('type') not in ('agent_message','reasoning','mcp_tool_call')],'visible_calls':len(gateway['calls']),'child_registry_calls':sum(len(c.get('child_dispatches',[])) for c in gateway['calls']),'driver_ms':sum(c['ms'] for c in gateway['calls'])}
    if row['other_tool_items']: row['passed']=False
    (directory/'summary.json').write_text(json.dumps(row,indent=2)+'\n')
    print(json.dumps(row),flush=True)
    return row

def preflight():
    def utc(): return datetime.now(timezone.utc).isoformat()
    started=utc()
    from shared import MCP
    import Quartz
    session=Quartz.CGSessionCopyCurrentDictionary() or {}
    displays=Quartz.CGGetActiveDisplayList(32,None,None)
    if session.get('CGSSessionScreenIsLocked') or not displays[2]:raise RuntimeError('native qualification requires an unlocked session with an active display; no session changes attempted')
    update=json.loads(subprocess.check_output(['cua-driver','check-update','--json','--no-cache'],text=True))
    if update.get('error') or update.get('update_available') or update.get('current_version')!=update.get('latest_version'):
        raise RuntimeError('installed candidate is not verified current')
    client=MCP(['cua-driver','mcp'],'agent-preflight')
    try: server=client.server_info
    finally: client.close()
    if server.get('version')!=update['current_version']:raise RuntimeError('running server differs from installed current candidate')
    previous=os.environ.get('ACTION_OBSERVE_BACKEND')
    os.environ['ACTION_OBSERVE_BACKEND']='daemon'
    try:
        host=MCP([os.environ['ACTION_OBSERVE_BINARY']],'sdk-metadata-preflight')
        try: metadata=host.backend_metadata
        finally: host.close()
    finally:
        if previous is None:os.environ.pop('ACTION_OBSERVE_BACKEND',None)
        else:os.environ['ACTION_OBSERVE_BACKEND']=previous
    if not isinstance(metadata,dict) or metadata.get('driver_version')!=update['current_version'] or metadata.get('embedded') is not False or metadata.get('host_bundle_id') not in (None,'com.trycua.driver') or not metadata.get('pid'):
        raise RuntimeError('SDK connected backend identity does not prove current signed daemon')
    permissions=json.loads(subprocess.check_output(['cua-driver','permissions','status','--json'],text=True))
    source=permissions.get('source',{})
    if source.get('pid')!=metadata['pid'] or source.get('bundle_id')!='com.trycua.driver' or permissions.get('accessibility') is not True or permissions.get('screen_recording') is not True:
        raise RuntimeError('SDK metadata PID is not the permission-attributed signed daemon with current grants')
    metadata_checked_at=utc()
    repo=HERE.parents[6]
    upstream=subprocess.check_output(['git','ls-remote','https://github.com/trycua/cua.git','refs/heads/main'],text=True).split()[0]
    ancestor=subprocess.check_output(['git','merge-base','HEAD',upstream],cwd=repo,text=True).strip()
    if ancestor!=upstream:raise RuntimeError('SDK example is not based on latest upstream source')
    return {'fixture_source_commit':subprocess.check_output(['git','rev-parse','HEAD'],cwd=os.environ['REFERENCE_EVAL_SOURCE'],text=True).strip(),'preflight_started_at':started,'source_checked_at':utc(),'daemon_metadata_checked_at':metadata_checked_at,'release_check':update,'daemon_server':server,'sdk_backend_metadata':metadata,'daemon_permissions':permissions,'sdk_upstream_commit':upstream,'sdk_head':subprocess.check_output(['git','rev-parse','HEAD'],cwd=repo,text=True).strip(),'source_file_sha256':{str(p.name):hashlib.sha256(p.read_bytes()).hexdigest() for p in [HERE/'operation.rs',HERE/'agent_gateway.py',HERE/'run_agent_trials.py',HERE/'shared.py',HERE.parent/'action_observe_experiment.rs']},'backend':'supported CuaDriver.connect existing signed daemon; same backend both arms'}

def main():
    parser=argparse.ArgumentParser();parser.add_argument('output',type=Path);parser.add_argument('--pairs',type=int,default=5);parser.add_argument('--model',default='gpt-6-sol');args=parser.parse_args()
    args.output.mkdir(parents=True,exist_ok=True);rows=[];versions=preflight()
    (args.output/'final-schema.json').write_text(json.dumps({'type':'object','properties':{'completed':{'type':'boolean'}},'required':['completed'],'additionalProperties':False}))
    (args.output/'preflight.json').write_text(json.dumps(versions,indent=2)+'\n')
    for rep in range(args.pairs):
        for condition in (('baseline','composite') if rep%2==0 else ('composite','baseline')):
            rows.append(trial(args.output,condition,rep,args.model))
            (args.output/'results.json').write_text(json.dumps({'binary_sha256':hashlib.sha256(Path(os.environ['ACTION_OBSERVE_BINARY']).read_bytes()).hexdigest(),'prompt_sha256':hashlib.sha256(PROMPT.encode()).hexdigest(),'model':args.model,'preflight':versions,'results':rows},indent=2)+'\n')
    for condition in ('baseline','composite'):
        selected=[r for r in rows if r['condition']==condition]
        print(condition,'passed',sum(r['passed'] for r in selected),'median_wall_ms',statistics.median(r['wall_ms'] for r in selected),'median_driver_ms',statistics.median(r['driver_ms'] for r in selected))

if __name__=='__main__':main()
