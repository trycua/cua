# Chaos runs of all your machines

Jepsen-style fault injection against real machines signed in to one
account, while runs (`cua sb run --on auto`), interactive Spaces (`cua spaces
create --on auto`) and runs that must stay on one machine are going. Every
operation and fault is recorded; afterwards the history is checked against
the invariants in `rfcs/pool-self-coordination.md`.

```sh
python3 chaos.py --nodes nodes.json --seed 7 --minutes 20 --out results/
```

| File | What it does |
|---|---|
| `node-up.sh` | Signs a machine in to a test relay in an isolated home and runs its daemon (`ROLE=provider` also sets it up to provide Spaces). |
| `host-again.sh` | Starts a machine's host service again after its cua-spacesd was killed. |
| `chaos.py` | The workload, the seeded fault scheduler (one or two faults at a time, healed after 15 to 90 s), the history, and the collection of the relay's pool log, its audit and each machine's job ledger. |
| `check.py` | Checks a run: no double results, no lost jobs, results are real, one writer per epoch, only allowed machines, every placement recorded, local runs stayed local, and the relay held only sealed data (no run's secret, put in its command, environment and output, anywhere in its state, log or journal). Exits 1 on a violation. |
| `nodes.example.json` | How to reach each machine, its role, and the faults it allows. |

Faults: kill -9 of a daemon, of a host's cua-spacesd and of the relay;
partitions from the relay (both ways or one way, `iptables`), packet loss,
latency and jitter (`tc netem`), clock jumps forward and back, a full disk,
memory pressure (`stress-ng`), a whole machine paused (SIGSTOP), a daemon
away past its lease that comes back with stale state, signing out mid-run,
Provide Spaces turned off mid-job, the path to the relay dropping and coming
back on a new port. Network, clock, disk and memory faults need root and
run only on machines that allow them; never point them at a machine whose
network you do not own. Test against your own relay (`cua-relay
--oidc-issuer ...`), never a production one.
