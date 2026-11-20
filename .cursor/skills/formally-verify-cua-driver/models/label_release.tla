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
VARIABLES rec, tomb, aClosed, aStep, scanHit, aMarked, aLate, bOutcome, lateBound
vars == <<rec, tomb, aClosed, aStep, scanHit, aMarked, aLate, bOutcome, lateBound>>
\* rec: owner of the live record on L: "none" | "A" | "B"
\* tomb: owner named by L's tombstone: "none" | "A" | "B"
\* aClosed: A's transport has closed. aMarked: A is in CLOSED_TRANSPORTS.
\* aStep: end_sessions_for_owner(A): "open" | "scanned" | "ended" | "done"
\* scanHit: its scan found A's record on L.
\* aLate: "none" | "sent" (a request from A is in flight to the daemon) | "done"
\* bOutcome: "none" | "ok" | "refused"

Init == /\ rec = "none" /\ tomb = "none" /\ aClosed = FALSE /\ aStep = "open"
        /\ scanHit = FALSE /\ aMarked = FALSE
        /\ aLate = "none" /\ bOutcome = "none" /\ lateBound = FALSE

\* A's agent sends a request on L before its transport closes.
ASends == /\ ~aClosed /\ aLate = "none" /\ aLate' = "sent"
          /\ UNCHANGED <<rec, tomb, aClosed, aStep, scanHit, aMarked, bOutcome, lateBound>>
\* begin_session_dispatch_inner (:482-541) for A's request, in one critical
\* section over ended -> records. A tombstone naming A refuses (:503-511);
\* otherwise the record is created or reused for A.
AAdmitted ==
  /\ aLate = "sent" /\ aLate' = "done"
  /\ IF tomb = "none" /\ rec \in {"none", "A"} /\ ~(RefuseClosed /\ aClosed)
     THEN rec' = "A" /\ lateBound' = (lateBound \/ aClosed)
     ELSE UNCHANGED <<rec, lateBound>>
  /\ UNCHANGED <<tomb, aClosed, aStep, scanHit, aMarked, bOutcome>>
\* The control connection EOFs and end_sessions_for_owner(A) runs in three
\* critical sections; a call can be admitted between any two of them.
\* 1. The scan (:870-877) lists A's records under the records lock. With
\*    RefuseClosed, A is recorded closed here, before anything ends.
AClosed ==
  /\ ~aClosed /\ aClosed' = TRUE /\ aStep' = "scanned"
  /\ scanHit' = (rec = "A")
  /\ aMarked' = (aMarked \/ RefuseClosed)
  /\ UNCHANGED <<rec, tomb, aLate, bOutcome, lateBound>>
\* 2. Each listed episode ends, bound to A (:880-882): an episode A no longer
\*    holds is left alone.
AEnds ==
  /\ aStep = "scanned" /\ aStep' = "ended"
  /\ IF scanHit /\ rec = "A" THEN rec' = "none" /\ tomb' = "A"
                              ELSE UNCHANGED <<rec, tomb>>
  /\ UNCHANGED <<aClosed, scanHit, aMarked, aLate, bOutcome, lateBound>>
\* 3. A is marked closed only when a label still names it (:886-895,
\*    names_labels_of :1166), live or tombstoned. A record bound to A after
\*    the scan is marked but not ended.
AMarks ==
  /\ aStep = "ended" /\ aStep' = "done"
  /\ aMarked' = (aMarked \/ rec = "A" \/ tomb = "A")
  /\ UNCHANGED <<rec, tomb, aClosed, scanHit, aLate, bOutcome, lateBound>>
\* The idle sweep ends a live record nobody uses (evict_idle_matching :2051).
Sweep == /\ rec # "none" /\ tomb' = rec /\ rec' = "none"
         /\ UNCHANGED <<aClosed, aStep, scanHit, aMarked, aLate, bOutcome, lateBound>>
\* B's agent calls start_session or a tool on L, after A has closed:
\* activate_or_revive_session_for_owner / begin_session_dispatch_inner.
\* A tombstone naming another transport is claimable only when released_to
\* (:1117-1129): that owner is in CLOSED_TRANSPORTS.
BClaims ==
  /\ aClosed /\ bOutcome = "none" /\ rec # "B"
  /\ IF rec = "none" /\ (tomb = "none" \/ (tomb = "A" /\ aMarked))
     THEN rec' = "B" /\ tomb' = "none" /\ bOutcome' = "ok"
     ELSE bOutcome' = "refused" /\ UNCHANGED <<rec, tomb>>
  /\ UNCHANGED <<aClosed, aStep, scanHit, aMarked, aLate, lateBound>>
\* B gives up and retries later.
BRetries == /\ bOutcome = "refused" /\ bOutcome' = "none"
            /\ UNCHANGED <<rec, tomb, aClosed, aStep, scanHit, aMarked, aLate, lateBound>>
Done == aStep = "done" /\ aLate # "sent" /\ bOutcome = "ok" /\ UNCHANGED vars

Next == ASends \/ AAdmitted \/ AClosed \/ AEnds \/ AMarks \/ Sweep \/ BClaims \/ BRetries \/ Done
Spec == Init /\ [][Next]_vars

\* LR1. No new episode is bound to a transport after it has closed.
LR1 == ~lateBound
\* LR2. A label that only a closed transport names, and that nobody ended
\* with end_session, can be claimed by another transport once A's close has
\* finished.
LR2 == (aStep = "done" /\ rec = "none" /\ tomb = "A") => aMarked
====
