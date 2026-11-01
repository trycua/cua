---
title: All your machines, no cluster (self-coordinating Spaces pool)
authors:
  - cua-spaces
created: 2026-10-06
last_updated: 2026-10-06
status: accepted
discussion:
rfc_pr:
implementation:
  - libs/cua/crates/cua-spaces-cluster (engine, pool agent)
  - libs/cua-spacesd/crates/cua-relay (lease, fenced log, RPC mailbox)
supersedes:
superseded_by:
---

# RFC: All your machines, no cluster

## Summary

There is no "cluster" a user creates, joins or serves. Signing in on a
machine and turning on **Provide Spaces** puts it in the account's pool;
`--on auto` (CLI), `on="auto"` (SDK, MCP) and **Auto** in the app place a
Space or a batch on the least-loaded machine that may take it. Interactive
placement is done by the client. Batch runs are handed to one machine of
the account elected through a lease the account's relay grants, and their
job ledger is a log the relay stores and fences with the lease's epoch. If
that machine dies, sleeps or is cut off, another signed-in machine takes
the lease and resumes from the log. Without a relay (direct and tailnet
hosts) the submitting machine coordinates its own runs with a local
ledger, and they pause while it sleeps.

## Motivation

`cua cluster serve | invite | join` made people learn a second system
(coordinator, join codes, admin tokens) next to the account they already
signed in to. Tailscale's bar is: install, sign in, done. The engine behind
the cluster (durable job queue, exactly-once results, rescheduling on node
loss, Modal overflow, capacity-aware placement, 27k browser sessions per
hour in fleet-bench) stays; only who runs it and how it is found changes.

## Goals

- Signing in plus the existing host settings decide a machine's roles:
  a Space itself (share this desktop), a Space provider (provide Spaces),
  both, or neither (a client).
- No user-run coordinator, no invite codes, no admin tokens.
- `on="auto"` everywhere; omitted `on` stays local everywhere.
- Batch runs survive the submitter closing its laptop and the coordinating
  machine dying, with no lost and no doubled results.
- Safety never depends on clocks.

## Non-goals

- A consensus system among the user's machines (Raft, Paxos): the account
  already has a linearizable authority, its relay.
- Placing work on machines the caller cannot already use.
- Hiding the relay's role: it is the control plane, as Tailscale's
  coordination server is.

## Design

### Roles

| Machine | Provides Spaces | Signed in | Can be elected | Runs jobs |
|---|---|---|---|---|
| Spare Mac / Linux box / cloud VM | on | yes | yes (preferred) | yes |
| Host set up without an account session | on | no | no | yes |
| Laptop | off | yes | only when no provider is | no |

Placing work on another machine needs an account session of an enrolled
device: the relay checks the device and the role (owner or editor) and
hands the host a signed principal assertion; the host checks it and its
own **Provide Spaces** setting before it accepts a job (`jobs_for`). A host
never holds an account credential. So a compromised host cannot place work
on the others.

### Placement

- **Interactive (`create_space(on="auto")`)**: the client reads each
  candidate's fresh status (`GetHostSpaces.node`: CPU and memory free,
  kinds, runtimes, problem), ranks the ones that fit by free share
  (spread), and asks the best to create the Space; the host admits or
  refuses from its own free resources (optimistic, shared-state placement
  as in Omega and Modal's scheduler), and a refusal moves on to the next.
  Candidates: this machine, your machines and the ones shared with you as
  an editor that provide Spaces, and Modal only when you connected it and
  no machine fits. A machine with a ready warm-pool member of the Space's
  shape (`HostNodeStatus.warm`) goes first. With more than eight candidates
  the client samples two and takes the less loaded (power of two choices)
  so that many clients do not all pick the same machine from the same
  stale view.
- **Batch (`cua sb run --count N --on auto`)**: the elected coordinator
  runs the existing engine: in-memory ranking per dispatch round, one
  `RunHostJobs` batch per host, hosts admit or reject, one long-poll feed
  per host as its heartbeat.
- **Local** (`on` omitted or `local`): a batch runs on this machine's own
  job runner through a coordinator in this machine's daemon with a local
  ledger. Nothing about it reaches the relay or another machine.

### Coordination: lease, fenced log, mailbox

The relay keeps, per account (a self-hosted relay's OIDC issuer defines
its accounts):

- a **lease**: `epoch` (a counter that only grows and is fsynced before it
  is handed out), `holder`, and an expiry on the relay's monotonic clock;
- a **log**: entries `(seq, epoch, kind, job?, run?, data)` appended only
  by the current epoch, a snapshot that truncates it, and a hash chain;
- a **mailbox**: requests from clients to the holder (submit, cancel, list)
  and the holder's answers. In memory only.

Lease:

- `acquire(id, priority)`: vacant or expired -> `epoch += 1`, holder `id`,
  expiry now + TTL (15 s). The holder renews every 5 s.
- Machines that provide Spaces contend at once; a signed-in client
  contends only while it has a request waiting and nobody holds the lease.
  A holder of lower priority than a waiting contender is told to yield,
  flushes and releases (a laptop hands over to the spare Mac when it comes
  online).
- A relay that restarts keeps the epoch and treats the lease as held for
  a full TTL.

Log append `append(epoch, from_seq, entries)`:

1. refused (`fenced`) unless `epoch` is the current epoch;
2. refused (`conflict`) unless `from_seq` is the head (a retry of entries
   already stored is answered with the head, so appends are idempotent);
3. refused if it would record a second terminal entry for a job.

The holder's engine is the existing coordinator with one change: its
writer funnels every state change (submits, dispatches, results, cancels,
fences, durable Spaces) through the log. For each group it evaluates the
changes against its SQLite state in a transaction it rolls back, appends
the ones that take effect, and only after the relay acknowledged them
applies them locally and acts (sends a dispatch, answers a client). A
`fenced` answer stops the engine at once. A new holder rebuilds its SQLite
state from the snapshot and the log, then reconciles every job in flight
with its host, as a restarted coordinator always did.

Attempts are numbered per job in the log. A dispatch is appended before it
is sent, so every attempt any host ever saw is in the log, and the next
holder's attempts are higher. Hosts already refuse attempts lower than the
newest they saw and cancel a lower one still running (fencing by attempt).
Job ids on hosts are `<pool id>-<job>` with the pool id in the log.

### Why it is correct

Invariants (checked by the model in
`libs/cua/crates/cua-spaces-cluster/tests/pool_model.rs`, by the simulation
in `tests/pool_sim.rs`, and against real histories by
`libs/cua/bench/pool-chaos`):

1. **Single writer per epoch.** Only the machine the relay granted epoch
   `e` can append entries of epoch `e`; once the relay grants `e + 1` no
   entry of `e` is ever appended again. The relay checks the epoch on
   every append (a fencing token checked by the storage, as Chubby's
   sequencers and Kleppmann's fencing tokens), so a paused or partitioned
   old holder whose lease ran out is refused however its clock runs.
2. **No double results.** The log has at most one terminal entry per job
   (the relay refuses a second), and a result is shown to anyone only
   after it is in the log. Every holder's state is the same deterministic
   function of the same log prefix.
3. **Results are real.** A terminal entry carries the attempt that
   produced it on its host, and is appended only while that attempt is the
   job's current one.
4. **No lost jobs.** A submit is acknowledged only once it is in the log.
   Every job in the log is queued, in flight on a host, or ended. A holder
   that dies leaves it in the log; the next one loads it, and in-flight
   jobs are looked up on their hosts and, if lost, run again with a new
   attempt, until they end or use up their attempts (then they end
   `error`, reported). Liveness needs the relay, one machine that can be
   elected, and a host that can run the job to be up long enough.
5. **Access.** A job or Space only goes to a machine the requesting
   account owns or is an editor of, that provides Spaces; the host checks
   both itself on every call.
6. **Every placement is recorded.** Each dispatch is a log entry before
   it is sent; each host also records every job it accepts.
7. **Local stays local.** `on` omitted or `local` never reaches the relay
   or another machine.

A coordinator whose lease ran out may still read hosts' feeds before the
relay refuses its next write. It never cancels a running attempt newer than
any it dispatched (a newer holder's): the model check found that without
this rule a stale holder could cancel the current attempt (results stayed
exactly once; the work ran again).

Hosts keep each account's jobs apart (`<account>/<job id>` on the host), so
an account sharing a machine with another never sees, cancels or fences the
other's jobs, whatever ids it picks.

Under a partition the side without the relay cannot append, so it cannot
dispatch or record a result: it stalls (jobs already on hosts keep
running and their results are collected later). The side with the relay
elects a holder once the lease runs out. Clock skew and jumps only change
when a lease is considered expired (liveness), never who may append
(safety): expiry is measured on the relay's monotonic clock and appends
are checked by epoch. Execution is at least once (a host that was cut off
may still finish an attempt that was moved elsewhere; its result is then
dropped), completion is exactly once.

### Why not

- **Raft/Paxos among the machines**: needs a majority of the user's
  machines up (most users have one or two always-on machines) and doubles
  the moving parts. The relay is already the account's linearizable
  authority (device list, sharing); Vertical Paxos and PacificA show that
  an external configuration master plus epoch-fenced primaries is safe.
- **Gossip membership (SWIM/Lifeguard)** for who is in the pool: the
  account's machine list is authoritative and small; liveness of hosts is
  the coordinator's long-poll heartbeat with a generous suspicion window
  (a slow host is not a dead host, Lifeguard's lesson).
- **CRDT replicated ledger**: merges never conflict, but "exactly one
  result per job" is not a CRDT property; it needs one decider.
- **Per-run coordinators on the submitter**: dies with the laptop; we keep
  this only where there is no relay.

### Durable Spaces

`cua spaces create IMAGE --on auto --durable [--name N]` (`durable=true` in
`create_space`, `SpaceCreateOptions.durable` in the SDK) asks whoever
coordinates to keep a Space: it is placed like a Space `--on auto`, on one
of the account's machines that provide Spaces, and created there through
the account (as `--on host:<machine>` does), so it joins the relay as the
account's and any of its devices reaches it. The coordinator records it in
the log; when its machine stops answering it creates it again on another
machine (the next generation, with a fresh disk), and deletes the old copy
when that machine answers again. Clients list it marked durable and find it
by its name wherever it runs; deleting it by name or id deletes it for good.

### Self-hosted and without a relay

- **Self-hosted relay**: `cua-relay --oidc-issuer <your IdP>` gives the
  same accounts, lease and log.
- **Direct and tailnet hosts** (`cua host setup --direct` / `--tailnet`): no
  lease authority. `--on auto` places on them; a batch is coordinated by
  the submitting machine's daemon with a local ledger. It survives the
  daemon restarting; it pauses while that machine sleeps.

### Data on the relay

The relay stores the log and carries the mailbox, but everything a job is
and produces is sealed on the account's devices before it leaves them
(`pool::seal`, `pool::keys`; the relay's `pool` module checks the shape):

- Each signed-in device has a pool key (X25519, kept with its device key)
  that it certifies with its enrolled P-256 device key and registers
  (`POST /v1/pool/member`). The device id is the hash of the P-256 key, so
  the relay cannot swap a device's pool key unnoticed.
- The account's data keys (one per generation, 256-bit) are wrapped to each
  member's pool key with HPKE (RFC 9180, `DHKEM(X25519, HKDF-SHA256)`,
  HKDF-SHA256, ChaCha20-Poly1305) in auth mode, so a device accepts only
  keys another member wrapped. The keyring (`/v1/pool/keys`) is updated by
  compare-and-swap on its version; within a generation wraps are only added.
- Log entries' data, snapshots, requests and answers are sealed with
  XChaCha20-Poly1305 under the newest generation, bound to the entry's
  position and clear fields (requests to the asking device, answers to the
  request). A retried append resends the same ciphertext, so retries stay
  idempotent.
- A new device is given the keys (every generation, so it can take over) by
  any member's agent within one upkeep round. A machine contends for the
  lease only once it has them. While members that hold the keys are
  enrolled but none is online, a pool request from the new device fails
  after a short grace (`keys_not_shared`, naming the holders and when the
  relay last saw them) instead of waiting; it never starts a new chain on
  its own, since the relay's "last seen" is only a hint (a relay that hid
  holders must not make a device drop the log). The person wakes a holder
  or revokes the ones they no longer have.
- When a member is revoked (or re-keyed), the relay refuses everything
  sealed under the old generation (`rekey`) until a member makes the next
  one without it; the holder does so on its next write. Nothing written
  after a device left is readable by it, even with the relay's storage.
  When no member holds the keys any more, the next device starts a new
  chain (`reset`) and the relay drops the log nobody can read.

What the relay reads: positions, epochs, job ids, the kind of each change,
which entry ends a job, a submit's run and job count, the machine and
attempt of a dispatch (placement), who holds the lease and the keys, and
sizes and timing. It never sees commands, environments, input, images,
outputs, results, durable Spaces' addresses or tokens, or machines' details.
It stores only sealed data (`not_sealed` otherwise). A relay that actively
enrolls a device of its own is given the keys for later writes (the same
trust the mesh places in the relay's device list); the devices holding the
keys are listed in `cua computers`.

## Verification

- **Model check** (`libs/cua/crates/cua-spaces-cluster/tests/pool_model.rs`,
  Stateright, runs in `cargo test`): the lease, the fenced log, hosts'
  attempt fencing, coordinator crashes, partitions (message loss),
  reordered and duplicated messages, lost append answers, and lease expiry
  at any step (so no timing assumption). Checked exhaustively over a
  bounded space (3.5M states by default, 41.6M with three epochs): single
  writer per epoch, no double results, results are real, the client sees
  only committed results, attempts only grow on hosts, every placement is
  recorded, one submit per key, and no lost jobs once faults stop.
  Removing the relay's epoch check, its second-terminal refusal, or
  persisting before sending each yields a counterexample.
- **Model check of the keys** (`tests/pool_keys_model.rs`, Stateright):
  devices that join and are revoked, keyring updates from stale views,
  writers sealing under stale keys, and a revoked device that colludes
  with the relay's storage. Nothing appended after a revocation is
  readable by the revoked device, no generation made after it is wrapped
  to it, every entry is readable by the keys' holders, and once nothing
  more happens every member has the newest keys and every append landed
  (exhaustive: 11,306 states).
  Removing the relay's key gate, its membership check, or the rotation
  yields a counterexample. Sealing does not change the fencing protocol: a
  `rekey` refusal is, to it, an append that did not reach the log.
- **Simulation of the real code** (`tests/pool_sim.rs`): real coordinators
  and host job runners with a seeded schedule of takeovers while the old
  holder still runs, crashes, partitions, and lost appends and answers,
  over a sealed log with the relay's key rules; mid-run a device that holds
  the keys is revoked (more work is submitted right after) and a new one
  joins. The same invariants are checked on the recorded log, and: every
  entry is sealed, none appended after the revocation opens with the keys
  the revoked device had, and the device that joined rebuilds the whole
  log. It found a stale holder canceling, over and over, the attempts of
  jobs a newer holder submitted after its view (the coordinator's fix of
  the finding below covered only jobs it knew); a stale holder now leaves a
  job above its newest id alone.
- **Chaos on real machines** (`libs/cua/bench/pool-chaos`): a Mac Studio
  and a Linux cloud VM that provide Spaces and a laptop, signed in to one
  account on a test relay, with seeded faults and a history checked against
  the invariants. Seven seeds, 108 minutes of faults (71 injected: kill -9
  of daemons, of hosts' cua-spacesd and of the relay; partitions both ways
  and one way; loss, latency and jitter; clock jumps of up to an hour
  either way; a full disk; memory pressure; whole machines paused; a
  daemon away past its lease; sign-out mid-run; Provide Spaces off
  mid-job; the path to the relay rebinding), 538 runs and 7,861 jobs: every
  job ended once with its own result, no epoch wrote after the next was
  granted, nothing ran outside the pool, every placement was recorded and
  no local run left its machine. A real instance termination mid-batch
  (spot-style) re-ran its 25 jobs on the other machine, once each. The
  runs found and fixed: a machine left not sharing after Provide Spaces was
  turned back on, a lease that ping-ponged between a laptop and a machine
  that provides Spaces, an error shown for a request answered during a
  takeover, and clients keeping a device session a restarted relay had
  forgotten. Network, clock, disk and memory faults ran only on the Linux
  VM (the Mac had no root).
- **Sealed, on real machines** (2026-10-07; a Linux EC2 provider, a Mac
  Studio provider and a laptop on a test relay behind HTTPS): three chaos
  seeds, 30 minutes of faults (20 injected: kill -9 of daemons, of a host's
  cua-spacesd and of the relay; whole machines paused; a daemon away past
  its lease; sign-out; Provide Spaces off; a partition; a clock jump; a
  full disk), 116 runs and 1,417 jobs: every invariant above held, all
  3,921 entries the relay stored were sealed, and none of the 101
  runs' secrets (each in its jobs' command, environment and output) was
  anywhere in the relay's state, log or journal. On the same machines: a
  revoked device's keys stopped at once (every entry after it under the
  next generation, wrapped only to the devices left), a re-enrolled device
  was given every generation and took over reading the whole log, revoking
  every device that held the keys started a new chain and dropped the
  unreadable log, and a durable Space whose machine was powered off came
  back on the other machine as generation 2, under its name, about three
  minutes later. The runs found: a fresh machine could not rebuild a log
  holding durable Spaces, and taking over waited on probes of a machine
  that was gone (both fixed).
- **Not covered by the models**: host capacity and placement, user cancels,
  durable Spaces, snapshots, access checks (tested in the relay, host and
  agent tests), and livelock (Stateright checks liveness on terminating
  paths only).

## Compatibility

`cua cluster`, the `cluster_*` MCP tools and `on="cluster"` are removed
(never released). Hosts need the cua daemon from this release; relays need
the pool routes (an older relay answers 404: batches then run attended on
the submitting machine, as without a relay).

## References

- C. Gray, D. Cheriton. Leases: an efficient fault-tolerant mechanism for
  distributed file cache consistency. SOSP 1989.
- M. Burrows. The Chubby lock service for loosely-coupled distributed
  systems. OSDI 2006 (sequencers).
- M. Kleppmann. How to do distributed locking. 2016 (fencing tokens).
- L. Lamport, D. Malkhi, L. Zhou. Vertical Paxos and primary-backup
  replication. PODC 2009.
- W. Lin, M. Yang, L. Zhang, L. Zhou. PacificA: replication in log-based
  distributed storage systems. MSR-TR-2008-25.
- M. Balakrishnan et al. Virtual consensus in Delos. OSDI 2020 (sealing a
  log by epoch).
- D. Ongaro, J. Ousterhout. In search of an understandable consensus
  algorithm (Raft). USENIX ATC 2014.
- A. Das, I. Gupta, A. Motivala. SWIM. DSN 2002; A. Dadgar et al.
  Lifeguard. DSN 2018.
- P. Helland. Idempotence is not a medical condition. CACM 2012.
- M. Shapiro et al. Conflict-free replicated data types. SSS 2011.
- K. Ousterhout et al. Sparrow. SOSP 2013; M. Schwarzkopf et al. Omega.
  EuroSys 2013; A. Verma et al. Borg. EuroSys 2015; I. Gog et al.
  Firmament. OSDI 2016.
- M. Mitzenmacher. The power of two choices in randomized load
  balancing. IEEE TPDS 2001.
- Tailscale. How Tailscale works (2020); How NAT traversal works (2020).
- K. Kingsbury. Jepsen: etcd 3.4.3 (2020): locks are not safe without
  fencing under process pauses.
- Modal. Scaling to one million concurrent sandboxes (2026): stateless
  schedulers, worker-side accept or reject.
