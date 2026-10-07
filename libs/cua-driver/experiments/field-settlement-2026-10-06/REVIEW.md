# Execution review

Aim: finish waiting for a native field's reaction and quiet value earlier, while keeping delayed-focus protection owned separately. Scope: an opt-in argument on the experimental macOS dispatch operation; timing decisions live in the common core. On-screen native text fields only. Application commitment and full protection completion remain separate.

The rejected shortcut is a shorter global detector timeout: that also shortens protection. The chosen operation leaves the original snapshot and protection lease with the runtime owner before starting its read-only field wait. Cancelling that wait therefore does not cancel the already-owned observer. No permanent wildcard watcher, generic save-completion assertion or stable-default change is introduced.

| Risk | Evidence or remaining gate | Disposition |
| --- | --- | --- |
| Immediate value echo bypasses quiet time | Common tracker requires the entire quiet interval and resets it on subsequent changes; zero-quiet mutation fails two keeper tests | Retired for timing state machine |
| No reaction or a no-op is falsely called a reaction | Exact common-state assertions; native prior-candidate no-op and absent-reaction controls pass | Core retired; final native requalification pending |
| Wrong stable value or endless changes report success | Exact mismatch and timeout assertions; both native prior-candidate controls refuse | Core retired; final native requalification pending |
| Missing/closed native field remains falsely valid | Native control exposed the stale-field failure; added fresh on-screen-window guard before mutation and during settling | Fix implemented; native re-test required |
| Delayed activation is unprotected | Original owner/snapshot is unchanged; prior-candidate activation during settling interrupts; activation after early return and final-candidate disconnect remain to run | Pending native qualification |
| User input leads to stale focus restoration | Existing owner/physical-counter tests remain in the full local gate; field wait also requires fresh physical-input and foreground evidence | Common/native policy covered offline; guest interference qualification pending |
| Field settlement is mistaken for a committed application transaction | Output keeps application commitment unverified; late rejection, missing acknowledgement and wrong-record scripts require independent transaction evidence before dependent input | Contract preserved; new native integration controls pending |
| Cancelled waiter destroys protection | Transfer precedes field waiting; existing owner cancellation/lease-drop tests cover ownership; read-only blocking probe cannot issue input | Owner contract retired offline; native disconnect remains pending |

The weakest assumption is that quiet AX field values are sufficient for callers' needs. They are sufficient only for a field-settlement report. Applications may reject later or expose AX echoes; generic transaction commitment remains unsupported. Adding this flag to a model does not itself establish an agent-performance gain. The matched comparison must measure actual successful task time and keep the full fence separate.

Review status: experimental draft. Local code gates can be completed without desktop input. Native gates remain blocked on an isolated macOS guest after the user's host-isolation constraint. Do not mark this ready or describe the speed gain as measured until those gates pass. The canonical desktop matrix and RFC decision remain separate upstream requirements.
