\* SPDX-License-Identifier: MIT
\* Copyright (c) 2026 Cua AI, Inc.
---- MODULE serve_drain ----
\* Daemon shutdown in cua-driver/src/serve.rs run_serve: the accept loop breaks, the
\* daemon drains its connections, gives up its name, and tears down the SDK.
\* One client connection is mid-call when shutdown starts; a successor daemon
\* may launch as soon as the name lock is free.
\*
\* One switch selects the platform:
\*   Platform = "unix"     connection tasks in a JoinSet, aborted and joined,
\*                         proxy sessions released, then the socket and the lock
\*                         go with the function's return (:1484-1495, :1046)
\*   Platform = "windows"  connection tasks spawned detached (:1930); the pid
\*                         file and the pipe lock go (:2274-2279), then
\*                         sdk.shutdown waits for in-flight calls (:2280)
\*
\* Check with TLC (tla2tools.jar):
\*   java -cp tla2tools.jar tlc2.TLC -config serve_drain.cfg serve_drain.tla
\*     unix: every invariant holds
\*   ... -config serve_drain_windows.cfg  SD1 and SD2 fail
EXTENDS Naturals
CONSTANTS Platform
VARIABLES pc, conn, proxy, lock, successor, overlap, leakedProxy
vars == <<pc, conn, proxy, lock, successor, overlap, leakedProxy>>
\* pc: the old daemon, "serving" | "draining" | "released" | "sdkdown" | "gone"
\* conn: its connection, "calling" (a tool call in flight) | "idle" | "closed"
\* proxy: its entry in ACTIVE_PROXY_SESSIONS: "active" | "released"
\* lock: who holds the daemon name lock: "old" | "free" | "new"
\* successor: "none" | "serving"

Init == /\ pc = "serving" /\ conn = "calling" /\ proxy = "active"
        /\ lock = "old" /\ successor = "none"
        /\ overlap = FALSE /\ leakedProxy = FALSE

\* `shutdown` or the host's lifetime pipe closes: the accept loop breaks.
Stop == /\ pc = "serving" /\ pc' = "draining"
        /\ UNCHANGED <<conn, proxy, lock, successor, overlap, leakedProxy>>
\* Unix: connection_tasks.shutdown() aborts and joins each task (:1486), then
\* release_active_proxy_sessions (:1487). Windows has neither.
Drain == /\ pc = "draining" /\ pc' = "released"
         /\ IF Platform = "unix"
            THEN conn' = "closed" /\ proxy' = "released"
            ELSE UNCHANGED <<conn, proxy>>
         /\ lock' = IF Platform = "windows" THEN "free" ELSE lock
         /\ UNCHANGED <<successor, overlap, leakedProxy>>
\* sdk.shutdown: Closed, then the lifecycle write guard waits for admitted
\* calls (cua-driver-sdk/src/runtime.rs:284-287).
SdkDown == /\ pc = "released" /\ conn # "calling" /\ pc' = "sdkdown"
           /\ leakedProxy' = (proxy = "active")
           /\ UNCHANGED <<conn, proxy, lock, successor, overlap>>
\* The function returns; on Unix the lock file closes now (:1046 `_socket_lock`).
Exit == /\ pc = "sdkdown" /\ pc' = "gone" /\ conn' = "closed"
        /\ lock' = IF lock = "old" THEN "free" ELSE lock
        /\ UNCHANGED <<proxy, successor, overlap, leakedProxy>>

\* The client's call finishes; the detached Windows task keeps its pipe.
CallFinishes == /\ conn = "calling" /\ conn' = "idle"
                /\ UNCHANGED <<pc, proxy, lock, successor, overlap, leakedProxy>>
\* A new daemon takes the free name and starts serving input.
SuccessorStarts == /\ successor = "none" /\ lock = "free"
                   /\ lock' = "new" /\ successor' = "serving"
                   /\ overlap' = (overlap \/ conn = "calling")
                   /\ UNCHANGED <<pc, conn, proxy, leakedProxy>>
Done == pc = "gone" /\ successor = "serving" /\ UNCHANGED vars

Next == Stop \/ Drain \/ SdkDown \/ Exit \/ CallFinishes \/ SuccessorStarts \/ Done
Spec == Init /\ [][Next]_vars

\* SD1. A successor daemon never serves while the old one still runs a call.
SD1 == ~overlap
\* SD2. Shutdown clears the proxy-session registry before the SDK goes down.
\* (sdk.shutdown revokes the sessions themselves; the registry entry is what stays.)
SD2 == ~leakedProxy
====
