import copy,re,sys,unittest
from pathlib import Path
from tree_geometry import enrich_tree,TreeGeometryDriver,TreeGeometryHandoff
sys.path.insert(0,str(Path(__file__).resolve().parents[2]/'examples/jev-use/python'))
from native import NativeObservation
from sources import NativeAccessibilitySource

def fixture():
    root={'element_index':0,'element_token':'s1:0','role':'AXWindow','label':'Owned','frame':{'x':0,'y':0,'w':400,'h':300}}
    field={'element_index':1,'element_token':'s1:1','role':'AXTextField','label':'Email','value':'','parent_index':0,'enabled':True,'frame':{'x':20,'y':30,'w':150,'h':25}}
    snapshot={'pid':7,'window_id':9,'snapshot_id':'s1','elements':[root,field],'elements_complete':False,'truncated':False,'tree_markdown':'Record A\nEmail','background_input':{'exact_window':{'pid':7,'window_id':9,'status':'matched'}}}
    listed={'windows':[{'pid':7,'window_id':9,'is_on_screen':True,'bounds':{'x':0,'y':0,'width':400,'height':300}}]}
    return snapshot,listed
class Tests(unittest.TestCase):
    def test_preserves_fresh_binding_ancestry_and_record_context_without_capture(self):
        s,w=fixture();out=enrich_tree(s,w,pid=7,window_id=9)
        self.assertEqual(out['elements'],s['elements']);self.assertEqual(out['tree_markdown'],s['tree_markdown'])
        self.assertNotIn('capture_id',out);self.assertNotIn('screenshot_frame_valid',out)
        source=NativeAccessibilitySource.from_observation(NativeObservation.from_window_state(out,expected_pid=7,expected_window_id=9),'macos')
        self.assertIsNotNone(source.find('text_input','Email'))
    def test_offscreen_rejection_remains_active(self):
        s,w=fixture();s['elements'][1]['frame']['x']=800
        out=enrich_tree(s,w,pid=7,window_id=9)
        source=NativeAccessibilitySource.from_observation(NativeObservation.from_window_state(out,expected_pid=7,expected_window_id=9),'macos')
        self.assertIsNone(source.find('text_input','Email'))
    def test_same_label_duplicates_are_not_collapsed(self):
        s,w=fixture();duplicate={**s['elements'][1],'element_index':2,'element_token':'s1:2'};s['elements'].append(duplicate)
        source=NativeAccessibilitySource.from_observation(NativeObservation.from_window_state(enrich_tree(s,w,pid=7,window_id=9),expected_pid=7,expected_window_id=9),'macos')
        self.assertIsNone(source.find('text_input','Email'))
    def test_owner_geometry_and_capture_shortcuts_fail(self):
        edits=[
            (lambda s,w:s.update(pid=8),'Snapshot owner/identity mismatch'),
            (lambda s,w:s['background_input']['exact_window'].update(status='ax_unresolved'),'Window resolution is unproven'),
            (lambda s,w:w['windows'][0].update(pid=8),'Fresh WindowServer owner/visibility mismatch'),
            (lambda s,w:w['windows'][0].update(is_on_screen=False),'Fresh WindowServer owner/visibility mismatch'),
            (lambda s,w:w['windows'][0]['bounds'].update(x=4),'Window moved or frame sources disagree'),
            (lambda s,w:s['elements'][0]['frame'].update(w=0),'Empty geometry'),
            (lambda s,w:s['elements'][0].update(element_token='old:0'),'Window root is not bound to this snapshot'),
            (lambda s,w:s.update(capture_id='invented'),'Unexpected capture claim in tree-only snapshot'),
            (lambda s,w:s['elements'].append(copy.deepcopy(s['elements'][0])),'Missing/ambiguous AX window root'),
        ]
        for edit,reason in edits:
            s,w=fixture();edit(s,w)
            with self.subTest(reason=reason),self.assertRaisesRegex(TreeGeometryHandoff,'^'+re.escape(reason)+'$'):enrich_tree(s,w,pid=7,window_id=9)

class AsyncTests(unittest.IsolatedAsyncioTestCase):
    async def test_extra_fresh_metadata_read_is_mandatory_and_input_not_retried(self):
        class Driver:
            def __init__(self):self.calls=[]
            async def call(self,name,args):
                self.calls.append((name,args));s,w=fixture()
                if name=='get_window_state':return s
                w['windows'][0]['pid']=8;return w
        raw=Driver();driver=TreeGeometryDriver(raw)
        with self.assertRaisesRegex(TreeGeometryHandoff,'^Fresh WindowServer owner/visibility mismatch$'):await driver.call('get_window_state',{'pid':7,'window_id':9,'include_screenshot':True})
        self.assertEqual([c[0] for c in raw.calls],['get_window_state','list_windows']);self.assertFalse(raw.calls[0][1]['include_screenshot']);self.assertEqual(driver.receipts,[])
if __name__=='__main__':unittest.main()
