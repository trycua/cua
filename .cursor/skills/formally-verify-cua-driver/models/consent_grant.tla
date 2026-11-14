\* SPDX-License-Identifier: MIT
\* Copyright (c) 2026 Cua AI, Inc.
---- MODULE consent_grant ----
\* One protected resource grant (standard mode, a GrantInStandard adapter such
\* as computer_history or browser_prepare.existing_profile) for one session
\* label on one transport, across two lifecycle episodes of that label.
\* cua-driver-core/src/consent.rs ProtectedResourceGrants::authorize (:376-478),
\* revoke_session (:508-520); the session-end hook that calls it
\* (tool.rs:1356-1385); the call path runs consent before session admission
\* (tool.rs:2230-2420, then begin_session_dispatch :2428).
\*
\* One switch, InsertCheck: what the insert checks before storing the grant.
\*   "none"    nothing (main)
\*   "prompt"  no revoke_session for its session ran since its prompt opened.
\*             Able-to-fail: a call queued on the admission mutex behind
\*             another prompt can see its session end before its own prompt.
\*   "counter" no revoke_session for its session ran since the authorization
\*             started. Able-to-fail: a session that ended between dispatch's
\*             ended check and the start of authorize still gets a grant.
\*   "call"    no revoke_session for its session ran since the authorization
\*             started (the fix: GrantTable.revocations, read at
\*             Authorizing::start before the first lookup), and the session
\*             had not already ended then (is_session_ended right after the
\*             snapshot; such a call may still be recreated at lifecycle
\*             admission, so its approval covers that call only)
EXTENDS Naturals
CONSTANTS InsertCheck
VARIABLES ep, live, grant, pc, askEp, revokedSince, usedStale, askedEnded
vars == <<ep, live, grant, pc, askEp, revokedSince, usedStale, askedEnded>>
\* ep: the label's current episode (1 or 2). live: it is live.
\* grant: 0 (none) or the episode the grant in the map was approved in.
\* pc: the call: "idle" | "queued" (waiting for the admission mutex) |
\*     "prompting" (provider showing the consent prompt) | "done"
\* askEp: the episode the prompt was asked in.

Init == /\ ep = 1 /\ live = TRUE /\ grant = 0 /\ pc = "idle" /\ askEp = 0
        /\ revokedSince = FALSE /\ usedStale = FALSE /\ askedEnded = FALSE

\* authorize starts and the first lookup misses; it waits for the admission
\* mutex, which another session's prompt may hold. The session may already
\* have ended: dispatch's ended check (tool.rs:2077) runs before consent, and
\* an end can land between the two.
Ask == /\ pc = "idle" /\ grant = 0
       /\ pc' = "queued" /\ askEp' = ep /\ askedEnded' = ~live
       /\ revokedSince' = IF InsertCheck \in {"counter", "call"} THEN FALSE ELSE revokedSince
       /\ UNCHANGED <<ep, live, grant, usedStale>>
\* The admission mutex is taken (:442), the second lookup misses and the
\* provider prompt starts (:464, broker.approve awaits the user).
Admit == /\ pc = "queued" /\ grant = 0 /\ pc' = "prompting"
         /\ revokedSince' = IF InsertCheck = "prompt" THEN FALSE ELSE revokedSince
         /\ UNCHANGED <<ep, live, grant, askEp, usedStale, askedEnded>>
\* lookup hits: the call is let through on the grant in the map without a prompt.
UseGrant == /\ pc = "idle" /\ live /\ grant # 0
            /\ usedStale' = (usedStale \/ grant # ep)
            /\ pc' = "done"
            /\ UNCHANGED <<ep, live, grant, askEp, revokedSince, askedEnded>>
\* The user accepts; the grant is inserted under the grants mutex (:477).
Accept == /\ pc = "prompting" /\ pc' = "done"
          /\ grant' = IF \/ InsertCheck # "none" /\ revokedSince
                         \/ InsertCheck = "call" /\ askedEnded
                      THEN grant ELSE askEp
          /\ UNCHANGED <<ep, live, askEp, revokedSince, usedStale, askedEnded>>
\* The session ends (end_session, its transport closes, or the idle sweep) and
\* its end hook runs revoke_session (tool.rs:1374), which takes only the grants
\* mutex, not the admission mutex a prompt holds.
SessionEnds == /\ live /\ live' = FALSE /\ grant' = 0 /\ revokedSince' = TRUE
               /\ UNCHANGED <<ep, pc, askEp, usedStale, askedEnded>>
\* start_session on the same label from the same transport, or the next call
\* of an idle-reclaimed session: a new episode with the same grant key
\* (resource_grant_key :561-581 has no episode in it).
Revive == /\ ~live /\ ep = 1 /\ live' = TRUE /\ ep' = 2
          /\ UNCHANGED <<grant, pc, askEp, revokedSince, usedStale, askedEnded>>
NextCall == /\ pc = "done" /\ pc' = "idle"
            /\ UNCHANGED <<ep, live, grant, askEp, revokedSince, usedStale, askedEnded>>

Next == Ask \/ Admit \/ UseGrant \/ Accept \/ SessionEnds \/ Revive \/ NextCall
Spec == Init /\ [][Next]_vars

\* CG1. No grant outlives the episode it was approved in.
CG1 == grant # 0 => (live /\ grant = ep)
\* CG2. No call is let through on a grant from an earlier episode.
CG2 == ~usedStale
====
