import copy
import unittest
from composite_driver import CompositeTextDriver
from run import DriverToolError

class Fake:
    def __init__(self, result): self.result,self.calls=result,[]
    async def call(self,name,args):self.calls.append((name,args));return copy.deepcopy(self.result)

def child(data, error=False):return {'isError':error,'content':[],'structuredContent':data}
def result():
    snapshot={'pid':7,'window_id':9,'snapshot_id':'after','capture_id':'after-cap','elements_complete':True,'truncated':False,'window_bounds':{'x':0,'y':0,'width':500,'height':400},'elements':[]}
    return {'observation_available':True,'action_result':child({'ok':True}),'observation_result':child(snapshot)}

class Tests(unittest.IsolatedAsyncioTestCase):
    async def test_input_once_read_failure_stops(self):
        payload=result();payload['observation_result']['isError']=True
        fake=Fake(payload);driver=CompositeTextDriver(fake,7,9)
        with self.assertRaises(DriverToolError):await driver.call('set_value',{'pid':7,'window_id':9,'element_token':'fresh','value':'literal'})
        self.assertEqual(len(fake.calls),1);self.assertEqual(driver.receipts,[])
    async def test_same_owner_postaction_read_required(self):
        payload=result();payload['observation_result']['structuredContent']['pid']=8
        fake=Fake(payload);driver=CompositeTextDriver(fake,7,9)
        with self.assertRaises(RuntimeError):await driver.call('set_value',{'pid':7,'window_id':9})
        self.assertEqual(len(fake.calls),1);self.assertEqual(driver.receipts,[])
    async def test_owner_mismatch_no_input(self):
        fake=Fake(result());driver=CompositeTextDriver(fake,7,9)
        with self.assertRaises(DriverToolError):await driver.call('set_value',{'pid':8,'window_id':9})
        self.assertEqual(fake.calls,[])
    async def test_refusal_not_overridden_by_wrapper_flags(self):
        payload=result();payload['action_result']=child({'status':'refused','refusal':{'code':'stale_element_token'}});payload['action_acknowledged']=True
        fake=Fake(payload);driver=CompositeTextDriver(fake,7,9)
        with self.assertRaises(DriverToolError) as caught:await driver.call('set_value',{'pid':7,'window_id':9})
        self.assertEqual(caught.exception.code,'stale_element_token');self.assertEqual(len(fake.calls),1)
    async def test_availability_and_snapshot_required(self):
        first=result();first['observation_available']=False
        second=result();second['observation_result']['structuredContent'].pop('snapshot_id')
        for payload in [first,second]:
            fake=Fake(payload);driver=CompositeTextDriver(fake,7,9)
            with self.assertRaises(DriverToolError):await driver.call('set_value',{'pid':7,'window_id':9})
            self.assertEqual(len(fake.calls),1)
    async def test_observations_are_not_cached(self):
        fake=Fake(result());driver=CompositeTextDriver(fake,7,9)
        await driver.call('set_value',{'pid':7,'window_id':9})
        await driver.call('get_window_state',{'pid':7,'window_id':9})
        self.assertEqual([x[0] for x in fake.calls],['experiment_action_observe','get_window_state'])

if __name__=='__main__':unittest.main()
