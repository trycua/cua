import sys,time,json,os
from pathlib import Path

from fixture_base import EvalForm,AppKit,Foundation,objc
class TransactionForm(EvalForm):
 def build(self):
  objc.super(TransactionForm,self).build();self.transactions={};self.plans={};self.generation=0;self.activation_requests=0
 def state(self):
  return {**objc.super(TransactionForm,self).state(),'transactions':json.loads(json.dumps(self.transactions)),'pid':os.getpid(),'window_id':int(self.window.windowNumber()),'generation':self.generation,'activation_requests':self.activation_requests,'window_visible':bool(self.window.isVisible())}
 def tick_(self,timer):
  control=self.path.with_suffix('.txn')
  if control.exists():
   req=json.loads(control.read_text());control.unlink();self.plans[req['tx']]={**req,'seen':None};self.transactions[req['tx']]={'status':'armed'}
  now=time.monotonic()
  for tx,p in self.plans.items():
   field=getattr(self,p['field']);value=str(field.stringValue())
   if p['seen'] is None and value==p['value']:p['seen']=now
   if p['seen'] is not None and p.get('noisy') and now-p['seen']<2.5:
    field.setStringValue_(p['value'] if int((now-p['seen'])/.06)%2==0 else 'CHANGING')
   if p['seen'] is not None and now-p['seen']>=p['delay'] and self.transactions[tx]['status']=='armed':
    if p.get('activate') and not p.get('activated'):
     p['activated']=True;self.activation_requests+=1;AppKit.NSApplication.sharedApplication().activateIgnoringOtherApps_(True)
    if p.get('close_window'):self.window.close()
    if p.get('no_reaction'):field.setStringValue_('')
    if p.get('missing'):continue
    if p.get('reject'):field.setStringValue_('REJECTED')
    status='rejected' if p.get('reject') else 'committed'
    self.transactions[tx]={'status':status,'tx':tx,'field':p['field'],'value':str(field.stringValue()),'record':str(self.record_label.stringValue()),'generation':self.generation,'pid':os.getpid(),'window_id':int(self.window.windowNumber())}
    if p.get('wrong_tx'):self.transactions[tx]['tx']='another-transaction'
    if p.get('record_change'):
     self.generation+=1;self.record_label.setStringValue_('Record B')
  objc.super(TransactionForm,self).tick_(timer)
if __name__=='__main__':
 app=AppKit.NSApplication.sharedApplication();app.setActivationPolicy_(AppKit.NSApplicationActivationPolicyRegular)
 form=TransactionForm.alloc().initWithPath_rows_(sys.argv[1],0);form.build();form.buildMenu();app.run()
