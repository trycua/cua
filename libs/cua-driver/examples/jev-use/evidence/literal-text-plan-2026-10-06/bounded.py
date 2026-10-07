"""Experimental caller-owned literal steps; no natural-language planner or driver hook."""
from dataclasses import dataclass,replace
import time
import importlib, os
_reference_module = importlib.import_module(os.environ['REFERENCE_MODULE'] + '')
Subtask = getattr(_reference_module, 'Subtask')
import importlib, os
_reference_module = importlib.import_module(os.environ['REFERENCE_MODULE'] + '.models')
ActionKind = getattr(_reference_module, 'ActionKind')
ExecutableAction = getattr(_reference_module, 'ExecutableAction')
import importlib, os
_reference_module = importlib.import_module(os.environ['REFERENCE_MODULE'] + '.validation')
materialize_action = getattr(_reference_module, 'materialize_action')

@dataclass(frozen=True)
class LiteralStep:
    label:str
    role:str
    kind:ActionKind
    value:str|None=None
    input_key:str='value'

class Handoff(RuntimeError):pass

def perform(backend,steps,verify_step,verify_all,*,policy=None,timeout_s=30,decision_log=None):
    """One delegation, at most three single-use inputs, caller-owned independent oracles."""
    if not 1<=len(steps)<=3:raise Handoff('Step budget exceeded')
    for step in steps:
        if not isinstance(step,LiteralStep) or step.kind not in (ActionKind.SET_VALUE,ActionKind.CLICK):raise Handoff('Unsupported literal step')
        if not step.label or not step.role:raise Handoff('Missing explicit selector')
        if step.kind==ActionKind.SET_VALUE and not isinstance(step.value,str):raise Handoff('Missing caller literal')
        if step.kind==ActionKind.CLICK and step.role!='CheckBox':raise Handoff('Only an explicit checkbox enable is permitted')
    deadline=time.monotonic()+timeout_s;records=[]
    for step in steps:
        if time.monotonic()>=deadline:raise Handoff('Deadline exceeded')
        snapshot=backend.observe()
        candidates=[e for e in snapshot.elements if e.name==step.label and e.role==step.role and e.enabled and e.visible and step.kind in e.actions]
        if len(candidates)!=1:raise Handoff('Selector is not uniquely actionable')
        target=candidates[0]
        if step.kind==ActionKind.CLICK and target.value!=0:raise Handoff('Checkbox enable precondition is not unchecked')
        decision=None
        if policy:
            # Caller has already resolved a single complete allowed action. JEV is
            # an intentionally redundant matched control, not additional authority.
            projected=replace(snapshot,elements=(replace(target,actions=(step.kind,)),))
            task=Subtask(goal=(f'Click the unchecked {step.label} checkbox once to check it.' if step.kind==ActionKind.CLICK else f'Set {step.label} to the supplied value.'),inputs={} if step.value is None else {step.input_key:step.value},constraints=('Perform only the single offered action; do not submit.',),verification=((f'{step.label} checkbox is checked (value 1)' if step.kind==ActionKind.CLICK else f'{step.label} equals the supplied value'),),max_actions=1)
            decision=policy.decide(subtask=task,snapshot=projected,history=[])
            if decision_log is not None:decision_log.append({'label':step.label,'kind':decision.kind.value if decision.kind else None,'terminal':decision.terminal.value if decision.terminal else None,'target':decision.target_id,'input_key':decision.input_key,'confidence':decision.confidence})
            if decision.terminal is not None or decision.target_id!=target.id or decision.kind!=step.kind:raise Handoff('Chooser did not select the sole authorized action')
            action=materialize_action(decision,projected,task)
            expected=ExecutableAction(kind=step.kind,target_id=target.id,target_guard=target.semantic_guard(),value=step.value)
            if action!=expected:raise Handoff('Chooser changed the complete allowed action')
        else:
            action=ExecutableAction(kind=step.kind,target_id=target.id,target_guard=target.semantic_guard(),value=step.value)
        if time.monotonic()>=deadline:raise Handoff('Deadline exceeded before input')
        if not backend.is_fresh(snapshot,action):raise Handoff('Binding changed before input')
        backend.execute(snapshot,action) # no uncertain input retry
        if not verify_step(step):raise Handoff('Independent postcondition failed')
        records.append({'label':step.label,'verified':True,'decision_kind':decision.kind.value if decision else None})
    if not verify_all():raise Handoff('Independent final outcome failed')
    return {'terminal':'verified_complete','steps':records}
