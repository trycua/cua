\* SPDX-License-Identifier: MIT
\* Copyright (c) 2026 Cua AI, Inc.
---- MODULE session ----
\* One lifecycle session id in cua-driver-core/src/session.rs: calls (one at a time), an
\* explicit end (end_session, transport EOF, revoke), the idle sweep, cleanup
\* hooks and start_session revival. The session's state is spread over
\* LIFECYCLE_RECORDS (rec, inflight, pending, fresh), ENDED_SESSIONS and
\* IDLE_ENDED_SESSIONS (tomb), and SESSION_CLEANUP_PROGRESS (cleanup).
\*
\* Two switches select the code. TRUE is the guarded behaviour:
\*   IdleRecheck  the sweep re-checks the record under the records lock that
\*                removes it (mark_session_ended, since #6773)
\*   EndRecheck   every end re-checks in_flight under the records lock that
\*                removes the record, and defers behind a call it finds
\*                (main today: FALSE)
\*
\* Check with TLC (tla2tools.jar):
\*   java -cp tla2tools.jar tlc2.TLC -config session.cfg session.tla
\*     main today: SE5 fails, an explicit end races a call admitted after it
\*   ... -config session_fixed.cfg     both re-checks: every invariant holds
\*   ... -config session_pre6773.cfg   SE2 fails, the sweep ends a used session
EXTENDS Naturals
CONSTANTS Implicit, IdleRecheck, EndRecheck
VARIABLES rec, inflight, pending, fresh, tomb, cleanup, call, ending, sweepPc,
          usedAfterPick, idleEndedBusy, overlap
vars == <<rec, inflight, pending, fresh, tomb, cleanup, call, ending, sweepPc,
          usedAfterPick, idleEndedBusy, overlap>>
\* rec: "none" | "live". pending: the deferred end on the record, "no" | reason.
\* tomb: "none" | "idle" (revived by its owner's next call) | "terminal".
\* call: "idle" | "running" (admitted; its SessionDispatchGuard is alive).
\* ending: "none", or the reason of an end that decided to finish under the
\*   records lock and released it (finish_session_end not run yet).

Init == /\ rec = "none" /\ inflight = 0 /\ pending = "no" /\ fresh = TRUE
        /\ tomb = "none" /\ cleanup = "done" /\ call = "idle"
        /\ ending = "none" /\ sweepPc = "idle"
        /\ usedAfterPick = FALSE /\ idleEndedBusy = FALSE /\ overlap = FALSE

\* mark_session_ended (:1288-1340): record out, tombstone in, cleanup
\* registered; the hooks run after it. busy: an admitted call is still running.
Finish(reason, busy) ==
  /\ rec' = "none" /\ inflight' = 0 /\ pending' = "no" /\ fresh' = FALSE
  /\ tomb' = IF reason = "idle" /\ Implicit THEN "idle" ELSE "terminal"
  /\ cleanup' = "pending"
  /\ overlap' = (overlap \/ busy)
  /\ idleEndedBusy' = (idleEndedBusy \/ (reason = "idle" /\ usedAfterPick))

\* begin_session_dispatch_inner (:448-541): one critical section over
\* ended -> idle_ended -> records. Refuses a terminal tombstone, a pending end,
\* and an idle tombstone whose cleanup has not finished; in_flight += 1.
Admit ==
  /\ call = "idle" /\ pending = "no"
  /\ \/ tomb = "none"
     \/ tomb = "idle" /\ cleanup = "done"
  /\ rec' = "live" /\ inflight' = 1 /\ fresh' = TRUE /\ tomb' = "none"
  /\ call' = "running"
  /\ usedAfterPick' = (usedAfterPick \/ sweepPc = "picked")
  /\ UNCHANGED <<pending, cleanup, ending, sweepPc, idleEndedBusy, overlap>>

\* SessionDispatchGuard::drop (:387-404): under the records lock in_flight -= 1,
\* the clock refreshes and the deferred end is taken; finish_session_end runs it
\* after the lock is released (:400-403).
CallEnds ==
  /\ call = "running" /\ call' = "idle"
  /\ inflight' = 0 /\ pending' = "no"
  /\ fresh' = (rec = "live" \/ fresh)
  /\ ending' = IF rec = "live" /\ pending # "no" THEN pending ELSE ending
  /\ UNCHANGED <<rec, tomb, cleanup, sweepPc, usedAfterPick, idleEndedBusy, overlap>>

\* the idle TTL passes with no call admitted
TimePasses ==
  /\ rec = "live" /\ inflight = 0 /\ fresh /\ fresh' = FALSE
  /\ UNCHANGED <<rec, inflight, pending, tomb, cleanup, call, ending, sweepPc,
                 usedAfterPick, idleEndedBusy, overlap>>

\* end_session_with_reason (:1692-1714) under the records lock: defer behind a
\* call in flight, or decide to finish and release the lock.
EndCheck ==
  /\ ending = "none" /\ rec = "live"
  /\ IF inflight > 0
     THEN pending' = "explicit" /\ UNCHANGED ending
     ELSE ending' = "explicit" /\ UNCHANGED pending
  /\ UNCHANGED <<rec, inflight, fresh, tomb, cleanup, call, sweepPc,
                 usedAfterPick, idleEndedBusy, overlap>>

\* finish_session_end -> mark_session_ended (:1722, :1296-1302) for a decided
\* end. With idle = None it removes the record without reading in_flight.
EndFinish ==
  /\ ending # "none" /\ ending' = "none"
  /\ CASE rec = "live" /\ EndRecheck /\ inflight > 0 ->
            /\ pending' = ending
            /\ UNCHANGED <<rec, inflight, fresh, tomb, cleanup, idleEndedBusy, overlap>>
       [] rec = "live" ->
            Finish(ending, call = "running")
       [] OTHER ->
            \* a terminal end over an idle tombstone reruns cleanup (:1307-1310)
            /\ tomb' = IF tomb = "idle" /\ ending = "explicit" THEN "terminal" ELSE tomb
            /\ cleanup' = IF tomb = "idle" /\ ending = "explicit" THEN "pending" ELSE cleanup
            /\ UNCHANGED <<rec, inflight, pending, fresh, idleEndedBusy, overlap>>
  /\ UNCHANGED <<call, sweepPc, usedAfterPick>>

\* evict_idle_matching (:1790-1810): pick expired records under the records lock ...
SweepPick ==
  /\ sweepPc = "idle" /\ rec = "live" /\ inflight = 0 /\ ~fresh
  /\ sweepPc' = "picked" /\ usedAfterPick' = FALSE
  /\ UNCHANGED <<rec, inflight, pending, fresh, tomb, cleanup, call, ending,
                 idleEndedBusy, overlap>>
\* ... then end each one. Today mark_session_ended re-runs IdleCheck::expired
\* under the lock that removes the record (:1296-1300), measured from the sweep's
\* own `now`, so any use after the pick keeps it (:1770-1776). Before #6773 the
\* end went through end_session_with_reason, which read only in_flight.
SweepEnd ==
  /\ sweepPc = "picked" /\ sweepPc' = "idle"
  /\ CASE rec = "live" /\ inflight = 0 /\ (IdleRecheck => ~usedAfterPick) ->
            Finish("idle", FALSE)
       [] ~IdleRecheck /\ rec = "live" /\ inflight > 0 /\ pending = "no" ->
            /\ pending' = "idle"
            /\ UNCHANGED <<rec, inflight, fresh, tomb, cleanup, idleEndedBusy, overlap>>
       [] OTHER ->
            UNCHANGED <<rec, inflight, pending, fresh, tomb, cleanup, idleEndedBusy, overlap>>
  /\ UNCHANGED <<call, ending, usedAfterPick>>

\* retry_session_cleanup (:1350): every remaining hook succeeds this time
CleanupDone ==
  /\ cleanup = "pending" /\ cleanup' = "done"
  /\ UNCHANGED <<rec, inflight, pending, fresh, tomb, call, ending, sweepPc,
                 usedAfterPick, idleEndedBusy, overlap>>
\* start_session -> activate_or_revive_session_for_owner (:614): cleanup first,
\* then tombstone out and record in.
Revive ==
  /\ tomb # "none" /\ cleanup = "done" /\ call = "idle"
  /\ tomb' = "none" /\ rec' = "live" /\ fresh' = TRUE
  /\ UNCHANGED <<inflight, pending, cleanup, call, ending, sweepPc,
                 usedAfterPick, idleEndedBusy, overlap>>

Next == \/ Admit \/ CallEnds \/ TimePasses \/ EndCheck \/ EndFinish
        \/ SweepPick \/ SweepEnd \/ CleanupDone \/ Revive
Spec == Init /\ [][Next]_vars

\* SE1. A session is never both live and ended.
SE1 == rec = "live" => tomb = "none"
\* SE2. The idle sweep never ends a session that took a call after it was picked.
SE2 == ~idleEndedBusy
\* SE3. A live session never inherits unfinished cleanup from an ended episode.
SE3 == rec = "live" => cleanup = "done"
\* SE4. A deferred end exists only while a call is in flight.
SE4 == pending # "no" => inflight > 0
\* SE5. A session's cleanup never starts while one of its admitted calls runs.
SE5 == ~overlap
====
