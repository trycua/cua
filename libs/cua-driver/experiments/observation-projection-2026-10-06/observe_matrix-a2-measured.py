"""Read-only owned-fixture matrix; all extra metadata calls and bytes counted."""
import asyncio,base64,hashlib,importlib.util,json,os,statistics,subprocess,sys,time
from contextlib import ExitStack
from pathlib import Path
from tree_geometry import TreeGeometryDriver,TreeGeometryHandoff
HERE=Path(__file__).resolve().parent
CUA=Path(os.environ['CUA_PROJECTION_SOURCE']);sys.path.insert(0,str(CUA/'libs/cua-driver/examples/jev-use/python'))
from native import NativeObservation
from sources import NativeAccessibilitySource
BASE=Path(os.environ['REFERENCE_FIXTURE_SOURCE'])
spec=importlib.util.spec_from_file_location('projection_fixture_owner',BASE/'run.py');h=importlib.util.module_from_spec(spec);sys.modules[spec.name]=h;spec.loader.exec_module(h)
class WireMCP(h.MCP):
    def request(self,method,params):
        result=super().request(method,params)
        if method=='initialize':self.backend_metadata=result.get('_meta',{}).get('driver_metadata')
        if method=='tools/call':
            images=[p for p in result.get('content',[]) if p.get('type')=='image']
            self.wire={'serialized_result_bytes':len(json.dumps(result,ensure_ascii=False,separators=(',',':')).encode()),'image_bytes':sum(len(base64.b64decode(p['data'])) for p in images),'images':len(images)}
        return result
    def call(self,name,**args):
        result,error=super().call(name,**args);self.calls[-1].update(self.wire);return result,error
class Driver:
    def __init__(self,client):self.client=client
    async def call(self,name,args):
        result,error=await asyncio.to_thread(self.client.call,name,**args)
        if error or result.get('status')=='refused' or result.get('refusal'):raise RuntimeError('Public Driver refusal')
        return result

def record_visible(snapshot,record):
    # Controlled fixture label; keep the full Markdown because StaticText is
    # intentionally absent from the actionable structured projection.
    return record in snapshot.get('tree_markdown','')
def evidence(snapshot,pid,window):
    observation=NativeObservation.from_window_state(snapshot,expected_pid=pid,expected_window_id=window)
    source=NativeAccessibilitySource.from_observation(observation,'macos')
    return {'required_fields_unique':all(source.find('text_input',label) is not None for label in ('Full name','Email')),'record_a_visible':record_visible(snapshot,'Record A'),'record_b_visible':record_visible(snapshot,'Record B'),'truncated':observation.truncated,'markdown_sha256':hashlib.sha256(snapshot.get('tree_markdown','').encode()).hexdigest(),'window_bounds':snapshot.get('window_bounds'),'semantic_elements':[{k:v for k,v in e.items() if k not in ('element_token','screenshot_frame')} for e in h.sanitize_result(snapshot).get('elements',[])]}
async def observe(client,fixture,window,mode):
    driver=Driver(client);offset=len(client.calls);start=time.perf_counter();row={'mode':mode,'qualified':False}
    try:
        reader=TreeGeometryDriver(driver) if mode in ('tree_geometry','query_email') else driver
        args={'pid':fixture.pid,'window_id':window,'include_accessibility_tree':True,'include_screenshot':mode!='tree_raw','timeout_ms':1000}
        if mode=='query_email':args['query']='Email'
        snapshot=await reader.call('get_window_state',args)
        row['evidence']=evidence(snapshot,fixture.pid,window)
        row['qualified']=row['evidence']['required_fields_unique'] and row['evidence']['record_a_visible'] and not row['evidence']['truncated']
        row['snapshot_id']=snapshot.get('snapshot_id');row['model_json_bytes']=len(json.dumps(snapshot,separators=(',',':')).encode())
    except Exception as exc:row['error_type']=type(exc).__name__;row['error']=str(exc)[:160]
    row['wall_ms']=(time.perf_counter()-start)*1000;row['calls']=client.calls[offset:];row['serialized_result_bytes']=sum(c['serialized_result_bytes'] for c in row['calls']);row['image_bytes']=sum(c['image_bytes'] for c in row['calls']);row['public_calls']=len(row['calls']);row['oracle_state']=fixture.state()
    return row
async def window_ready(client,fixture):
    driver=Driver(client)
    for _ in range(20):
        listed=await driver.call('list_windows',{'pid':fixture.pid})
        matches=[w for w in listed.get('windows',[]) if w.get('pid')==fixture.pid and w.get('title')=='Reference Bench Form' and w.get('is_on_screen') is True]
        if len(matches)==1:
            window=matches[0]['window_id'];row=await observe(client,fixture,window,'full')
            if row['qualified']:return window
        await asyncio.sleep(.1)
    raise RuntimeError('Fixture never became actionable')
def main():
    import Quartz
    if (Quartz.CGSessionCopyCurrentDictionary() or {}).get('CGSSessionScreenIsLocked'):raise RuntimeError('Locked session')
    subprocess.run(['git','-C',str(CUA),'fetch','origin','main'],check=True,capture_output=True)
    upstream=subprocess.check_output(['git','-C',str(CUA),'rev-parse','origin/main'],text=True).strip();subprocess.run(['git','-C',str(CUA),'merge-base','--is-ancestor',upstream,'HEAD'],check=True)
    versions=h.candidate_versions();out={**versions,'upstream_source':upstream,'source_checked_at':__import__('datetime').datetime.now(__import__('datetime').timezone.utc).isoformat(),'projection_source':subprocess.check_output(['git','-C',str(CUA),'rev-parse','HEAD'],text=True).strip(),'results':[],'controls':[]}
    arms=json.loads(os.environ.get('CUA_PROJECTION_ARMS','[{"name":"released","argv":["cua-driver","mcp"]}]'))
    repeats=int(os.environ.get('CUA_PROJECTION_REPS','20'));cases=os.environ.get('CUA_PROJECTION_CASES','small,large').split(',')
    with ExitStack() as stack:
        clients={}
        for arm in arms:
            client=WireMCP(arm['argv'],'projection-'+arm['name']);stack.callback(client.close);clients[arm['name']]=client
            if client.server_info['version']!=versions['native_version']:raise RuntimeError('Server version differs')
            metadata=getattr(client,'backend_metadata',None)
            if not isinstance(metadata,dict) or metadata.get('driver_version')!=versions['native_version'] or type(metadata.get('pid')) is not int:raise RuntimeError('Actual backend version/identity differs')
            manifest=json.loads(Path(arm['build_manifest']).read_text())
            actual_hash=hashlib.sha256(Path(arm['argv'][0]).read_bytes()).hexdigest()
            if manifest['binary_sha256']!=actual_hash or manifest['base_source']!=upstream:raise RuntimeError('Binary/source build receipt differs from latest source')
            subprocess.run(['git','-C',str(CUA),'merge-base','--is-ancestor',upstream,manifest['source_commit']],check=True)
            out.setdefault('servers',{})[arm['name']]={'server_info':client.server_info,'daemon_metadata':metadata,'argv':arm['argv'],'build_manifest':manifest,'binary_sha256':hashlib.sha256(Path(arm['argv'][0]).read_bytes()).hexdigest() if Path(arm['argv'][0]).is_file() else None}
        for case in cases:
            saved=h.HERE
            if case=='large':h.HERE=Path(os.environ['LARGE_FIXTURE_SOURCE'])
            fixture=None
            try:
                fixture=h.Fixture();window=asyncio.run(window_ready(next(iter(clients.values())),fixture))
                for rep in range(repeats+2):
                    combos=[(name,mode) for name in clients for mode in ('full','tree_geometry')]
                    if rep%2:combos.reverse()
                    for name,mode in combos:
                        row=asyncio.run(observe(clients[name],fixture,window,mode));row.update(case=case,arm=name,rep=rep-2,warmup=rep<2);out['results'].append(row)
                        matching=[r for r in out['results'] if r['case']==case and r['arm']==name and r['rep']==rep-2]
                        if len(matching)==2:
                            bymode={r['mode']:r for r in matching}
                            a,b=bymode['full'],bymode['tree_geometry']
                            same=a.get('evidence')==b.get('evidence') and a['qualified'] and b['qualified']
                            for item in matching:item['pair_semantics_equal']=same
                            if not same:
                                (HERE/'observation-results.json').write_text(json.dumps(out,indent=2)+'\n')
                                raise RuntimeError('Full/tree semantics or required proof differ; arm refused')
                        (HERE/'observation-results.json').write_text(json.dumps(out,indent=2)+'\n')
                    completed=[r for r in out['results'] if r['case']==case and r['rep']==rep-2]
                    canonical=next(r for r in completed if r['arm']==arms[0]['name'] and r['mode']=='full')
                    across=all(r.get('evidence')==canonical.get('evidence') and r['qualified'] for r in completed)
                    for item in completed:item['all_arm_semantics_equal']=across
                    if not across:
                        (HERE/'observation-results.json').write_text(json.dumps(out,indent=2)+'\n')
                        raise RuntimeError('Cross-arm semantics differ; matrix refused')
                for mode in ('tree_raw','query_email'):
                    row=asyncio.run(observe(next(iter(clients.values())),fixture,window,mode));row.update(case=case,control=mode);out['controls'].append(row)
                fixture.mutate('record')
                for mode in ('full','tree_geometry','query_email'):
                    row=asyncio.run(observe(next(iter(clients.values())),fixture,window,mode));row.update(case=case,control='record_changed');out['controls'].append(row)
            finally:
                if fixture:fixture.close()
                h.HERE=saved
            (HERE/'observation-results.json').write_text(json.dumps(out,indent=2)+'\n')
            print(json.dumps({'case':case,'rows':len(out['results']),'qualified':sum(r['qualified'] for r in out['results']),'controls':len(out['controls'])}),flush=True)
if __name__=='__main__':main()
