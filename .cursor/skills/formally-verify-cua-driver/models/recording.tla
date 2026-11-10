\* SPDX-License-Identifier: MIT
\* Copyright (c) 2026 Cua AI, Inc.
---- MODULE recording ----
\* The runtime's trajectory recorder: RecordingSession and enum Recorder in
\* cua-driver-core/src/recording.rs (:634), and the idle-TTL stop in
\* cua-driver-sdk/src/runtime.rs. Two sessions A and B share one recorder.
\* Directories are numbered by start. The cursor sampler (and video, when it
\* started) is the recording's media.
\*
\* Three switches select the code. All TRUE is main (#6794); each FALSE is the
\* old behaviour it replaced:
\*   FinalizeOnRestart  a start over a live recording finalizes the old one
\*   DirFirst           the new directory is created before the old one is torn down
\*   IdleByGen          the idle-TTL stop names the generation it read
\*
\* Check with TLC (tla2tools.jar), from this directory:
\*   java -cp tla2tools.jar tlc2.TLC -config recording.cfg recording.tla
\*     the code: every invariant holds
\*   ... -config recording_restart.cfg   RC3 fails: A starts, B starts
\*   ... -config recording_dirfail.cfg   RC4 fails: a start whose directory fails
\*   ... -config recording_idle.cfg      RC5 fails: idle read, restart, idle stop
EXTENDS Naturals
CONSTANTS MaxStarts, FinalizeOnRestart, DirFirst, IdleByGen
S == {"A", "B"}
VARIABLES enabled, owner, gen, media, ended, hooked, dirs, turn, idle
vars == <<enabled, owner, gen, media, ended, hooked, dirs, turn, idle>>
\* dirs[g]: "none" | "open" (initial session.json) | "final" (rewritten at finalize)
\* media: "none" (idle) | "running" | "stopped" (stopped while the recording stays live)
\* idle: the TTL check reads the live generation, then stops; hit is what it stopped
Init == /\ enabled = FALSE /\ owner = "none" /\ gen = 0 /\ media = "none"
        /\ ended = [s \in S |-> FALSE] /\ hooked = [s \in S |-> FALSE] /\ dirs = [g \in 1..MaxStarts |-> "none"]
        /\ turn = [g |-> 0, into |-> 0, phase |-> "none"]
        /\ idle = [phase |-> "none", read |-> 0, hit |-> 0]

\* start_with_redactions (:802-860) under the recorder mutex: refuse an ended
\* owner, create the new directory (:830), finalize the live recording, then
\* publish the new generation.
Start(s) ==
  /\ gen < MaxStarts /\ ~ended[s]
  /\ gen' = gen + 1
  /\ dirs' = [g \in 1..MaxStarts |->
               IF g = gen + 1 THEN "open"
               ELSE IF g = gen /\ enabled /\ FinalizeOnRestart THEN "final" ELSE dirs[g]]
  /\ enabled' = TRUE /\ owner' = s /\ media' = "running"
  /\ UNCHANGED <<ended, hooked, turn, idle>>
\* create_dir_all fails (:830). The old code had already stopped the live media.
DirFail(s) ==
  /\ ~ended[s]
  /\ media' = IF enabled /\ ~DirFirst THEN "stopped" ELSE media
  /\ UNCHANGED <<enabled, owner, gen, ended, hooked, dirs, turn, idle>>
Stop == /\ enabled' = FALSE /\ owner' = "none" /\ media' = "none"
        /\ dirs' = [dirs EXCEPT ![gen] = "final"]
\* stop(StopBy::Anyone) (:900): manual stop and runtime shutdown; Stop is finalize (:665)
StopAnyone == enabled /\ Stop /\ UNCHANGED <<gen, ended, hooked, turn, idle>>
\* session end: tombstone first, then the "recording" hook runs stop(StopBy::Owner(s))
\* (cua-driver-sdk/src/runtime.rs:697)
SessionEnds(s) == /\ ~ended[s] /\ ended' = [ended EXCEPT ![s] = TRUE]
                  /\ UNCHANGED <<enabled, owner, gen, media, hooked, dirs, turn, idle>>
EndHook(s) == /\ ended[s] /\ ~hooked[s] /\ hooked' = [hooked EXCEPT ![s] = TRUE]
              /\ IF enabled /\ owner = s THEN Stop ELSE UNCHANGED <<enabled, owner, media, dirs>>
              /\ UNCHANGED <<gen, ended, turn, idle>>
\* The idle-TTL thread reads current_state().generation, then calls
\* stop(StopBy::Generation(g)) in a separate call (cua-driver-sdk/src/runtime.rs:614-620).
IdleRead == /\ enabled /\ idle.phase = "none"
            /\ idle' = [phase |-> "read", read |-> gen, hit |-> 0]
            /\ UNCHANGED <<enabled, owner, gen, media, ended, hooked, dirs, turn>>
IdleStop(readGen) ==
  /\ idle.phase = "read" /\ idle.read = readGen
  /\ IF enabled /\ (IdleByGen => gen = readGen)
       THEN Stop /\ idle' = [idle EXCEPT !.phase = "done", !.hit = gen]
       ELSE idle' = [idle EXCEPT !.phase = "done"] /\ UNCHANGED <<enabled, owner, media, dirs>>
  /\ UNCHANGED <<gen, ended, hooked, turn>>
IdleReset == /\ idle.phase = "done" /\ idle' = [phase |-> "none", read |-> 0, hit |-> 0]
             /\ UNCHANGED <<enabled, owner, gen, media, ended, hooked, dirs, turn>>
\* begin_turn (:943) reserves under the live generation; finish_turn (:1100) drops it
\* if the generation moved (Recorder::is_live, :659)
BeginTurn == /\ enabled /\ turn.phase = "none"
             /\ turn' = [g |-> gen, into |-> 0, phase |-> "pending"]
             /\ UNCHANGED <<enabled, owner, gen, media, ended, hooked, dirs, idle>>
FinishTurn == /\ turn.phase = "pending"
              /\ turn' = [g |-> turn.g, into |-> gen,
                          phase |-> IF enabled /\ gen = turn.g THEN "written" ELSE "discarded"]
              /\ UNCHANGED <<enabled, owner, gen, media, ended, hooked, dirs, idle>>
ResetTurn == /\ turn.phase \in {"written", "discarded"} /\ turn' = [g |-> 0, into |-> 0, phase |-> "none"]
             /\ UNCHANGED <<enabled, owner, gen, media, ended, hooked, dirs, idle>>

Next == \/ \E s \in S : Start(s) \/ DirFail(s) \/ SessionEnds(s) \/ EndHook(s)
        \/ StopAnyone \/ IdleRead \/ \E g \in 1..MaxStarts : IdleStop(g) \/ IdleReset
        \/ BeginTurn \/ FinishTurn \/ ResetTurn
Spec == Init /\ [][Next]_vars

\* RC1. A turn is written only into the recording it was reserved under.
RC1 == turn.phase = "written" => turn.into = turn.g
\* RC2. Once a session's end hook has run, no live recording belongs to it.
RC2 == ~(enabled /\ owner # "none" /\ hooked[owner])
\* RC3. Every recording directory that is no longer live has a final session.json.
RC3 == \A g \in 1..MaxStarts : (dirs[g] = "open") => (enabled /\ g = gen)
\* RC4. A live recording's media is running: a failed start leaves no half-torn recording.
RC4 == enabled <=> media = "running"
\* RC5. An idle stop never stops a later generation than the one it read.
RC5 == idle.hit # 0 => idle.hit = idle.read
====
