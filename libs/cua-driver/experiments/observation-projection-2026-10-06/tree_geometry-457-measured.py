"""Research-only screenshot omission with independently refreshed owner/geometry.

No Driver API change; no synthetic capture identity. The extra list_windows call
is mandatory and counted. Missing/mismatched metadata causes handoff.
"""
from __future__ import annotations
import math
from typing import Any

class TreeGeometryHandoff(RuntimeError): pass

def rectangle(value: Any, *, ax=False) -> tuple[float,float,float,float]:
    if not isinstance(value,dict): raise TreeGeometryHandoff('Missing geometry')
    keys=('x','y','w','h') if ax else ('x','y','width','height')
    raw=[value.get(k) for k in keys]
    if any(type(v) not in (int,float) or not math.isfinite(v) for v in raw):
        raise TreeGeometryHandoff('Invalid geometry')
    x,y,w,h=map(float,raw)
    if w<=0 or h<=0: raise TreeGeometryHandoff('Empty geometry')
    return x,y,w,h

def enrich_tree(snapshot:dict[str,Any], listed:dict[str,Any], *,pid:int,window_id:int) -> dict[str,Any]:
    if type(pid) is not int or type(window_id) is not int or pid<=0 or window_id<=0 or not isinstance(snapshot,dict) or not isinstance(listed,dict):
        raise TreeGeometryHandoff('Invalid owner arguments or observation payload')
    if type(snapshot.get('pid')) is not int or type(snapshot.get('window_id')) is not int:
        raise TreeGeometryHandoff('Invalid snapshot owner types')
    if snapshot.get('pid')!=pid or snapshot.get('window_id')!=window_id or not isinstance(snapshot.get('snapshot_id'),str) or not snapshot['snapshot_id']:
        raise TreeGeometryHandoff('Snapshot owner/identity mismatch')
    background=snapshot.get('background_input')
    if not isinstance(background,dict):raise TreeGeometryHandoff('Missing exact-window metadata')
    exact=background.get('exact_window')
    if exact!={'pid':pid,'window_id':window_id,'status':'matched'}:
        raise TreeGeometryHandoff('Window resolution is unproven')
    raw_windows=listed.get('windows');elements=snapshot.get('elements')
    if not isinstance(raw_windows,list) or any(not isinstance(w,dict) for w in raw_windows) or not isinstance(elements,list) or any(not isinstance(e,dict) for e in elements):
        raise TreeGeometryHandoff('Invalid window/element metadata')
    windows=[w for w in raw_windows if w.get('window_id')==window_id]
    if len(windows)!=1 or type(windows[0].get('pid')) is not int or windows[0].get('pid')!=pid or windows[0].get('is_on_screen') is not True:
        raise TreeGeometryHandoff('Fresh WindowServer owner/visibility mismatch')
    roots=[e for e in elements if e.get('role')=='AXWindow' and e.get('parent_index') is None]
    if len(roots)!=1: raise TreeGeometryHandoff('Missing/ambiguous AX window root')
    root=roots[0]
    if not isinstance(root.get('element_token'),str) or not root['element_token'].startswith(snapshot['snapshot_id']+':'):
        raise TreeGeometryHandoff('Window root is not bound to this snapshot')
    expected=rectangle(windows[0].get('bounds'));observed=rectangle(root.get('frame'),ax=True)
    if any(abs(a-b)>0.5 for a,b in zip(expected,observed)):
        raise TreeGeometryHandoff('Window moved or frame sources disagree')
    if snapshot.get('capture_id') or snapshot.get('screenshot_frame_valid') or snapshot.get('screenshot_scale'):
        raise TreeGeometryHandoff('Unexpected capture claim in tree-only snapshot')
    # Preserve every element, token, action, ancestry edge and Markdown context.
    result=dict(snapshot)
    x,y,width,height=expected
    result['window_bounds']={'x':x,'y':y,'width':width,'height':height}
    return result

class TreeGeometryDriver:
    def __init__(self,driver):self.driver=driver;self.receipts=[]
    async def call(self,name:str,arguments:dict[str,Any]):
        if name!='get_window_state': return await self.driver.call(name,arguments)
        pid,window_id=arguments['pid'],arguments['window_id']
        requested={**arguments,'include_accessibility_tree':True,'include_screenshot':False}
        if requested.get('screenshot_out_file'):
            raise TreeGeometryHandoff('File capture is not a tree-only observation')
        snapshot=await self.driver.call(name,requested)
        listed=await self.driver.call('list_windows',{'pid':pid})
        result=enrich_tree(snapshot,listed,pid=pid,window_id=window_id)
        self.receipts.append({'snapshot_id':snapshot['snapshot_id'],'pid':pid,'window_id':window_id,'bounds_verified':True,'extra_metadata_reads':1})
        return result
