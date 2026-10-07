"""Exercise the public receipt tools while a real native observer is pending."""
import live as l,inspect,asyncio,json
source=inspect.getsource(l.trial)
checks='''   if mode=='combined':
    rid=receipt_ids[-1]
    pending,error=c.call('get_action_supervision',receipt_id=rid)
    if error or pending.get('supervision',{}).get('state')!='pending':raise RuntimeError('pending_receipt_missing')
    timeout,error=c.call('fence_action_supervision',receipt_id=rid,timeout_ms=0)
    if not error or timeout.get('refusal')!='timeout' or timeout.get('replay_allowed') is not False:raise RuntimeError('timeout_refusal_missing')
    release,error=c.call('release_action_supervision',receipt_id=rid)
    if not error or 'Pending' not in json.dumps(release):raise RuntimeError('pending_release_allowed')
    original_scope=c.session;c.session+='-foreign'
    try:
     foreign,error=c.call('get_action_supervision',receipt_id=rid)
     if not error or foreign.get('refusal')!='unavailable':raise RuntimeError('foreign_receipt_visible')
    finally:c.session=original_scope
    row.setdefault('receipt_checks',[]).append({'pending':True,'timeout_retains_observer':True,'pending_release_refused':True,'foreign_scope_refused':True})
'''
source=source.replace("   row['writes']+=1;txs.append((tx,field,value))", "   row['writes']+=1;txs.append((tx,field,value))\n"+checks)
terminal='''  for rid in receipt_ids:
   release,error=c.call('release_action_supervision',receipt_id=rid)
   if error or release.get('released') is not True:raise RuntimeError('terminal_release_failed')
   unavailable,error=c.call('get_action_supervision',receipt_id=rid)
   if not error or unavailable.get('refusal')!='unavailable':raise RuntimeError('released_receipt_visible')
  row['terminal_release_verified']=True
'''
source=source.replace("  row['decision_ms']=",terminal+"  row['decision_ms']=")
# The final cleanup fence reads IDs already released; it must be skipped after verified release.
source=source.replace("  if mode in ('supervision','combined'):\n   try:c.fence()", "  if mode in ('supervision','combined') and not row.get('terminal_release_verified'):\n   try:c.fence()")
exec(source,l.__dict__)
async def main():
 out={'versions':l.preflight()};original=l.fresh_front();sentinel=l.m.h.Fixture();c=l.Native('settled')
 try:
  row=await l.trial(c,'combined','stable',-1,sentinel);out['row']=row;print(json.dumps(row),flush=True)
  if not row['strict_pass'] or not row['foreground_preserved'] or row.get('competing_input_detected'):raise RuntimeError('Receipt API qualification failed')
 finally:
  c.call('bring_to_front',pid=original)
  try:await asyncio.to_thread(l.m.h.wait_for,lambda:l.fresh_front()==original,3);out['cleanup_restored']=True
  except Exception:out['cleanup_restored']=False
  sentinel.close();c.close();(l.OUT/'native-receipts.json').write_text(json.dumps(out,indent=2))
asyncio.run(main())
