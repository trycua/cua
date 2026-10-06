from __future__ import annotations
import copy
import asyncio
import sys
import unittest
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from literal_text_plan import LiteralTextStep, LiteralPlanHandoff, execute_literal_text_plan


def observation(token="s1:1", *, value="", pid=7, duplicate=False, truncated=False):
    target={"element_index":1,"role":"AXTextField","label":"Email","value":value,"enabled":True,"parent_index":0,"element_token":token,"frame":{"x":10,"y":10,"w":80,"h":20}}
    elements=[{"element_index":0,"role":"AXWindow","label":"Main","frame":{"x":0,"y":0,"w":500,"h":400}},target]
    if duplicate:elements.append({**target,"element_index":2,"element_token":"s1:2"})
    return {"pid":pid,"window_id":9,"snapshot_id":token.split(':')[0],"capture_id":"cap1","elements_complete":True,"truncated":truncated,"window_bounds":{"x":0,"y":0,"width":500,"height":400},"elements":elements}


class Driver:
    def __init__(self, payloads):self.payloads=list(payloads);self.inputs=[];self.observations=0
    async def call(self,name,args):
        if name=="get_window_state":
            self.observations+=1
            return copy.deepcopy(self.payloads.pop(0))
        self.inputs.append((name,args))
        return {"ok":True}


class Tests(unittest.IsolatedAsyncioTestCase):
    async def run_plan(self,driver,*,step_ok=True,final_ok=True,context_ok=True,choose=None,steps=None):
        async def verify_step(step):return step_ok
        async def verify_final():return final_ok
        return await execute_literal_text_plan(driver,pid=7,window_id=9,platform="macos",steps=steps or [LiteralTextStep("Email","caller literal")],verify_step=verify_step,verify_final=verify_final,verify_context=lambda snapshot:context_ok,choose=choose)
    async def test_fresh_token_and_exact_caller_literal(self):
        driver=Driver([observation(),observation("s2:1")])
        result=await self.run_plan(driver)
        self.assertEqual(result["status"],"verified_complete")
        self.assertEqual(driver.inputs,[("set_value",{"pid":7,"window_id":9,"element_token":"s2:1","value":"caller literal"})])
    async def test_selector_changed_before_input(self):
        driver=Driver([observation(),observation("s2:1",value="other")])
        with self.assertRaises(LiteralPlanHandoff):await self.run_plan(driver)
        self.assertEqual(driver.inputs,[])
    async def test_duplicate_label_does_not_select_first(self):
        driver=Driver([observation(duplicate=True)])
        with self.assertRaises(LiteralPlanHandoff):await self.run_plan(driver)
        self.assertEqual(driver.inputs,[])
    async def test_wrong_owner_and_context_fail_closed(self):
        for payload,context in [(observation(pid=8),True),(observation(),False)]:
            driver=Driver([payload])
            with self.assertRaises(RuntimeError):await self.run_plan(driver,context_ok=context)
            self.assertEqual(driver.inputs,[])
    async def test_input_acknowledgement_is_not_completion(self):
        for step_ok,final_ok in [(False,True),(True,False)]:
            driver=Driver([observation(),observation("s2:1")])
            with self.assertRaises(LiteralPlanHandoff):await self.run_plan(driver,step_ok=step_ok,final_ok=final_ok)
            self.assertEqual(len(driver.inputs),1)
    async def test_truncated_tree_reobserves_once_then_hands_off(self):
        driver=Driver([observation(truncated=True),observation(truncated=True)])
        with self.assertRaises(LiteralPlanHandoff):await self.run_plan(driver)
        self.assertEqual(driver.observations,2);self.assertEqual(driver.inputs,[])
    async def test_unknown_model_id_never_inputs(self):
        async def choose(candidate,observation):return "invented"
        driver=Driver([observation()])
        with self.assertRaises(LiteralPlanHandoff):await self.run_plan(driver,choose=choose)
        self.assertEqual(driver.inputs,[])
    async def test_driver_error_stops_before_next_input(self):
        class RefusingDriver(Driver):
            async def call(self,name,args):
                result=await super().call(name,args)
                if name=="set_value":raise RuntimeError("structured refusal")
                return result
        driver=RefusingDriver([observation(),observation("s2:1")])
        with self.assertRaises(RuntimeError):await self.run_plan(driver,steps=[LiteralTextStep("Email","x"),LiteralTextStep("Name","y")])
        self.assertEqual(len(driver.inputs),1)
    async def test_timeout_after_input_does_not_retry_or_complete(self):
        class SlowDriver(Driver):
            async def call(self,name,args):
                result=await super().call(name,args)
                if name=="set_value":await asyncio.sleep(1)
                return result
        driver=SlowDriver([observation(),observation("s2:1")])
        async def verify(step):raise AssertionError("Timed-out input cannot authorize verification/completion")
        with self.assertRaises(TimeoutError):
            await execute_literal_text_plan(driver,pid=7,window_id=9,platform="macos",steps=[LiteralTextStep("Email","x")],verify_step=verify,verify_final=lambda:None,verify_context=lambda observation:True,timeout_s=.02)
        self.assertEqual(len(driver.inputs),1)
    async def test_budget_and_duplicate_plan_labels_before_observation(self):
        for steps in [[LiteralTextStep(str(i),"x") for i in range(4)],[LiteralTextStep("Email","x"),LiteralTextStep("Email","y")]]:
            driver=Driver([])
            with self.assertRaises(LiteralPlanHandoff):await self.run_plan(driver,steps=steps)
            self.assertEqual(driver.observations,0)

if __name__=="__main__":unittest.main()
