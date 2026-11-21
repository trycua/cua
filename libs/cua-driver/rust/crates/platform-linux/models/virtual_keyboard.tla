\* SPDX-License-Identifier: MIT
\* Copyright (c) 2026 Cua AI, Inc.
---- MODULE virtual_keyboard ----
\* One session's uinput keyboard on the X11 virtual master keyboard route:
\* Keyboard, Delivery and plugged_keyboard in src/input/mpx_keyboard.rs, and
\* BorrowedKeycodes in src/input/mod.rs. virtual_keyboard.md draws it.
\*
\* A call writes key transitions (press, release, press, ...) to the device.
\* The X server's input thread reads them later, and can stall at any time
\* while the rest of the server still answers requests. A text may type on
\* keycodes it borrowed, which are restored when the call ends.
\*
\* Assumptions, each observed on Xorg 21.1 with libinput:
\*   closing the device discards what the server has not read, and the server
\*   releases a key it holds down on a device that goes away;
\*   the server keeps listing a closed device for a while, under its name;
\*   with key repeat on, the server repeats a key it holds down.
\*
\* Not modelled: a failed write to the device (it counts as written, so it
\* ends as KeysStayUnread), pacing, the idle wait before the restore, and a
\* key that is held, not repeated, through a stall.
\*
\* Check with TLC (tla2tools.jar):
\*   java -cp tla2tools.jar tlc2.TLC -config virtual_keyboard.cfg virtual_keyboard.tla
\*     the rules in mpx_keyboard.rs: every invariant holds
\*   java -cp tla2tools.jar tlc2.TLC -config virtual_keyboard_before.cfg virtual_keyboard.tla
\*     the rules before them: every invariant but TypeOK fails
EXTENDS Naturals
CONSTANTS N,        \* key transitions one call writes (even)
          Calls,    \* calls the session makes
          Unplug,   \* "unread": a call that ends with unread keys unplugs. "never": it does not
          WarmUp,   \* "strict": an unread warm-up key fails the plug-in. "besteffort": it is kept
          RepeatOff \* the server's key repeat is turned off on the keyboard
VARIABLES kbd,      \* "unplugged", "listed" (closed, the server still lists it), "warming", "ready", "typing"
          written,  \* transitions this call (or warm-up) wrote
          read,     \* raw key events this call (or warm-up) counted
          queue,    \* transitions the server has not read
          foreign,  \* of those, the ones an earlier call or the warm-up wrote
          probe,    \* of those, the warm-up key's (it has no symbol in any keymap)
          stalled,  \* the server's input thread is not reading
          down,     \* the server has read a press and not its release
          keymap,   \* "borrowed" while a call's keycodes are bound, else "plain"
          calls,    \* calls left
          repeated, \* the server repeated a key
          wrong,    \* the server read a call's key after its keycodes were restored
          early     \* a call counted a key it did not write
vars == <<kbd, written, read, queue, foreign, probe, stalled, down, keymap, calls, repeated, wrong, early>>
flags == <<repeated, wrong, early>>

Init == /\ kbd = "unplugged" /\ written = 0 /\ read = 0 /\ queue = 0 /\ foreign = 0 /\ probe = 0
        /\ stalled = FALSE /\ down = FALSE /\ keymap = "plain" /\ calls = Calls
        /\ repeated = FALSE /\ wrong = FALSE /\ early = FALSE

\* plugged_keyboard: create the device, attach it, write the warm-up key.
PlugIn == /\ kbd = "unplugged" /\ calls > 0
          /\ kbd' = "warming" /\ written' = 2 /\ read' = 0 /\ queue' = 2 /\ foreign' = 0 /\ probe' = 2
          /\ UNCHANGED <<stalled, down, keymap, calls, flags>>

WarmUpRead == /\ kbd = "warming" /\ read = written
              /\ kbd' = "ready"
              /\ UNCHANGED <<written, read, queue, foreign, probe, stalled, down, keymap, calls, flags>>

Close == /\ kbd' = "listed" /\ queue' = 0 /\ foreign' = 0 /\ probe' = 0 /\ down' = FALSE

WarmUpUnread == /\ kbd = "warming" /\ read < written
                /\ IF WarmUp = "strict"
                   THEN Close /\ calls' = calls - 1
                   ELSE kbd' = "ready" /\ UNCHANGED <<queue, foreign, probe, down, calls>>
                /\ UNCHANGED <<written, read, stalled, keymap, flags>>

ServerDropsOld == /\ kbd = "listed" /\ kbd' = "unplugged"
                  /\ UNCHANGED <<written, read, queue, foreign, probe, stalled, down, keymap, calls, flags>>

\* Delivery::begin, after the text's keycodes were borrowed.
StartTyping == /\ kbd = "ready" /\ calls > 0
               /\ kbd' = "typing" /\ written' = 0 /\ read' = 0 /\ foreign' = queue
               /\ keymap' = "borrowed" /\ calls' = calls - 1
               /\ UNCHANGED <<queue, probe, stalled, down, flags>>

Write == /\ kbd = "typing" /\ written < N
         /\ written' = written + 1 /\ queue' = queue + 1
         /\ UNCHANGED <<kbd, read, foreign, probe, stalled, down, keymap, calls, flags>>

\* The input thread reads one transition and reports it as a raw key event.
ServerReads ==
    /\ queue > 0 /\ ~stalled
    /\ queue' = queue - 1 /\ down' = ~down
    /\ foreign' = IF foreign > 0 THEN foreign - 1 ELSE 0
    /\ probe' = IF probe > 0 THEN probe - 1 ELSE 0
    /\ read' = IF kbd \in {"warming", "typing"} THEN read + 1 ELSE read
    /\ early' = (early \/ (kbd = "typing" /\ foreign > 0))
    /\ wrong' = (wrong \/ (probe = 0 /\ (keymap = "plain" \/ foreign > 0)))
    /\ UNCHANGED <<kbd, written, stalled, keymap, calls, repeated>>

\* Delivery::read_all returned true at the end of the text.
AllRead == /\ kbd = "typing" /\ written = N /\ read >= written
           /\ kbd' = "ready" /\ keymap' = "plain"
           /\ UNCHANGED <<written, read, queue, foreign, probe, stalled, down, calls, flags>>

\* read_all timed out at a point where the call holds no key down.
KeysStayUnread ==
    /\ kbd = "typing" /\ written % 2 = 0 /\ read < written
    /\ IF Unplug = "unread" THEN Close ELSE kbd' = "ready" /\ UNCHANGED <<queue, foreign, probe, down>>
    /\ keymap' = "plain"
    /\ UNCHANGED <<written, read, stalled, calls, flags>>

SessionEnds == /\ kbd = "ready" /\ Close
               /\ UNCHANGED <<written, read, stalled, keymap, calls, flags>>

Stall == /\ ~stalled /\ stalled' = TRUE
         /\ UNCHANGED <<kbd, written, read, queue, foreign, probe, down, keymap, calls, flags>>
Resume == /\ stalled /\ stalled' = FALSE
          /\ UNCHANGED <<kbd, written, read, queue, foreign, probe, down, keymap, calls, flags>>

ServerRepeats == /\ down /\ stalled /\ ~RepeatOff /\ ~repeated /\ repeated' = TRUE
                 /\ UNCHANGED <<kbd, written, read, queue, foreign, probe, stalled, down, keymap, calls, wrong, early>>

Next == PlugIn \/ WarmUpRead \/ WarmUpUnread \/ ServerDropsOld \/ StartTyping \/ Write
        \/ ServerReads \/ AllRead \/ KeysStayUnread \/ SessionEnds \/ Stall \/ Resume
        \/ ServerRepeats
Spec == Init /\ [][Next]_vars

TypeOK == /\ kbd \in {"unplugged", "listed", "warming", "ready", "typing"}
          /\ written \in 0..N /\ read \in 0..(Calls * N + 2) /\ queue \in 0..(Calls * N + 2)
          /\ foreign \in 0..queue /\ probe \in 0..2 /\ calls \in 0..Calls
          /\ keymap \in {"plain", "borrowed"}
          /\ {stalled, down, repeated, wrong, early} \subseteq BOOLEAN
\* A keyboard that stays plugged in between calls has nothing unread and no key down.
ReadyIsClean == kbd = "ready" => (queue = 0 /\ ~down)
\* The server never types a key the driver did not write.
NoRepeat == ~repeated
\* A call's key is never read after that call's borrowed keycodes were restored.
NoKeyAfterRestore == ~wrong
\* A call confirms only its own keys.
CountsOnlyItsOwnKeys == ~early
====
