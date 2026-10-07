"""Same-build trials using the explicitly selected off-screen display model."""
import os,sys,json,asyncio,inspect,subprocess,hashlib
from pathlib import Path
sys.path.insert(0,str(Path(os.environ['OFFSCREEN_MODEL_SOURCE'])/'computer_use'))
from spaces_client import SpaceMover
import live as l
original_trial_source=inspect.getsource(l.trial)
exec(Path(__file__).with_name('reference.py').read_text().rsplit('asyncio.run(main())',1)[0])
class Foreground:
 def __init__(self,pid):self.pid=pid
 def state(self):return {'active':l.fresh_front()==self.pid}
async def main():
 out={'versions':l.preflight(),'source_commit':subprocess.check_output(['git','-C',str(l.ROOT),'rev-parse','HEAD'],text=True).strip(),'binary_sha256':hashlib.sha256(Path(l.BINARY).read_bytes()).hexdigest(),'rows':[]}
 mover=SpaceMover();out['baseline_displays']=mover.run('displays','--online')['displays'];original=l.fresh_front();clients={};out['original_pid']=original
 try:
  display=mover.ensure_agent_display();os.environ['CUA_TEST_DISPLAY_ID']=str(display);out['owned_display_id']=display;out['display_inventory']=mover.run('displays','--online')['displays'];out['display_helper_sha256']=hashlib.sha256(mover.binary.read_bytes()).hexdigest()
  if l.fresh_front()!=original:raise RuntimeError('Display creation changed foreground')
  clients={name:l.Native(name) for name in ('baseline','owned','settled')};clients['reference']=Reference(clients['owned']);out['native_backend']=clients['owned'].backend_metadata;out['reference_server']=clients['reference'].wire.server_info
  if clients['reference'].wire.server_info['version']!=out['versions']['reference_version'] or out['native_backend']['driver_version']!=out['versions']['native_version']:raise RuntimeError('Running backend mismatch')
  import Quartz
  def front_window():
   rows=Quartz.CGWindowListCopyWindowInfo(Quartz.kCGWindowListOptionOnScreenOnly,Quartz.kCGNullWindowID)
   matches=[row for row in rows if int(row.get('kCGWindowOwnerPID',0))==original and int(row.get('kCGWindowLayer',-1))==0]
   if not matches:raise RuntimeError('Original foreground window unavailable')
   return int(matches[0]['kCGWindowNumber'])
  front_w=front_window();foreground=Foreground(original)
  native_source=original_trial_source
  # Remove synthetic sentinel activation; keep the user's current foreground.
  start=native_source.index("  activation,err=c.call('bring_to_front'")
  end=native_source.index('  observed_active=False',start)
  replacement="  row['foreground_binding']=c.request('research/restore_target',{'pid':sentinel.pid,'window_id':FRONT_WINDOW})\n  if not all(row['foreground_binding'].get(k) is True for k in ('registered','cocoa_front_matches','native_front_matches')):raise RuntimeError('foreground_binding_failed')\n  verify_display(f,w)\n"
  native_source=native_source[:start]+replacement+native_source[end:]
  insert="""  if mode=='combined':
   proof=await c.observe(f,w)
   if not m.evidence(proof,f.pid,w)['record_a_visible'] or f.state().get('record')!='Record A':raise RuntimeError('context_changed')
   if any(m.h.find(c,proof,{'name':'Full name','email':'Email'}[field]).get('value')!=value for _,field,value in txs):raise RuntimeError('commit_proof_failed')
   row['verified_ms']=(time.perf_counter()-start)*1000
"""
  native_source=native_source.replace("  if hasattr(c,'dispatches'):",insert+"  if hasattr(c,'dispatches'):",1)
  virtual=next(d for d in out['display_inventory'] if d['id']==display)
  def verify_display(f,w):
   rows=Quartz.CGWindowListCopyWindowInfo(Quartz.kCGWindowListOptionOnScreenOnly,Quartz.kCGNullWindowID)
   row=next((r for r in rows if int(r.get('kCGWindowNumber',0))==w and int(r.get('kCGWindowOwnerPID',0))==f.pid),None)
   if row is None:raise RuntimeError('Fixture window unavailable')
   b=row['kCGWindowBounds'];x,y,width,height=[float(b[k]) for k in ('X','Y','Width','Height')]
   if not (x>=virtual['x'] and y>=virtual['y'] and x+width<=virtual['x']+virtual['width'] and y+height<=virtual['y']+virtual['height']):raise RuntimeError('Fixture is not wholly on owned display: '+json.dumps(dict(b)))
  l.__dict__.update(FRONT_WINDOW=front_w,verify_display=verify_display)
  exec(native_source,l.__dict__);native_trial=l.trial
  external_source=native_source.replace("not m.evidence(b,f.pid,w)['record_a_visible']", "not any('Record A' in (e.get('name'),e.get('label'),e.get('value')) for e in b.get('elements',[]))")
  names=list(clients)
  for rep in range(5):
   for name in (names if rep%2==0 else list(reversed(names))):
    l.preflight()
    if l.fresh_front()!=original:raise RuntimeError('User foreground changed; no restoration or further input')
    if name=='reference':exec(external_source,l.__dict__);row=await l.trial(clients[name],'evidence','stable',rep,foreground)
    else:row=await native_trial(clients[name],'evidence' if name=='baseline' else 'combined','stable',rep,foreground)
    row['candidate']=name;row['warmup']=rep==0
    if name in ('baseline','reference'):row['verified_ms']=row.get('decision_ms')
    out['rows'].append(row);print(json.dumps({k:row.get(k) for k in ('candidate','verified_ms','decision_ms','strict_pass','foreground_preserved','error')}),flush=True)
    if not row['strict_pass'] or not row['foreground_preserved'] or row.get('competing_input_detected'):raise RuntimeError('Off-screen qualification/interference failure')
 finally:
  for c in clients.values():c.close()
  out['foreground_unchanged']=l.fresh_front()==original
  try:mover.stop();out['display_removed']=True
  except Exception as error:out['display_cleanup_error']=str(error);out['display_removed']=False
  out['final_displays']=mover.run('displays','--online')['displays'];out['topology_restored']=out['final_displays']==out['baseline_displays']
  (l.OUT/'offscreen-results.json').write_text(json.dumps(out,indent=2))
asyncio.run(main())
