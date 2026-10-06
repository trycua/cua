import unittest, io, json, os, tempfile
from unittest.mock import patch
from pathlib import Path
import agent_gateway
from agent_gateway import validate_action

class BindingTests(unittest.TestCase):
    def setUp(self):
        self.snapshot={'elements':[{'label':'Full name','role':'AXTextField','element_token':'fresh:1'}]}
        self.args={'pid':7,'window_id':9,'element_token':'fresh:1','value':'Synthetic Person'}
    def test_exact_allowed_current_input(self):
        self.assertEqual(validate_action('set_value',self.args,self.snapshot,7,9,set()),'Full name')
    def test_wrong_owner_window_stale_token_literal_extra_and_replay_refused(self):
        for changed in ({'pid':8},{'window_id':10},{'element_token':'old:1'},{'value':'Other'},{'session':'override'}):
            with self.assertRaises(ValueError):validate_action('set_value',{**self.args,**changed},self.snapshot,7,9,set())
        with self.assertRaises(ValueError):validate_action('set_value',self.args,self.snapshot,7,9,{'Full name'})
        with self.assertRaises(ValueError):validate_action('click',self.args,self.snapshot,7,9,set())
    def test_duplicate_or_unoffered_token_refused(self):
        with self.assertRaises(ValueError):validate_action('set_value',self.args,{'elements':self.snapshot['elements']*2},7,9,set())
        with self.assertRaises(ValueError):validate_action('set_value',self.args,{'elements':[]},7,9,set())

class DispatchTests(unittest.TestCase):
    def test_invalid_inputs_do_not_reach_driver_and_valid_input_only_once(self):
        calls=[]
        class Client:
            server_info={'version':'0.34.0'}
            calls=[]
            def __init__(self,*a):pass
            def call(self,name,**args):
                calls.append(name)
                if name=='get_window_state': return {'pid':7,'window_id':9,'snapshot_id':'fresh','elements':[{'label':'Full name','role':'AXTextField','element_token':'fresh:1'}]},False
                return {'status':'acknowledged'},False
            def close(self):pass
        class Fixture:
            pid=7
            def state(self):return {'name':'Synthetic Person'}
            def close(self):pass
        args={'pid':7,'window_id':9,'element_token':'fresh:1','value':'Synthetic Person'}
        requests=[{'name':'get_window_state','arguments':{}}]
        requests += [{'name':'set_value','arguments':{**args,**change}} for change in ({'pid':8},{'window_id':10},{'element_token':'old:1'},{'value':'Wrong'},{'session':'bypass'})]
        requests += [{'name':'set_value','arguments':args},{'name':'get_window_state','arguments':{}},{'name':'set_value','arguments':args}]
        wire=''.join(json.dumps({'id':i,'method':'tools/call','params':request})+'\n' for i,request in enumerate(requests))
        with tempfile.TemporaryDirectory() as tmp, patch.dict(os.environ,{'ACTION_OBSERVE_CONDITION':'baseline','ACTION_OBSERVE_AGENT_EVIDENCE':str(Path(tmp)/'evidence.json'),'ACTION_OBSERVE_BINARY':'unused'}),patch.object(agent_gateway,'MCP',Client),patch.object(agent_gateway,'Fixture',Fixture),patch.object(agent_gateway,'owned_window',lambda client,pid:9),patch.object(agent_gateway.sys,'stdin',io.StringIO(wire)),patch.object(agent_gateway.sys,'stdout',io.StringIO()):
            agent_gateway.run()
        self.assertEqual(calls,['get_window_state','set_value','get_window_state'])

class WindowDiscoveryTests(unittest.TestCase):
    def test_public_inventory_ignores_shadow_and_requires_full_controls(self):
        class Client:
            def call(self,name,**args):
                if name=='list_windows':return {'windows':[{'pid':7,'title':'Arc Bench Form','window_id':3,'is_on_screen':False},{'pid':8,'title':'Arc Bench Form','window_id':4,'is_on_screen':True},{'pid':7,'title':'Arc Bench Form','window_id':9,'is_on_screen':True}]},False
                self.window=args['window_id']
                return {'pid':7,'window_id':9,'snapshot_id':'fresh','elements':[{'label':label,'role':'AXCheckBox' if label=='Subscribe' else 'AXTextField','element_token':'fresh:'+str(i)} for i,label in enumerate(agent_gateway.VALUES)]},False
        client=Client()
        with patch.object(agent_gateway,'wait_for',lambda fn,timeout:fn()):self.assertEqual(agent_gateway.owned_window(client,7),9)
        self.assertEqual(client.window,9)
    def test_empty_control_readiness_is_not_accepted(self):
        class Client:
            def call(self,name,**args):return ({'windows':[{'pid':7,'title':'Arc Bench Form','window_id':9,'is_on_screen':True}]} if name=='list_windows' else {'pid':7,'window_id':9,'snapshot_id':'fresh','elements':[]}),False
        with patch.object(agent_gateway,'wait_for',lambda fn,timeout:fn()):self.assertIsNone(agent_gateway.owned_window(Client(),7))

if __name__=='__main__':unittest.main()
