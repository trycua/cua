import hashlib,json,os,statistics,sys,time
from pathlib import Path
from shared import MCP,Fixture,windows,wait_for
BINARY=Path(os.environ['ACTION_OBSERVE_BINARY'])
client=MCP([str(BINARY)],'action-observe-experiment')
rows=[]
try:
 for rep in range(5):
  for composite in ((False,True) if rep%2==0 else (True,False)):
   fixture=Fixture()
   try:
    window=wait_for(lambda:windows(fixture.pid).get('Reference Bench Form'),8)
    snapshot,error=client.call('get_window_state',pid=fixture.pid,window_id=window,include_screenshot=False)
    assert not error
    offset=len(client.calls);started=time.perf_counter()
    for label,value in [('Full name','Synthetic Person'),('Email','synthetic@example.invalid'),('Subscribe',None)]:
     matches=[e for e in snapshot['elements'] if e.get('label',e.get('name'))==label and e.get('role') not in ('AXStaticText','AXGroup')]
     assert len(matches)==1,(label,[(e.get('role'),e.get('label')) for e in matches])
     target=matches[0];args={'pid':fixture.pid,'window_id':window,'element_token':target['element_token']}
     tool='click' if value is None else 'set_value'
     if value is not None:args['value']=value
     if composite:
      result,error=client.call('experiment_action_observe',tool=tool,arguments=args)
      assert not error
      assert result['action_acknowledged'] and result['observation_available']
      observation=result['observation_result'];assert not observation.get('isError')
      snapshot=observation['structuredContent']
     else:
      result,error=client.call(tool,**args);assert not error
      snapshot,error=client.call('get_window_state',pid=fixture.pid,window_id=window,include_screenshot=False);assert not error
     assert snapshot['pid']==fixture.pid and snapshot['window_id']==window
    elapsed=(time.perf_counter()-started)*1000
    expected={'name':'Synthetic Person','email':'synthetic@example.invalid','subscribe':True,'submitted':0}
    wait_for(lambda:all(fixture.state().get(k)==v for k,v in expected.items()),3)
    calls=client.calls[offset:]
    row={'rep':rep,'composite':composite,'passed':True,'wall_ms':elapsed,'tool_ms':sum(c['ms'] for c in calls),'visible_calls':len(calls),'child_registry_calls':6,'oracle_state':{k:fixture.state()[k] for k in expected}}
    rows.append(row);print(json.dumps(row),flush=True)
   finally:fixture.close()
finally:client.close()
output={'source_commit':'0b90b6f4a','binary_sha256':hashlib.sha256(BINARY.read_bytes()).hexdigest(),'server':client.server_info,'scope':'same-process SDK experimental host; one exact action plus same-window read vs individual calls; synthetic AppKit','results':rows}
Path(os.environ.get('ACTION_OBSERVE_OUTPUT', str(Path(__file__).with_name('results.json')))).write_text(json.dumps(output,indent=2)+'\n')
for composite in (False,True):
 a=[r for r in rows if r['composite']==composite];print(composite,'median_tool_ms',statistics.median(r['tool_ms'] for r in a),'median_wall_ms',statistics.median(r['wall_ms'] for r in a))
