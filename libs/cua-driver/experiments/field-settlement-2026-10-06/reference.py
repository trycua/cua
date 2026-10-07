import live as l,asyncio,json,sys,inspect,time,os
class Reference:
 def __init__(self,helper):
  self.helper=helper;self.wire=l.m.h.MCP([sys.executable,'-m',os.environ['REFERENCE_MODULE'],'mcp'],'reference');self.wire.server_info={**self.wire.server_info,'name':'reference-driver'};self.name='reference';self.calls=helper.calls;self.schemas=helper.schemas
 async def observe(self,f,w):
  self.last=l.m.h.observe(self.wire,f,w)
  for e in self.last.get('elements',[]):e['element_token']=e['id']
  return self.last
 def request(self,*a):return self.helper.request(*a)
 def call(self,name,**args):
  if name!='set_value':return self.helper.call(name,**args)
  r,err=self.wire.call('act',snapshot=self.last['snapshot'],element=args['element_token'],action='SET_VALUE',value=args['value'],settle=True)
  return r,err or r.get('status')!='done'
 def close(self):self.wire.close()
source=inspect.getsource(l.trial).replace("not m.evidence(b,f.pid,w)['record_a_visible']", "not any('Record A' in (e.get('name'),e.get('label'),e.get('value')) for e in b.get('elements',[]))")
exec(source,l.__dict__)
async def main():
 versions=l.preflight();helper=l.Native('evidence');reference=Reference(helper);sentinel=l.m.h.Fixture();original=l.fresh_front();out={'versions':versions,'server':reference.wire.server_info,'rows':[]}
 try:
  if reference.wire.server_info['version']!=versions['reference_version']:raise RuntimeError('Reference server version mismatch')
  for rep in range(4):
   l.preflight();row=await l.trial(reference,'evidence','stable',rep,sentinel);row['mode']='reference_ack';row['warmup']=rep==0;out['rows'].append(row);print(json.dumps({k:row.get(k) for k in ('decision_ms','error','strict_pass','foreground_preserved')}),flush=True)
   if not row['strict_pass'] or not row['foreground_preserved'] or row.get('competing_input_detected'):raise RuntimeError('Reference qualification or interference failure')
 finally:
  helper.call('bring_to_front',pid=original);out['cleanup_restored']=l.fresh_front()==original;sentinel.close();reference.close();helper.close();(l.OUT/'reference-reference.json').write_text(json.dumps(out,indent=2))
asyncio.run(main())
