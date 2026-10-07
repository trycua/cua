"""Same-build synchronous baseline, owned dispatch, field settling, external reference."""
from pathlib import Path
import live as l,inspect,asyncio,json,statistics,subprocess,hashlib
original=inspect.getsource(l.trial)
exec(Path(__file__).with_name('reference.py').read_text().rsplit('asyncio.run(main())',1)[0])
insert='''  if mode=='combined':
   proof=await c.observe(f,w)
   if not m.evidence(proof,f.pid,w)['record_a_visible'] or f.state().get('record')!='Record A':raise RuntimeError('context_changed')
   if any(m.h.find(c,proof,{'name':'Full name','email':'Email'}[field]).get('value')!=value for _,field,value in txs):raise RuntimeError('commit_proof_failed')
   row['verified_ms']=(time.perf_counter()-start)*1000
'''
exec(original.replace("  if hasattr(c,'dispatches'):",insert+"  if hasattr(c,'dispatches'):"),l.__dict__)
native_trial=l.trial
async def main():
 versions=l.preflight();clients={name:l.Native(name) for name in ('baseline','owned','settled')};reference=Reference(clients['owned']);clients['reference']=reference
 original_front=l.fresh_front();sentinel=l.m.h.Fixture();out={'source_commit':subprocess.check_output(['git','-C',str(l.ROOT),'rev-parse','HEAD'],text=True).strip(),'versions':versions,'binary_sha256':hashlib.sha256(Path(l.BINARY).read_bytes()).hexdigest(),'native_backend':clients['owned'].backend_metadata,'reference_server':reference.wire.server_info,'rows':[]}
 try:
  if reference.wire.server_info['version']!=versions['reference_version'] or clients['owned'].backend_metadata['driver_version']!=versions['native_version']:raise RuntimeError('Running backend version mismatch')
  names=list(clients)
  for rep in range(5):
   order=names if rep%2==0 else list(reversed(names))
   for name in order:
    l.preflight();client=clients[name]
    if name=='reference':exec(source,l.__dict__);row=await l.trial(client,'evidence','stable',rep,sentinel)
    else:row=await native_trial(client,'evidence' if name=='baseline' else 'combined','stable',rep,sentinel)
    row['candidate']=name;row['warmup']=rep==0
    if name in ('baseline','reference'):row['verified_ms']=row.get('decision_ms')
    out['rows'].append(row);print(json.dumps({k:row.get(k) for k in ('candidate','verified_ms','decision_ms','strict_pass','foreground_preserved','error')}),flush=True)
    if not row['strict_pass'] or not row['foreground_preserved'] or row.get('competing_input_detected'):raise RuntimeError('Qualification/interference failure')
 finally:
  clients['owned'].call('bring_to_front',pid=original_front)
  try:await asyncio.to_thread(l.m.h.wait_for,lambda:l.fresh_front()==original_front,3);out['cleanup_restored']=True
  except Exception:out['cleanup_restored']=False
  sentinel.close()
  for c in clients.values():c.close()
  (l.OUT/'results.json').write_text(json.dumps(out,indent=2))
asyncio.run(main())
