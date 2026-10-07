import os,sys
from pathlib import Path
sys.path.insert(0,str(Path(os.environ['OFFSCREEN_MODEL_SOURCE'])/'computer_use'))
from spaces_client import SpaceMover
class Foreground:
 def __init__(self,pid):self.pid=pid
 def state(self):return {'active':l.fresh_front()==self.pid}
"""Adversarial native field-settlement outcomes through the public tool."""
import live as l,asyncio,json,uuid,time
from pathlib import Path
async def one(c,sentinel,scenario):
 saved=l.m.h.HERE;l.m.h.HERE=l.OUT/'fixture';f=l.m.h.Fixture();l.m.h.HERE=saved
 row={'scenario':scenario}
 try:
  w=await l.m.window_ready(c,f)
  import Quartz
  bounds=next(dict(row['kCGWindowBounds']) for row in Quartz.CGWindowListCopyWindowInfo(Quartz.kCGWindowListOptionOnScreenOnly,Quartz.kCGNullWindowID) if int(row.get('kCGWindowNumber',0))==w and int(row.get('kCGWindowOwnerPID',0))==f.pid)
  x,y,width,height=[float(bounds[k]) for k in ('X','Y','Width','Height')]
  display=out['owned_display']
  if not (x>=display['x'] and y>=display['y'] and x+width<=display['x']+display['width'] and y+height<=display['y']+display['height']):raise RuntimeError('Fixture outside owned display; refusing input')
  row['verified_bounds']=bounds
  if l.fresh_front()!=sentinel.pid:raise RuntimeError('User foreground changed; no further input')
  await asyncio.to_thread(l.m.h.wait_for,lambda:l.fresh_front()==sentinel.pid,3)
  a=await c.observe(f,w);first=l.m.h.find(c,a,'Full name');b=await c.observe(f,w);element=l.m.h.find(c,b,'Full name')
  if not l.m.evidence(b,f.pid,w)['record_a_visible'] or first.get('value')!=element.get('value'):raise RuntimeError('context_changed')
  tx=uuid.uuid4().hex;value='' if scenario=='noop' else 'Settled Person'
  req={'tx':tx,'field':'name','value':value,'delay':0 if scenario in ('no_reaction','noisy') else .08,'reject':scenario=='early_reject','activate':scenario=='early_activation','no_reaction':scenario=='no_reaction','noisy':scenario=='noisy','close_window':scenario=='closed_window'}
  f.path.with_suffix('.txn').write_text(json.dumps(req));await asyncio.to_thread(l.m.h.wait_for,lambda:tx in f.state().get('transactions',{}),3)
  start=time.perf_counter();result,error=c.call('dispatch_set_value',pid=f.pid,window_id=w,element_token=element['element_token'],value=value,settle=True);row['call_ms']=(time.perf_counter()-start)*1000;row['result']=result;row['is_error']=error
  expected={'early_reject':'value_mismatch','early_activation':'interrupted','no_reaction':'no_reaction','noisy':'timed_out','closed_window':'unavailable','noop':'settled'}[scenario]
  if result.get('settlement',{}).get('status')!=expected or error!=(scenario!='noop') or result.get('application_commit')!='unverified' or result.get('replay_allowed') is not False:raise RuntimeError('Wrong settlement/refusal contract')
  if scenario=='noop' and result['settlement']['reacted']:raise RuntimeError('No-op falsely labelled a reaction')
  rid=result['receipt_id'];status,status_error=c.call('get_action_supervision',receipt_id=rid)
  if status_error:raise RuntimeError('Attempted input lost its receipt')
  row['receipt_after_return']=status
  fenced,fence_error=c.call('fence_action_supervision',receipt_id=rid,timeout_ms=3000);row['fence']=fenced
  if result.get('foreground_guard',{}).get('physical_input_unchanged') is not True or fenced.get('foreground_guard',{}).get('physical_input_unchanged') is not True:raise RuntimeError('Competing physical input invalidates the control')
  if fence_error or fenced.get('supervision',{}).get('state')!='finished':raise RuntimeError('Protection did not finish')
  row['oracle']=f.state();row['foreground_preserved']=l.fresh_front()==sentinel.pid and not row['oracle']['active']
  if not row['foreground_preserved']:raise RuntimeError('Foreground not restored')
  if scenario=='early_activation' and row['oracle']['activation_requests']!=1:raise RuntimeError('Activation injector did not run')
  if scenario=='closed_window' and row['oracle']['window_visible']:raise RuntimeError('Window close injector did not run')
  if scenario=='early_reject' and row['oracle']['name']!='REJECTED':raise RuntimeError('Rejection injector did not run')
  row['passed']=True
 except Exception as error:
  row['error']=str(error);raise
 finally:
  row.setdefault('oracle',f.state());f.close();out['rows'].append(row);print(json.dumps({k:row.get(k) for k in ('scenario','call_ms','passed','error')}),flush=True)
async def main():
 global out
 out={'versions':l.preflight(),'rows':[]};original=l.fresh_front();sentinel=Foreground(original);mover=SpaceMover();out['baseline_displays']=mover.run('displays','--online')['displays'];c=None
 try:
  os.environ['CUA_TEST_DISPLAY_ID']=str(mover.ensure_agent_display());out['owned_display_id']=int(os.environ['CUA_TEST_DISPLAY_ID']);out['owned_display']=next(d for d in mover.run('displays','--online')['displays'] if d['id']==out['owned_display_id']);c=l.Native('settled')
  for scenario in ('noop','early_reject','no_reaction','noisy','closed_window'):await one(c,sentinel,scenario)
 finally:
  # Leave deliberate user foreground changes alone.
  try:await asyncio.to_thread(l.m.h.wait_for,lambda:l.fresh_front()==original,3);out['cleanup_restored']=True
  except Exception:out['cleanup_restored']=False
  if c is not None:c.close()
  try:mover.stop();out['display_removed']=True
  except Exception as error:out['display_cleanup_error']=str(error);out['display_removed']=False
  out['topology_restored']=mover.run('displays','--online')['displays']==out['baseline_displays']
  (l.OUT/'offscreen-controls.json').write_text(json.dumps(out,indent=2))
asyncio.run(main())
