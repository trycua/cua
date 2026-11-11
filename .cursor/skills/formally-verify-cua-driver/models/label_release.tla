\* SPDX-License-Identifier: MIT
\* Copyright (c) 2026 Cua AI, Inc.
---- MODULE label_release ----
\* One public session label L and two transports, A then B, after #7083
\* (cua-driver-core/src/session.rs at 43d06894f8). A's transport closes;
\* end_sessions_for_owner ends what A holds and marks A closed only if a label
\* still names it (:869-896). A request A sent before it closed can still be
\* admitted afterwards: the proxy's per-call connections are separate sockets
\* from its control connection (serve.rs:1260 vs :1466), and mcp_envelope ends
\* the transport's sessions before it aborts its spawned tasks (mcp_envelope.rs:302-304).
\*
\* One switch:
\*   RefuseClosed  end_sessions_for_owner records every closed owner before
\*                 it ends anything, and admission refuses a closed owner on
\*                 every label (main today: FALSE: the owner is recorded only
\*                 when a label still names it, and begin_session_dispatch_inner
\*                 never reads CLOSED_TRANSPORTS for a label with no tombstone)
EXTENDS Naturals
CONSTANTS RefuseClosed
VARIABLES rec, tomb, aClosed, aMarked, aLate, bOutcome, lateBound
vars == <<rec, tomb, aClosed, aMarked, aLate, bOutcome, lateBound>>
\* rec: owner of the live record on L: "none" | "A" | "B"
\* tomb: owner named by L's tombstone: "none" | "A" | "B"
\* aClosed: A's transport has closed. aMarked: A is in CLOSED_TRANSPORTS.
\* aLate: "none" | "sent" (a request from A is in flight to the daemon) | "done"
\* bOutcome: "none" | "ok" | "refused"

Init == /\ rec = "none" /\ tomb = "none" /\ aClosed = FALSE /\ aMarked = FALSE
        /\ aLate = "none" /\ bOutcome = "none" /\ lateBound = FALSE

\* A's agent sends a request on L before its transport closes.
ASends == /\ ~aClosed /\ aLate = "none" /\ aLate' = "sent"
          /\ UNCHANGED <<rec, tomb, aClosed, aMarked, bOutcome, lateBound>>
\* begin_session_dispatch_inner (:482-541) for A's request, in one critical
\* section over ended -> records. A tombstone naming A refuses (:503-511);
\* otherwise the record is created or reused for A.
AAdmitted ==
  /\ aLate = "sent" /\ aLate' = "done"
  /\ IF tomb = "none" /\ rec \in {"none", "A"} /\ ~(RefuseClosed /\ aClosed)
     THEN rec' = "A" /\ lateBound' = (lateBound \/ aClosed)
     ELSE UNCHANGED <<rec, lateBound>>
  /\ UNCHANGED <<tomb, aClosed, aMarked, bOutcome>>
\* The control connection EOFs: end_sessions_for_owner(A) ends every episode
\* A holds (:870-882), then marks A closed only when a label still names it
\* (:886-895, names_labels_of :1166). One step: the close's end and its mark.
AClosed ==
  /\ ~aClosed /\ aClosed' = TRUE
  /\ IF rec = "A" THEN rec' = "none" /\ tomb' = "A" ELSE UNCHANGED <<rec, tomb>>
  /\ aMarked' = (RefuseClosed \/ rec = "A" \/ tomb = "A")
  /\ UNCHANGED <<aLate, bOutcome, lateBound>>
\* The idle sweep ends a live record nobody uses (evict_idle_matching :2051).
Sweep == /\ rec # "none" /\ tomb' = rec /\ rec' = "none"
         /\ UNCHANGED <<aClosed, aMarked, aLate, bOutcome, lateBound>>
\* B's agent calls start_session or a tool on L, after A has closed:
\* activate_or_revive_session_for_owner / begin_session_dispatch_inner.
\* A tombstone naming another transport is claimable only when released_to
\* (:1117-1129): that owner is in CLOSED_TRANSPORTS.
BClaims ==
  /\ aClosed /\ bOutcome = "none" /\ rec # "B"
  /\ IF rec = "none" /\ (tomb = "none" \/ (tomb = "A" /\ aMarked))
     THEN rec' = "B" /\ tomb' = "none" /\ bOutcome' = "ok"
     ELSE bOutcome' = "refused" /\ UNCHANGED <<rec, tomb>>
  /\ UNCHANGED <<aClosed, aMarked, aLate, lateBound>>
\* B gives up and retries later.
BRetries == /\ bOutcome = "refused" /\ bOutcome' = "none"
            /\ UNCHANGED <<rec, tomb, aClosed, aMarked, aLate, lateBound>>
Done == aClosed /\ aLate # "sent" /\ bOutcome = "ok" /\ UNCHANGED vars

Next == ASends \/ AAdmitted \/ AClosed \/ Sweep \/ BClaims \/ BRetries \/ Done
Spec == Init /\ [][Next]_vars

\* LR1. No new episode is bound to a transport after it has closed.
LR1 == ~lateBound
\* LR2. A label that only a closed transport names, and that nobody ended
\* with end_session, can be claimed by another transport.
LR2 == (aClosed /\ rec = "none" /\ tomb = "A") => aMarked
====
