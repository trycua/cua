"""Close transport after dispatch; verify retained observation and independent state."""
import live as l,asyncio,json,time
class Disconnect(l.Native):
 def __init__(self,helper):super().__init__('supervision');self.helper=helper;self.drained=None
 async def observe(self,f,w):
  return await (self.helper.observe(f,w) if self.drained is not None else super().observe(f,w))
 def fence(self):
  if self.drained is not None:return self.drained
  t=time.perf_counter();self.proc.stdin.close();code=self.proc.wait(timeout=7);self.drained={'exit_code':code,'disconnect_wait_ms':(time.perf_counter()-t)*1000}
  if code!=0 or self.drained['disconnect_wait_ms']<700:raise RuntimeError('Disconnect did not retain supervision')
  return self.drained
async def main():
 out={'versions':l.preflight()};original=l.fresh_front();sentinel=l.m.h.Fixture();helper=l.Native('independent');c=Disconnect(helper)
 try:
  row=await l.trial(c,'supervision','activation',-1,sentinel);out['row']=row;print(json.dumps(row),flush=True)
  if not row['strict_pass'] or not row['foreground_preserved'] or row.get('competing_input_detected'):raise RuntimeError('Disconnect qualification failed')
 finally:
  helper.call('bring_to_front',pid=original)
  try:await asyncio.to_thread(l.m.h.wait_for,lambda:l.fresh_front()==original,3);out['cleanup_restored']=True
  except Exception:out['cleanup_restored']=False
  sentinel.close();c.close();helper.close();(l.OUT/'native-disconnect.json').write_text(json.dumps(out,indent=2))
asyncio.run(main())
