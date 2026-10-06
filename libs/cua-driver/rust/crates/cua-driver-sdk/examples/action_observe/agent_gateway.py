"""Fixture-scoped MCP gateway for actual tool-agent qualification, not production routing."""
import json, os, sys, time, signal
from pathlib import Path
from shared import MCP, Fixture, wait_for

VALUES = {'Full name': 'Synthetic Person', 'Email': 'synthetic@example.invalid', 'Subscribe': None}

def projected(snapshot):
    return {k: snapshot[k] for k in ('pid', 'window_id', 'snapshot_id') if k in snapshot} | {
        'elements': [e for e in snapshot.get('elements', []) if e.get('label', e.get('name')) in VALUES and e.get('role') in ('AXTextField','AXCheckBox')]}

def validate_action(tool, action, snapshot, pid, window, attempted):
    if action.get('pid')!=pid or action.get('window_id')!=window: raise ValueError('wrong owned target')
    if set(action)-{'pid','window_id','element_token','value'}: raise ValueError('unsupported action arguments')
    candidates=[e for e in projected(snapshot)['elements'] if e.get('element_token')==action.get('element_token')]
    if len(candidates)!=1: raise ValueError('token not from latest observation')
    label=candidates[0].get('label',candidates[0].get('name'))
    if label in attempted: raise ValueError('input already attempted; no replay')
    expected_tool='click' if VALUES[label] is None else 'set_value'
    if tool!=expected_tool or (VALUES[label] is not None and action.get('value')!=VALUES[label]) or (VALUES[label] is None and 'value' in action): raise ValueError('not one of the permitted literal actions')
    return label

def verified_snapshot(snapshot):
    if not isinstance(snapshot,dict):return False
    elements=projected(snapshot)['elements']
    for label,value in VALUES.items():
        matches=[e for e in elements if e.get('label',e.get('name'))==label]
        if len(matches)!=1:return False
        if value is None:
            if matches[0].get('selected') is not True or matches[0].get('value')!='1':return False
        elif matches[0].get('value')!=value:return False
    return True

def owned_window(client, pid):
    def discover():
        data,error=client.call('list_windows',pid=pid,on_screen_only=True)
        if error: return None
        matches=[w for w in data.get('windows',[]) if w.get('pid')==pid and w.get('title')=='Arc Bench Form' and w.get('is_on_screen') is True and isinstance(w.get('window_id'),int) and w['window_id']>0]
        if len(matches)!=1: return None
        window=matches[0]['window_id']
        snapshot,error=client.call('get_window_state',pid=pid,window_id=window,include_screenshot=False)
        if error or snapshot.get('pid')!=pid or snapshot.get('window_id')!=window or not snapshot.get('snapshot_id'): return None
        offered=projected(snapshot)['elements']
        if any(sum(e.get('label',e.get('name'))==label and bool(e.get('element_token')) for e in offered)!=1 for label in VALUES):return None
        return window
    return wait_for(discover,8)

def run():
    condition = os.environ['ACTION_OBSERVE_CONDITION']
    output = Path(os.environ['ACTION_OBSERVE_AGENT_EVIDENCE'])
    client = fixture = None
    rows = []; setup_calls=[]; snapshot = None; completed = set(); attempted = set()
    try:
        client = MCP([os.environ['ACTION_OBSERVE_BINARY']], 'scoped-agent')
        fixture = Fixture(); window = owned_window(client,fixture.pid)
        setup_calls=list(client.calls)
        action_names = ['set_value','click'] if condition == 'baseline' else ['experiment_action_observe']
        tools = [{'name':'get_window_state','description':'Observe this owned synthetic form. Exact pid/window and current element tokens are returned. Independently verify each change through another observation.', 'inputSchema':{'type':'object','properties':{},'additionalProperties':False}}]
        schema = {'type':'object','properties':{'pid':{'type':'integer'},'window_id':{'type':'integer'},'element_token':{'type':'string'},'value':{'type':'string'}},'required':['pid','window_id','element_token'],'additionalProperties':False}
        for name in action_names:
            tools.append({'name':name,'description':'One currently observed exact synthetic element action; gateway enforces owned window, current token, permitted literal value, and at most one attempt per field. Composite returns a fresh same-window observation too.', 'inputSchema':schema if name != 'experiment_action_observe' else {'type':'object','properties':{'tool':{'type':'string','enum':['set_value','click']},'arguments':schema},'required':['tool','arguments'],'additionalProperties':False}})
        for line in sys.stdin:
            request=json.loads(line); ident=request.get('id')
            if ident is None: continue
            method=request.get('method'); result={}
            if method=='initialize': result={'protocolVersion':'2025-06-18','capabilities':{'tools':{}},'serverInfo':{'name':'owned-fixture-gateway','version':'1'}}
            elif method=='tools/list': result={'tools':tools}
            elif method=='tools/call':
                name=request['params']['name']; args=request['params'].get('arguments',{}); started=time.perf_counter()
                dispatches=[]
                try:
                    if name=='get_window_state':
                        if args: raise ValueError('observation accepts no caller target overrides')
                        dispatches.append(name)
                        read,error=client.call(name,pid=fixture.pid,window_id=window,include_screenshot=False)
                        if error: raise ValueError('observation failed')
                        if read.get('pid')!=fixture.pid or read.get('window_id')!=window: raise ValueError('observation owner mismatch')
                        snapshot=read; data=projected(read)
                    else:
                        if name not in action_names or snapshot is None: raise ValueError('unobserved or unsupported input')
                        action=args['arguments'] if name=='experiment_action_observe' else args
                        tool=args['tool'] if name=='experiment_action_observe' else name
                        label=validate_action(tool,action,snapshot,fixture.pid,window,attempted)
                        attempted.add(label)
                        child,error=client.call(name,**args)
                        dispatches=child.get('child_dispatches',[]) if name=='experiment_action_observe' and isinstance(child,dict) else [name]
                        snapshot=None
                        if error: raise ValueError('child failed; inspect effects before any retry')
                        if name=='experiment_action_observe':
                            if not child.get('observation_available'): raise ValueError('postread unavailable; input must not be repeated')
                            read=child['observation_result']['structuredContent']
                            if read.get('pid')!=fixture.pid or read.get('window_id')!=window: raise ValueError('postread owner mismatch')
                            snapshot=read
                            action_receipt=child['action_result']
                            data={**child,'action_result':{'isError':action_receipt.get('isError',False),'structuredContent':action_receipt.get('structuredContent')},'observation_result':{'structuredContent':projected(read)}}
                        else: data=child
                        state=fixture.state(); key={'Full name':'name','Email':'email','Subscribe':'subscribe'}[label]; expected=True if VALUES[label] is None else VALUES[label]
                        wait_for(lambda:fixture.state().get(key)==expected,3); completed.add(label)
                    result={'content':[{'type':'text','text':json.dumps(data)}],'structuredContent':data}
                except (ValueError,KeyError,TypeError,TimeoutError):
                    result={'isError':True,'content':[{'type':'text','text':'Scoped gateway refused or effect uncertain; observe and stop if incomplete. No retry authorized.'}]}
                rows.append({'tool':name,'ms':(time.perf_counter()-started)*1000,'error':bool(result.get('isError')),'child_dispatches':dispatches})
            elif method=='ping': result={}
            else: result={'isError':True,'content':[{'type':'text','text':'unsupported gateway method'}]}
            print(json.dumps({'jsonrpc':'2.0','id':ident,'result':result}),flush=True)
    finally:
        state=fixture.state() if fixture else {}
        evidence={'condition':condition,'setup_calls':setup_calls,'calls':rows,'driver_server':client.server_info if client else None,'driver_metadata':getattr(client,'backend_metadata',None) if client else None,'final_observation_verified':verified_snapshot(snapshot),'passed':verified_snapshot(snapshot) and all(state.get(k)==v for k,v in {'name':VALUES['Full name'],'email':VALUES['Email'],'subscribe':True,'submitted':0}.items()),'oracle_state':{k:state.get(k) for k in ('name','email','subscribe','submitted')},'completed_fields':sorted(completed)}
        try:
            if client: client.close()
        finally:
            if fixture: fixture.close()
            output.write_text(json.dumps(evidence,indent=2)+'\n')

if __name__=='__main__':
    def shutdown(signum, frame): raise SystemExit(0)
    signal.signal(signal.SIGTERM,shutdown)
    signal.signal(signal.SIGINT,shutdown)
    run()
