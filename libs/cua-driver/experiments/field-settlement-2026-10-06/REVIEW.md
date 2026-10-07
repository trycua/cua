# Execution review

Aim: finish waiting for a native field's reaction and quiet value earlier, while keeping delayed-focus protection owned separately. Scope: an opt-in argument on the experimental macOS dispatch operation; timing decisions live in the common core. On-screen native text fields only. Application commitment and full protection completion remain separate.

The rejected shortcut is a shorter global detector timeout: that also shortens protection. The chosen operation leaves the original snapshot and protection lease with the runtime owner before starting its read-only field wait. Cancelling that wait therefore does not cancel the already-owned observer. No permanent wildcard watcher, generic save-completion assertion or stable-default change is introduced.

| Risk | Evidence or remaining gate | Disposition |
| --- | --- | --- |
| Immediate value echo bypasses quiet time | Common tracker requires the entire quiet interval and resets it on subsequent changes; zero-quiet mutation fails two keeper tests | Retired for timing state machine |
| No reaction or a no-op is falsely called a reaction | Exact common-state assertions; final off-screen no-op and absent-reaction controls pass | Retired by final off-screen controls |
| Wrong stable value or endless changes report success | Exact mismatch and timeout assertions; both final native controls refuse | Retired by final off-screen controls |
| Missing/closed native field remains falsely valid | Native control exposed the stale-field failure; added WindowServer visibility and fresh application AXWindows membership during settling | Retired by final closed-window refusal and retained receipt |
| Delayed activation is unprotected | Original owner/snapshot is unchanged; prior-candidate activation during settling interrupts; final-candidate non-activating disconnect passed; deliberate delayed activation remains to run | Pending native qualification |
| User input leads to stale focus restoration | Existing owner/physical-counter tests remain in the full local gate; field wait also requires fresh physical-input and foreground evidence | Common/native policy covered offline; guest interference qualification pending |
| Field settlement is mistaken for a committed application transaction | Output keeps application commitment unverified; late rejection, missing acknowledgement and wrong-record scripts require independent transaction evidence before dependent input | Contract preserved; final off-screen transaction guards passed |
| Cancelled waiter destroys protection | Transfer precedes field waiting; existing owner cancellation/lease-drop tests cover ownership; read-only blocking probe cannot issue input | Owner contract retired offline; final off-screen disconnect passed |

The weakest assumption is that quiet AX field values are sufficient for callers' needs. They are sufficient only for a field-settlement report. Applications may reject later or expose AX echoes; generic transaction commitment remains unsupported. Adding this flag to a model does not itself establish an agent-performance gain. The matched comparison must measure actual successful task time and keep the full fence separate.

Review status: experimental draft. Final off-screen timing, field controls, transaction guards, receipt lifecycle and disconnect checks pass. Deliberate delayed activation and concurrent-input restoration and canonical desktop matrix remain outstanding. The virtual display is sufficient for ordinary field qualification but is not a separate input/focus session.

## Off-screen results and limitations

All twenty trials preserved foreground and detected no physical interference. Median verified task times were 2,512 ms synchronous, 464 ms owned, 855 ms settled and 493 ms reference. The full owned protection fence remained 1,498–1,684 ms. Field settlement improves ordinary synchronous latency while adding 391 ms over owned dispatch; no general agent-speed claim follows from this fixture. The session-wide physical-input guard still interrupts unrelated user typing. Previous interrupted and setup pilots remain excluded. All final display teardowns restored topology.
