\* SPDX-License-Identifier: MIT
\* Copyright (c) 2026 Cua AI, Inc.
---- MODULE invoke_menu_cancel ----
\* A background invoke_menu call whose caller goes away (a timeout, or the
\* client drops the request) while its worker still runs:
\* run_in_background and run_admitted in platform-macos/src/tools/invoke_menu.rs, and the
\* desktop lane in cua-driver-core/src/tool.rs.
\*
\* Actors: the call (the dispatch future), its blocking worker, and the next
\* desktop action, which waits for the desktop lane and for this pid's
\* background-mutation lease.
\*
\* Two switches select the code. TRUE is the guarded behaviour:
\*   Admission   the worker reads the cancel flag before priming and before
\*               the AX press (PressAdmission, #6953)
\*   WorkerLease the worker keeps the desktop lane and the pid lease until it
\*               returns, as bring_to_front's worker does (#6980)
\*               (main today: FALSE)
\*
\* Check with TLC (tla2tools.jar):
\*   java -cp tla2tools.jar tlc2.TLC -config invoke_menu_cancel.cfg invoke_menu_cancel.tla
\*     main today: MC2 fails, the next action runs while the cancelled worker still presses
\*   ... -config invoke_menu_cancel_fixed.cfg     every invariant holds
\*   ... -config invoke_menu_cancel_pre6953.cfg   MC1 fails, a cancelled call presses
EXTENDS Naturals
CONSTANTS Admission, WorkerLease
VARIABLES call, worker, cancelled, lane, pidLease, next,
          pressedAfterCancel, \* the worker went on to press although its call was cancelled
          overlapped          \* another desktop action ran while a menu press was in flight
vars == <<call, worker, cancelled, lane, pidLease, next, pressedAfterCancel, overlapped>>
\* call: "waiting" (awaiting the worker) | "gone" (future dropped) | "returned"
\* worker: "resolving" | "checked" (read the flag, not cancelled) | "pressing" | "done"
\* lane / pidLease: who holds them: "call", "worker", "next" or "free"
\* next: "waiting" | "running" | "done"

Init == /\ call = "waiting" /\ worker = "resolving" /\ cancelled = FALSE
        /\ lane = "call" /\ pidLease = "call" /\ next = "waiting"
        /\ pressedAfterCancel = FALSE /\ overlapped = FALSE

\* The dispatch future is dropped while it awaits the worker: CancelOnDrop
\* sets the flag (:533-537); the call's DesktopCall drops, and the lane frees
\* once no lease clone is left (tool.rs:218-226); the pid lease `_mutation`
\* drops with the future (:776). The worker keeps the lane and the pid lease
\* only when it holds clones of them.
CallerGoes ==
  /\ call = "waiting" /\ call' = "gone" /\ cancelled' = TRUE
  /\ lane' = IF WorkerLease /\ worker # "done" THEN "worker" ELSE "free"
  /\ pidLease' = IF WorkerLease /\ worker # "done" THEN "worker" ELSE "free"
  /\ UNCHANGED <<worker, next, pressedAfterCancel, overlapped>>

\* invoke_path_in_background (:662-710): resolve the menu path, then
\* PressAdmission::admit reads the flag immediately before perform_action (:545-552).
WorkerChecks ==
  /\ worker = "resolving"
  /\ worker' = IF Admission /\ cancelled THEN "done" ELSE "checked"
  /\ pressedAfterCancel' = (pressedAfterCancel \/ (worker' = "checked" /\ cancelled))
  /\ UNCHANGED <<call, cancelled, next, overlapped>>
  /\ lane' = IF worker' = "done" /\ lane = "worker" THEN "free" ELSE lane
  /\ pidLease' = IF worker' = "done" /\ pidLease = "worker" THEN "free" ELSE pidLease
\* AXPerformAction on the menu item (up to the 2 s AX messaging timeout, :32).
\* A cancel that lands between the check and the press still presses; the
\* check and the AX call cannot be one step.
WorkerPresses ==
  /\ worker = "checked" /\ worker' = "pressing"
  /\ overlapped' = (overlapped \/ next = "running")
  /\ UNCHANGED <<call, cancelled, lane, pidLease, next, pressedAfterCancel>>
WorkerReturns ==
  /\ worker = "pressing" /\ worker' = "done"
  /\ lane' = IF lane = "worker" THEN "free" ELSE lane
  /\ pidLease' = IF pidLease = "worker" THEN "free" ELSE pidLease
  /\ UNCHANGED <<call, cancelled, next, pressedAfterCancel, overlapped>>
\* The call sees the worker's result and returns, releasing both.
CallReturns ==
  /\ call = "waiting" /\ worker = "done" /\ call' = "returned"
  /\ lane' = "free" /\ pidLease' = "free"
  /\ UNCHANGED <<worker, cancelled, next, pressedAfterCancel, overlapped>>

\* The next desktop action on this pid takes the lane, then the pid lease
\* (tool.rs:2498, tools/mod.rs:585), and runs its own input.
NextStarts ==
  /\ next = "waiting" /\ lane = "free" /\ pidLease = "free"
  /\ next' = "running" /\ lane' = "next" /\ pidLease' = "next"
  /\ overlapped' = (overlapped \/ worker = "pressing")
  /\ UNCHANGED <<call, worker, cancelled, pressedAfterCancel>>
NextEnds ==
  /\ next = "running" /\ next' = "done" /\ lane' = "free" /\ pidLease' = "free"
  /\ UNCHANGED <<call, worker, cancelled, pressedAfterCancel, overlapped>>
Done == call # "waiting" /\ worker = "done" /\ next = "done" /\ UNCHANGED vars

Next == \/ CallerGoes \/ WorkerChecks \/ WorkerPresses \/ WorkerReturns
        \/ CallReturns \/ NextStarts \/ NextEnds \/ Done
Spec == Init /\ [][Next]_vars

\* MC1. A worker whose call was already cancelled when it reached the press does not press.
MC1 == ~pressedAfterCancel
\* MC2. No other desktop action runs while a menu press is in flight.
MC2 == ~overlapped
====
