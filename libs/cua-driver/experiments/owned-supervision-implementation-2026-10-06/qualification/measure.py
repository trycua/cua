from pathlib import Path
import live as l,inspect
original=inspect.getsource(l.trial)
exec(Path(__file__).with_name('reference.py').read_text().rsplit('asyncio.run(main())',1)[0])
insert='''  if mode=='combined':
   proof=await c.observe(f,w)
   if not m.evidence(proof,f.pid,w)['record_a_visible'] or f.state().get('record')!='Record A':raise RuntimeError('context_changed')
   if any(m.h.find(c,proof,{'name':'Full name','email':'Email'}[field]).get('value')!=value for _,field,value in txs):raise RuntimeError('commit_proof_failed')
   row['committed_ms']=(time.perf_counter()-start)*1000
   row['supervision_at_commit']='pending_owned'
'''
exec(original.replace("  if mode in ('supervision','combined'):row['fence']=c.fence()",insert+"  if mode in ('supervision','combined'):row['fence']=c.fence()"),l.__dict__)
cua_trial=l.trial
async def main():
 versions=l.preflight();helper=l.Native('combined');reference=Reference(helper);sentinel=l.m.h.Fixture();original_front=l.fresh_front();out={'source_commit':__import__('subprocess').check_output(['git','-C',str(l.ROOT),'rev-parse','HEAD'],text=True).strip(),'versions':versions,'original_pid':original_front,'binary_sha256':__import__('hashlib').sha256(Path(l.BINARY).read_bytes()).hexdigest(),'native_backend':helper.backend_metadata,'reference_server':reference.wire.server_info,'rows':[]};(l.OUT/'combined-commit.json').write_text(json.dumps(out,indent=2))
 try:
  if reference.wire.server_info['version']!=versions['reference_version'] or helper.backend_metadata['driver_version']!=versions['native_version']:raise RuntimeError('Running backend version mismatch')
  for rep in range(5):
   for client in ([helper,reference] if rep%2==0 else [reference,helper]):
    l.preflight()
    if client is reference:
     exec(source,l.__dict__);row=await l.trial(client,'evidence','stable',rep,sentinel)
    else:row=await cua_trial(client,'combined','stable',rep,sentinel)
    row['mode']='reference_ack' if client is reference else 'combined';row['warmup']=rep==0;out['rows'].append(row);print(json.dumps({k:row.get(k) for k in ('mode','committed_ms','decision_ms','strict_pass','foreground_preserved','error')}),flush=True)
    if not row['strict_pass'] or not row['foreground_preserved'] or row.get('competing_input_detected'):raise RuntimeError('Combined qualification/interference failure')
 finally:
  helper.call('bring_to_front',pid=original_front)
  try:await asyncio.to_thread(l.m.h.wait_for,lambda:l.fresh_front()==original_front,3);out['cleanup_restored']=True
  except Exception:out['cleanup_restored']=False
  finally:sentinel.close();reference.close();helper.close();(l.OUT/'combined-commit.json').write_text(json.dumps(out,indent=2))
asyncio.run(main())
