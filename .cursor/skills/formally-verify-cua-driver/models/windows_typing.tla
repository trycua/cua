\* SPDX-License-Identifier: MIT
\* Copyright (c) 2026 Cua AI, Inc.
---- MODULE windows_typing ----
\* One foreground input turn on Windows: with_confirmed_foreground
\* (platform-windows/src/input/keyboard.rs:736) and the text body of
\* send_text_synthesized_paced_after_focus (:615), after the target has been
\* confirmed in front and the input queue attached. The restore rule is
\* restores_previous in platform-windows/src/typing_turn.rs.
\* Windows: U (the user's window, foreground before the turn), T (the target),
\* O (any other window, such as a popup that takes the foreground).
\*
\* Assumption (#4477, #4500): input the target thread has not read yet is read
\* by whichever window is foreground when it is read, so a restore that runs
\* before the read hands the unread input to U.
\*
\* The unattached fallback (no queue attachment, 40 ms settle) has no read
\* signal and is not modelled.
\*
\* Check with TLC (tla2tools.jar), from this directory:
\*   java -cp tla2tools.jar tlc2.TLC -config windows_typing.cfg windows_typing.tla
\*     the rule in typing_turn.rs: every invariant holds
\*   java -cp tla2tools.jar tlc2.TLC -config windows_typing_always.cfg windows_typing.tla
\*     the old rule (restore on every exit): NoLateKeystroke fails on a stall
EXTENDS Naturals
CONSTANTS N,           \* events the turn will insert
          RestoreRule  \* "always" (old) or "drained" (typing_turn.rs)
VARIABLES front,   \* "U", "T" or "O"
          sent,    \* events inserted so far
          pending, \* inserted events the target thread has not read
          reader,  \* the target's input thread: "live" or "hung"
          turn,    \* "sending", then how it ended: "drained", "notsent", "undrained" or "lost"
          done,    \* the restore decision ran
          userGot, \* U read input the driver meant for T
          stolen   \* the restore took the foreground from O
vars == <<front, sent, pending, reader, turn, done, userGot, stolen>>

Init == /\ front = "T" /\ sent = 0 /\ pending = 0 /\ reader = "live"
        /\ turn = "sending" /\ done = FALSE /\ userGot = FALSE /\ stolen = FALSE

\* SendInput of b events after checking the foreground. Paced text sends one
\* unit (b = 1); zero-delay text sends a batch (#6892); a key chord sends all
\* of its events at once (b = N).
Send(b) == /\ turn = "sending" /\ front = "T" /\ pending = 0 /\ sent + b <= N
           /\ sent' = sent + b /\ pending' = b
           /\ UNCHANGED <<front, reader, turn, done, userGot, stolen>>
\* SendInput inserts fewer events than asked (UIPI, secure desktop).
Short == /\ turn = "sending" /\ front = "T" /\ pending = 0 /\ sent < N
         /\ sent' = sent + 1 /\ pending' = 1 /\ turn' = "undrained"
         /\ UNCHANGED <<front, reader, done, userGot, stolen>>
\* SendInput inserts nothing, so nothing is pending.
ShortNone == /\ turn = "sending" /\ front = "T" /\ pending = 0 /\ turn' = "notsent"
             /\ UNCHANGED <<front, sent, pending, reader, done, userGot, stolen>>
\* A 2 s read wait (send_and_await_read per batch, or wait_for_drain :877) runs out.
Timeout == /\ turn = "sending" /\ pending > 0 /\ turn' = "undrained"
           /\ UNCHANGED <<front, sent, pending, reader, done, userGot, stolen>>
\* The final drain sees every inserted event read.
Drained == /\ turn = "sending" /\ pending = 0 /\ sent = N /\ turn' = "drained"
           /\ UNCHANGED <<front, sent, pending, reader, done, userGot, stolen>>
\* foreground_lost: before any text batch, the first included, the foreground
\* is not the target (keyboard.rs:650-660).
Lost == /\ turn = "sending" /\ pending = 0 /\ sent < N /\ front # "T"
        /\ turn' = "lost"
        /\ UNCHANGED <<front, sent, pending, reader, done, userGot, stolen>>
\* The target's input thread reads one event in whichever window is in front.
Read == /\ pending > 0 /\ reader = "live"
        /\ pending' = pending - 1 /\ userGot' = (userGot \/ front = "U")
        /\ UNCHANGED <<front, sent, reader, turn, done, stolen>>
Hang == reader = "live" /\ reader' = "hung"
        /\ UNCHANGED <<front, sent, pending, turn, done, userGot, stolen>>
Wake == reader = "hung" /\ reader' = "live"
        /\ UNCHANGED <<front, sent, pending, turn, done, userGot, stolen>>
OtherTakesFront == /\ ~done /\ front = "T" /\ front' = "O"
                   /\ UNCHANGED <<sent, pending, reader, turn, done, userGot, stolen>>

\* restores_previous (typing_turn.rs), applied at keyboard.rs:827
Restores == IF RestoreRule = "always" THEN TRUE ELSE turn \in {"drained", "notsent"}
Finish == /\ turn # "sending" /\ ~done /\ done' = TRUE
          /\ front' = IF Restores THEN "U" ELSE front
          /\ stolen' = (Restores /\ front = "O")
          /\ UNCHANGED <<sent, pending, reader, turn, userGot>>
Idle == done /\ UNCHANGED vars

Next == \/ \E b \in 1..N : Send(b)
        \/ Short \/ ShortNone \/ Timeout \/ Drained \/ Lost \/ Read \/ Hang \/ Wake
        \/ OtherTakesFront \/ Finish \/ Idle
Spec == Init /\ [][Next]_vars

TypeOK == /\ front \in {"U", "T", "O"} /\ sent \in 0..N /\ pending \in 0..N
          /\ reader \in {"live", "hung"} /\ done \in BOOLEAN
          /\ turn \in {"sending", "drained", "notsent", "undrained", "lost"}
\* A keystroke meant for the target never lands in the user's window.
NoLateKeystroke == ~userGot
\* After foreground_lost the driver does not take the foreground back from the
\* window that took it. A window that takes it after the last read check is
\* not observed, and the restore still runs, as before.
NoSteal == ~(stolen /\ turn = "lost")
\* A turn with nothing left unread gives the user's window back.
GivesWindowBack == (done /\ turn \in {"drained", "notsent"}) => front = "U"
====
