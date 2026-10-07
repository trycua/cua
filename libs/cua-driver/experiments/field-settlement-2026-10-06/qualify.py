import live as l,inspect,asyncio,json
source=inspect.getsource(l.trial).replace("'activate':scenario=='activation'", "'activate':scenario=='activation','wrong_tx':scenario=='wrong_tx','record_change':scenario=='record_change'")
source=source.replace("if scenario!='stable' and field=='email':break", "if False:break")
source=source.replace("scenario in ('reject','late_reject')", "scenario in ('reject','late_reject') or (scenario=='second_reject' and field=='email')")
exec(source,l.__dict__)
async def main():
 out={'versions':l.preflight(),'rows':[]};original=l.fresh_front();sentinel=l.m.h.Fixture();clients={}
 try:
  for mode in ('combined',):clients[mode]=l.Native('settled')
  for scenario in ('wrong_tx','record_change','reject','second_reject','late_reject','missing_ack','activation'):
   for mode,c in clients.items():
    row=await l.trial(c,mode,scenario,-1,sentinel);out['rows'].append(row);print(json.dumps({k:row.get(k) for k in ('mode','scenario','error','writes','foreground_preserved')}),flush=True)
    if scenario=='activation':
     if row['strict_pass'] or row.get('error')!='foreground_changed' or row['writes']!=1 or not row['foreground_preserved']:raise RuntimeError('Delayed activation must stop before dependent input')
     continue
    if row['strict_pass'] or row['writes']!=(2 if scenario=='second_reject' else 1) or row.get('error') not in ('ack_binding','context_changed','rejected','ack_unavailable') or not row['foreground_preserved'] or row.get('competing_input_detected'):raise RuntimeError('Native contract guard failed')
 finally:
  if clients:next(iter(clients.values())).call('bring_to_front',pid=original)
  try:await asyncio.to_thread(l.m.h.wait_for,lambda:l.fresh_front()==original,3);out['cleanup_restored']=True
  except Exception:out['cleanup_restored']=False
  sentinel.close()
  for c in clients.values():c.close()
  (l.OUT/'native-guards.json').write_text(json.dumps(out,indent=2))
asyncio.run(main())
