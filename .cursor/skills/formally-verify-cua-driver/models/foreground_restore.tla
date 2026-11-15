\* SPDX-License-Identifier: MIT
\* Copyright (c) 2026 Cua AI, Inc.
---- MODULE foreground_restore ----
\* One macOS foreground turn: with_foreground_assist and
\* with_foreground_hid_activation in platform-macos/src/input/skylight.rs. The same
\* save-front, act, put-it-back shape is in type_text's browser path
\* (platform-macos/src/tools/type_text.rs, reactivate_application) and the foreground
\* invoke_menu restore (platform-macos/src/tools/invoke_menu.rs).
\* Apps: U (in front before the turn), T (the target), O (another app).
\*
\* One switch selects the code:
\*   TargetStillFront  the restore runs only while T is still the front
\*               process, as Windows' TypingTurn::Lost and the macOS browser
\*               restore already do (main today: FALSE, the restore is
\*               unconditional). The arbiter's hardware-input check is not
\*               used: the HID route posts its own events at the HID tap.
\*
\* Not modelled: the front read and set_front(prev) are two calls, so a switch
\* in that gap is still overridden; and the HID route's second set_front(T)
\* after a slow focus wait is one Front here. Both windows are milliseconds.
\*
\* Check with TLC (tla2tools.jar):
\*   java -cp tla2tools.jar tlc2.TLC -config foreground_restore.cfg foreground_restore.tla
\*     main today: FR1 fails, the restore undoes your app switch
\*   ... -config foreground_restore_fixed.cfg   every invariant holds
EXTENDS Naturals
CONSTANTS TargetStillFront
VARIABLES front, pc, saved, userMoved, undidUser
vars == <<front, pc, saved, userMoved, undidUser>>
\* pc: "idle" | "saved" (front read) | "fronted" (T activated, body running) | "restored"
\* userMoved: the user switched apps with mouse or keyboard during the turn

Init == /\ front = "U" /\ pc = "idle" /\ saved = "none"
        /\ userMoved = FALSE /\ undidUser = FALSE

\* foreground_turn() bumps the epoch and takes FOREGROUND_LOCK (:1302-1316),
\* then the turn reads the front process (:1359) ...
Save == /\ pc = "idle" /\ pc' = "saved" /\ saved' = front
        /\ UNCHANGED <<front, userMoved, undidUser>>
\* ... and set_front(T) (:1369), waits for the activation (:1376), runs the body.
Front == /\ pc = "saved" /\ pc' = "fronted" /\ front' = "T"
         /\ UNCHANGED <<saved, userMoved, undidUser>>
\* You click another window or press Cmd-Tab while the keys are posted.
\* A switch between the front read and set_front(T) is overridden by the
\* activation itself, which FR1 does not count, so only a switch after it
\* marks userMoved. A switch back to T itself is not modelled.
UserSwitches == /\ pc \in {"saved", "fronted"} /\ \E a \in {"U", "O"} : a # front /\ front' = a
                /\ userMoved' = (userMoved \/ pc = "fronted")
                /\ UNCHANGED <<pc, saved, undidUser>>
\* Another app activates itself (a notification, a dialog).
OtherActivates == /\ pc = "fronted" /\ front # "O" /\ front' = "O"
                  /\ UNCHANGED <<pc, saved, userMoved, undidUser>>
\* if prev_ok { set_front(prev_psn) } (:1380-1382; HID route :1532), with no check of who is in
\* front now. With TargetStillFront the turn leaves the front alone once T has
\* lost it, to you or to another app.
Restore ==
  /\ pc = "fronted" /\ pc' = "restored"
  /\ IF TargetStillFront /\ front # "T"
     THEN UNCHANGED <<front, undidUser>>
     ELSE /\ front' = saved
          /\ undidUser' = (undidUser \/ (userMoved /\ front # saved))
  /\ UNCHANGED <<saved, userMoved>>
Done == pc = "restored" /\ UNCHANGED vars

Next == Save \/ Front \/ UserSwitches \/ OtherActivates \/ Restore \/ Done
Spec == Init /\ [][Next]_vars

\* FR1. The turn's restore never undoes an app switch you made during the turn.
FR1 == ~undidUser
\* FR2. A turn nobody interrupted gives your app back.
FR2 == (pc = "restored" /\ ~userMoved /\ saved = "U" /\ front # "O") => front = "U"
====
