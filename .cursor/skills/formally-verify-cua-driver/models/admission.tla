\* SPDX-License-Identifier: MIT
\* Copyright (c) 2026 Cua AI, Inc.
---- MODULE admission ----
\* Runtime admission and the macOS permission gate: enum Admission in
\* cua-driver-sdk/src/runtime.rs, set by cua-driver/src/main.rs and cua-driver/src/serve.rs
\* (set_permission_gate_pending, settle_permission_gate).
\* Every route (daemon socket call, trusted socket session, MCP HTTP, MCP
\* envelopes, SDK) reaches a tool through invoke_with_context_and_evidence
\* or RuntimeSession::invoke, both of which refuse while AwaitingPermissions.
\*
\* Three switches:
\*   PollAfterTimeout  a gate that timed out keeps polling on its own thread
\*               (#6739); FALSE is the code before, which stayed gated until restart
\*   EveryRoute  every route reads admission; FALSE is the code before #6802,
\*               where only the daemon socket `call` arm read the gate
\*   Regate      not on main: a grant revoked while serving moves Open back to
\*               AwaitingPermissions (CUA-3682 question D1(c))
\*
\* Check with TLC (tla2tools.jar):
\*   java -cp tla2tools.jar tlc2.TLC -config admission.cfg admission.tla
\*     main today: every invariant holds
\*   ... -config admission_pre6739.cfg  PG2 fails: the grant arrives, the gate never opens
\*   ... -config admission_pre6802.cfg  PG1 fails: an MCP HTTP call runs while pending
\*   ... -config admission_regate.cfg   PG1 fails: a call admitted just before
\*     the re-gate runs while the gate is pending (the check is not held)
EXTENDS Naturals
CONSTANTS PollAfterTimeout, EveryRoute, Regate
VARIABLES adm, grants, poll, call, ranPending, everClosed
vars == <<adm, grants, poll, call, ranPending, everClosed>>
\* adm: "Open" | "Awaiting" | "Closed". grants: TCC state, "missing" | "granted".
\* poll: the gate's helper, "probing" | "timedout" (slow poll thread) | "settled".
\* call: one tool call, "idle" | "checked" (passed the admission check) | "running".

Init == /\ adm = "Awaiting" /\ grants = "missing" /\ poll = "probing"
        /\ call = "idle" /\ ranPending = FALSE /\ everClosed = FALSE

\* main.rs:751 sets AwaitingPermissions before the serve thread binds (Init).
\* You grant Accessibility and Screen Recording in System Settings.
UserGrants == /\ grants = "missing" /\ grants' = "granted"
              /\ UNCHANGED <<adm, poll, call, ranPending, everClosed>>
UserRevokes == /\ grants = "granted" /\ grants' = "missing"
               /\ UNCHANGED <<adm, poll, call, ranPending, everClosed>>
\* run_if_needed_with_observer returns Ok once a fresh child probe is green;
\* settle_permission_gate opens admission (serve.rs:705-707).
GatePasses == /\ poll = "probing" /\ grants = "granted" /\ poll' = "settled"
              /\ adm' = IF adm = "Awaiting" THEN "Open" ELSE adm
              /\ UNCHANGED <<grants, call, ranPending, everClosed>>
\* The 10 minute deadline passes: admission stays gated and a poll thread
\* keeps waiting (serve.rs:709-716, #6739).
GateTimesOut == /\ poll = "probing" /\ poll' = "timedout"
                /\ UNCHANGED <<adm, grants, call, ranPending, everClosed>>
SlowPollOpens == /\ PollAfterTimeout /\ poll = "timedout" /\ grants = "granted" /\ poll' = "settled"
                 /\ adm' = IF adm = "Awaiting" THEN "Open" ELSE adm
                 /\ UNCHANGED <<grants, call, ranPending, everClosed>>
\* Not on main: a revoke noticed while serving re-gates (CAS Open -> Awaiting).
Regated == /\ Regate /\ adm = "Open" /\ grants = "missing" /\ adm' = "Awaiting"
           /\ UNCHANGED <<grants, poll, call, ranPending, everClosed>>
\* shutdown stores Closed (runtime.rs:284-287). Closed is terminal: the CAS in
\* set_awaiting_permissions (:260-272) only moves Open <-> Awaiting.
Shutdown == /\ adm # "Closed" /\ adm' = "Closed" /\ everClosed' = TRUE
            /\ UNCHANGED <<grants, poll, call, ranPending>>

\* invoke_with_context_and_evidence (:364-376): refuse while Awaiting, refuse
\* when Closed, take the lifecycle read guard, check Closed again. Awaiting is
\* not re-read after the guard.
CallChecks == /\ call = "idle" /\ (adm = "Open" \/ (~EveryRoute /\ adm = "Awaiting"))
              /\ call' = "checked"
              /\ UNCHANGED <<adm, grants, poll, ranPending, everClosed>>
CallRefused == /\ call = "idle" /\ adm # "Open"
               /\ UNCHANGED vars
CallRuns == /\ call = "checked" /\ call' = "running"
            /\ IF adm = "Closed" THEN call' = "idle" /\ UNCHANGED ranPending
               ELSE ranPending' = (ranPending \/ adm = "Awaiting")
            /\ UNCHANGED <<adm, grants, poll, everClosed>>
CallEnds == /\ call = "running" /\ call' = "idle"
            /\ UNCHANGED <<adm, grants, poll, ranPending, everClosed>>

Next == \/ UserGrants \/ UserRevokes \/ GatePasses \/ GateTimesOut \/ SlowPollOpens
        \/ Regated \/ Shutdown \/ CallChecks \/ CallRefused \/ CallRuns \/ CallEnds
\* Fairness: the gate's probe and the slow poll run on their own threads, and
\* a fresh child probe made after you grant sees the grant. A probe that keeps
\* failing reads as "missing" (platform-macos/src/permissions/gate.rs:122-150)
\* and keeps the gate closed; PG2 assumes that does not go on forever.
Spec == Init /\ [][Next]_vars /\ WF_vars(GatePasses) /\ WF_vars(SlowPollOpens)

\* PG1. No tool runs while the permission gate is pending.
PG1 == ~ranPending
\* PG2. Once you grant the permissions, tools open without a restart (unless
\* you revoke them again or the daemon shuts down). A temporal property.
PG2 == (grants = "granted") ~> (adm # "Awaiting" \/ grants = "missing")
\* PG3. Shutdown is final: a closed runtime never opens again.
PG3 == everClosed => adm = "Closed"
====
