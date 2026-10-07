"""Adversarial native field-settlement outcomes through the public tool."""
import live as l,asyncio,json,uuid,time
from pathlib import Path
async def one(c,sentinel,scenario):
 saved=l.m.h.HERE;l.m.h.HERE=l.OUT/'fixture';f=l.m.h.Fixture();l.m.h.HERE=saved
 row={'scenario':scenario}
 try:
  w=await l.m.window_ready(c,f);c.call('bring_to_front',pid=sentinel.pid)
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
  f.close();out['rows'].append(row);print(json.dumps({k:row.get(k) for k in ('scenario','call_ms','passed','error')}),flush=True)
async def main():
 global out
 out={'versions':l.preflight(),'rows':[]};original=l.fresh_front();sentinel=l.m.h.Fixture();c=l.Native('settled')
 try:
  for scenario in ('noop','early_reject','early_activation','no_reaction','noisy','closed_window'):await one(c,sentinel,scenario)
 finally:
  c.call('bring_to_front',pid=original)
  try:await asyncio.to_thread(l.m.h.wait_for,lambda:l.fresh_front()==original,3);out['cleanup_restored']=True
  except Exception:out['cleanup_restored']=False
  sentinel.close();c.close();(l.OUT/'field-controls.json').write_text(json.dumps(out,indent=2))
asyncio.run(main())
