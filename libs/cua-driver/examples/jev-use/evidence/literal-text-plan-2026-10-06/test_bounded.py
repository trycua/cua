import unittest
from bounded import LiteralStep,perform,Handoff
from arc_cua.models import ActionKind,DesktopElement,DesktopSnapshot,Decision

class Backend:
    def __init__(self,elements=None):
        self.elements=elements if elements is not None else (DesktopElement(id='e',role='TextField',name='Email',actions=(ActionKind.SET_VALUE,)),)
        self.actions=[];self.fresh=True
    def observe(self):return DesktopSnapshot(application='owned',window='owned',revision='r',elements=tuple(self.elements))
    def is_fresh(self,snapshot,action):return self.fresh
    def execute(self,snapshot,action):self.actions.append(action)

class Tests(unittest.TestCase):
    def setUp(self):self.step=LiteralStep('Email','TextField',ActionKind.SET_VALUE,'synthetic@example.invalid')
    def test_independent_failure_does_not_complete_or_retry(self):
        backend=Backend()
        with self.assertRaises(Handoff):perform(backend,[self.step],lambda s:False,lambda:True)
        self.assertEqual(len(backend.actions),1)
    def test_stale_binding_never_inputs(self):
        backend=Backend();backend.fresh=False
        with self.assertRaises(Handoff):perform(backend,[self.step],lambda s:True,lambda:True)
        self.assertEqual(backend.actions,[])
    def test_duplicate_selector_never_inputs(self):
        backend=Backend();backend.elements=backend.elements*2
        with self.assertRaises(Handoff):perform(backend,[self.step],lambda s:True,lambda:True)
        self.assertEqual(backend.actions,[])
    def test_model_modifier_cannot_expand_action(self):
        backend=Backend((DesktopElement(id='c',role='CheckBox',name='Subscribe',value=0,actions=(ActionKind.CLICK,)),))
        class Policy:
            def decide(self,**kwargs):return Decision(kind=ActionKind.CLICK,target_id='c',click_modifier='MOD')
        with self.assertRaises(Handoff):perform(backend,[LiteralStep('Subscribe','CheckBox',ActionKind.CLICK)],lambda s:True,lambda:True,policy=Policy())
        self.assertEqual(backend.actions,[])
    def test_final_oracle_required(self):
        backend=Backend()
        with self.assertRaises(Handoff):perform(backend,[self.step],lambda s:True,lambda:False)
    def test_budget_checked_before_any_input(self):
        backend=Backend()
        with self.assertRaises(Handoff):perform(backend,[self.step]*4,lambda s:True,lambda:True)
        self.assertEqual(backend.actions,[])
if __name__=='__main__':unittest.main()
