\* SPDX-License-Identifier: MIT
\* Copyright (c) 2026 Cua AI, Inc.
---- MODULE token ----
\* The life of an element token in cua-driver-core/src/snapshot_store.rs and element_token.rs.
\* One pid, two windows, one session, a bounded number of reads.
\*   publish_snapshot (snapshot_store.rs:557-635) mints a snapshot id and retires the
\*     window's previous snapshot, plus the oldest one once the pid's lane holds
\*     LRU_CAP_PER_PID (element_token.rs:7, snapshot_store.rs:596-598);
\*   remember_retired (:392-399) keeps the last RETIRED_MEMORY (:352) retired ids;
\*   resolve (:878) acts only on a current snapshot, else stale_token_refusal names
\*     the lane; a bare row resolves through row_snapshot (:222-250) only when this
\*     session read that window once and the read is still current;
\*   a token-only call goes through fill_pid_from_token (element_token.rs:170) and
\*     token_owner (snapshot_store.rs:859), which name the pid only while the id is
\*     current or remembered (pid_of :421-429).
\*
\* Check with TLC (tla2tools.jar):
\*   java -cp tla2tools.jar tlc2.TLC -config token.cfg token.tla
\*     the code at its real bounds: every invariant holds
\*   ... -config token_agedout.cfg   R4 fails once a retired id ages out of the memory
\*   ... -config token_rowrepoint.cfg  R3 fails when a bare row re-points (pre-#6259)
EXTENDS Naturals, FiniteSets, Sequences
CONSTANTS Windows, Cap, MaxReads, RetiredMemory, RowGuard
Ids == 1..MaxReads
NoWindow == 2
VARIABLES lane, win, held, rowFrom, rowWin, passed, touched,
          refusedSet, refusalNamed, zoomed, lastResult, reads, nextId,
          wreads, retired
vars == <<lane, win, held, rowFrom, rowWin, passed, touched,
          refusedSet, refusalNamed, zoomed, lastResult, reads, nextId,
          wreads, retired>>
Minted == 1..(nextId - 1)
Replaced(w) == {id \in lane : win[id] = w}
Survivors(w) == lane \ Replaced(w)
\* :577-598 the window's own entry, then the oldest survivor when the lane is at the cap
Retired(w) == Replaced(w) \cup (IF Cardinality(Survivors(w)) = Cap THEN Survivors(w) ELSE {})
Cur(w) == {id \in lane : win[id] = w}
SetToSeq(S) == IF S = {} THEN <<>> ELSE <<CHOOSE x \in S : TRUE>> \o
               (IF Cardinality(S) > 1 THEN <<CHOOSE x \in S \ {CHOOSE y \in S : TRUE} : TRUE>> ELSE <<>>)
Remember(log, S) == LET l == log \o SetToSeq(S)
                    IN IF Len(l) > RetiredMemory THEN SubSeq(l, Len(l) - RetiredMemory + 1, Len(l)) ELSE l
InLog(id) == \E i \in 1..Len(retired) : retired[i] = id

Init == /\ lane = {} /\ win = [id \in Ids |-> NoWindow] /\ held = 0
        /\ rowFrom = 0 /\ rowWin = NoWindow /\ passed = 0 /\ touched = 0
        /\ refusedSet = {} /\ refusalNamed = FALSE /\ zoomed = {}
        /\ lastResult = "none" /\ reads = 0 /\ nextId = 1
        /\ wreads = [w \in Windows |-> 0] /\ retired = <<>>

\* publish_snapshot (:557-635): mint, retire, count this session's read of the window
ReadWin(w) ==
  /\ reads < MaxReads
  /\ lane' = (lane \ Retired(w)) \cup {nextId}
  /\ win' = [win EXCEPT ![nextId] = w]
  /\ \/ held' = nextId
     \/ held' = held /\ held # 0
  /\ \/ rowFrom' = nextId /\ rowWin' = w
     \/ rowFrom' = rowFrom /\ rowWin' = rowWin
  /\ refusedSet' = {} /\ refusalNamed' = FALSE /\ passed' = 0 /\ touched' = 0
  /\ lastResult' = "none"
  /\ zoomed' = zoomed \cap ((lane \ Retired(w)) \cup {nextId})
  /\ reads' = reads + 1 /\ nextId' = nextId + 1
  /\ wreads' = [wreads EXCEPT ![w] = @ + 1]
  /\ retired' = Remember(retired, Retired(w))
Read == \E w \in Windows : ReadWin(w)

\* resolve (:878) with a pid: a retired id is refused naming the lane (:895)
ActHeld ==
  /\ held # 0
  /\ passed' = held /\ touched' = held
  /\ IF held \in lane
     THEN lastResult' = "acted" /\ refusedSet' = {} /\ refusalNamed' = FALSE
     ELSE lastResult' = "refused" /\ refusedSet' = lane /\ refusalNamed' = TRUE
  /\ UNCHANGED <<lane, win, held, rowFrom, rowWin, zoomed, reads, nextId, wreads, retired>>

\* a remembered token, with or without a pid. Token-only: fill_pid_from_token
\* (element_token.rs:170) -> token_owner (snapshot_store.rs:859): named iff the id is
\* still in the lane or in the retired memory.
ActForgotten ==
  /\ \E id \in Minted :
     /\ id # held
     /\ passed' = id /\ touched' = id
     /\ IF id \in lane
        THEN lastResult' = "acted" /\ refusedSet' = {} /\ refusalNamed' = FALSE
        ELSE \E withPid \in BOOLEAN :
             /\ lastResult' = "refused"
             /\ refusalNamed' = (withPid \/ InLog(id))
             /\ refusedSet' = IF withPid \/ InLog(id) THEN lane ELSE {}
  /\ UNCHANGED <<lane, win, held, rowFrom, rowWin, zoomed, reads, nextId, wreads, retired>>

\* a bare row number (:904-916): acts only when this session read the window once and
\* that read is still current; otherwise row_refusal (:255) names the current snapshot.
\* RowGuard = FALSE is the pre-#6259 code, which re-pointed the row at the current read.
ActRow ==
  /\ rowFrom # 0
  /\ passed' = rowFrom
  /\ CASE ~RowGuard /\ Cur(rowWin) # {} ->
            /\ lastResult' = "acted" /\ touched' = CHOOSE id \in Cur(rowWin) : TRUE
            /\ refusedSet' = {} /\ refusalNamed' = FALSE
       [] Cur(rowWin) = {rowFrom} /\ wreads[rowWin] = 1 ->
            /\ lastResult' = "acted" /\ touched' = rowFrom
            /\ refusedSet' = {} /\ refusalNamed' = FALSE
       [] OTHER ->
            /\ lastResult' = "refused" /\ touched' = 0
            /\ refusedSet' = lane /\ refusalNamed' = TRUE
  /\ UNCHANGED <<lane, win, held, rowFrom, rowWin, zoomed, reads, nextId, wreads, retired>>

\* set_zoom (:738-756): a field of the window's current snapshot
Zoom ==
  /\ held \in lane /\ zoomed' = zoomed \cup {held}
  /\ UNCHANGED <<lane, win, held, rowFrom, rowWin, passed, touched,
                refusedSet, refusalNamed, lastResult, reads, nextId, wreads, retired>>

Next == Read \/ ActHeld \/ ActForgotten \/ ActRow \/ Zoom
Spec == Init /\ [][Next]_vars

\* R1. An action acts only on a current snapshot.
R1 == lastResult = "acted" => touched \in lane
\* R2. Each window has at most one current snapshot.
R2 == lane \subseteq Ids /\ \A w \in Windows : Cardinality(Cur(w)) <= 1
\* R3. An action acts on the snapshot the caller named, never a re-pointed one.
R3 == lastResult = "acted" => touched = passed
\* R4. A refusal names the process's current snapshots.
R4 == lastResult = "refused" => refusalNamed /\ refusedSet = lane
\* R6. A zoom is bound only to a current snapshot.
R6 == zoomed \subseteq lane
====
