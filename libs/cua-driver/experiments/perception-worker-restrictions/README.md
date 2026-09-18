# Cua perception worker process-restriction spike

Status: feasibility experiment only. This is not production Cua Driver code, a
complete sandbox, or a claim that untrusted native code is contained.

This standalone crate tests a narrow launcher shape for a future optional Rust
perception worker. It uses only synthetic strings and allocations. It does not
read screenshots, browser state, input events, credentials, models, or the
network.

The executable evidence applies only to the host where the self-test runs. The
spike does not establish cross-platform enforcement, general descendant
containment, filesystem or network isolation, or closure of every
descriptor/handle that a future production host and its libraries might open.

## Run

From this directory:

```sh
cargo test
cargo run -- self-test
```

The crate declares its own empty workspace so it does not join or alter the Cua
Driver workspace or release graph.

## What the experiment enforces

The parent starts the exact current executable directly and gives the child
only piped stdin and stdout. Stderr is redirected to the null device. There is
no listening socket, discoverable endpoint, reconnect path, shell, or `PATH`
lookup. The probe accepts one synthetic stdin message and returns one fixed
stdout line.

Before launch, the parent:

- creates a unique working directory under the operating-system temporary
  directory and sets mode `0700` on Unix;
- clears the environment and adds only `CUA_WORKER_DIR`,
  `CUA_WORKER_SENTINEL`, and private temporary-directory variables; Windows
  also receives `SystemRoot` when the parent has it because process startup may
  require it;
- explicitly marks an owned sentinel file descriptor or handle as
  non-inheritable, and the child proves that the numeric descriptor or handle
  is invalid after exec/process creation;
- rejects stdin requests larger than 2 MiB before creating a child;
- starts deadline accounting before `Command::spawn` and writes bounded stdin
  on a separate supervised thread, so a worker that never reads cannot strand
  the supervisor after spawn; synchronous `Command::spawn` itself cannot be
  interrupted if the operating system hangs inside process creation;
- on Unix, creates a dedicated session/process group and terminates that group
  on cancellation so descendants that remain in the group cannot keep the
  stdin/stdout pipes open; Windows descendant containment is unsupported;
- caps captured stdout and kills then waits for a child that crosses the cap;
- applies a wall-clock deadline and kills then waits for an overdue child; and
- on Linux, applies `RLIMIT_AS` before exec and verifies that a synthetic heap
  over-allocation fails under the limit. macOS and Windows report memory
  enforcement as unsupported rather than implying a bound.

Every forced termination targets the Unix group before waiting for the direct
child, then verifies that the stdin writer and stdout reader stop and joins
both helper threads. Normal completion requires both stdout EOF and a
non-reaping child-exit query before the sole `wait`: Unix uses `waitid` with
`WNOHANG | WNOWAIT`, while Windows uses `GetExitCodeProcess`. Thus stdout EOF
alone cannot enter a blocking wait or release the group leader identity while a
descendant may still retain the pipes. The executable reports `direct child
reaped` only after `wait` succeeds. The stdin writer sends a start event before
`write_all`; combined with the never-read worker's readiness marker, the test
proves the synchronized 2 MiB request remained pending when the deadline
expired. Unix tests also cover a live parent with a pipe-retaining descendant,
a direct parent that exits before its pipe-retaining descendant, and a live
worker that closes stdout then sleeps. In the exited-parent case, the
descendant waits for EOF on a private pipe whose write end is held until
direct-process exit, then emits its readiness marker. All cases require safe
group termination and joined I/O helpers.

## Enforced restriction versus assumption

| Area | Enforced by this spike | Not established |
| --- | --- | --- |
| IPC | Exact executable, piped stdin/stdout, null stderr, 2 MiB request cap, no listener | The protocol is not authenticated, versioned, or production-ready |
| Environment | `env_clear` followed by a small explicit allowlist; probe compares the complete key set | Native libraries can still discover host facts without environment variables |
| Working files | Unique cwd/temp path; Unix mode `0700`; task-owned cleanup | Windows ACL privacy is assumed from the user's temp-directory defaults and is not certified |
| Descriptor inheritance | The launcher-owned sentinel is explicitly non-inheritable and tested in the child | The spike does not inventory unknown descriptors/handles opened by future host libraries |
| Network | No socket is created or passed; proxy and credential variables are absent because the environment is cleared | There is no kernel network denial. A worker could create a socket using the host network stack |
| Time/output | Deadline accounting starts before spawn and covers concurrent stdin delivery after spawn; stdout is capped; non-reaping liveness prevents stdout EOF from bypassing the deadline; Unix cancellation kills the dedicated process group and reaps the direct child | A hung synchronous `Command::spawn` cannot be interrupted; Windows descendant containment and descendants that escape the Unix group are unsupported |
| Memory | Linux `RLIMIT_AS` is installed before exec and exercised | macOS and Windows have no memory limit in this spike; GPU memory is not covered anywhere |
| Cancellation | Unix process-group kill occurs before the sole direct-child wait; stdin/stdout helpers must stop and join; exited-leader and closed-stdout regressions preserve supervision until cancellation | Parent-death cleanup, escaped Unix descendants, and Windows process-tree cancellation are not implemented; Windows direct-child deadlines rely only on its process handle |

The descriptor test is deliberately scoped to handles the launcher owns. A
production launcher needs a construction rule that creates every non-stdio
handle as non-inheritable and a platform audit proving that libraries cannot
leak additional inheritable handles between enumeration and spawn.

## Platform design needed before implementation

### macOS

- Implement an enforceable memory policy. This host rejected practical
  `RLIMIT_AS` and `RLIMIT_DATA` values with `EINVAL`; candidate mechanisms need
  native evaluation against the intended allocator, memory-mapped model files,
  Metal/MPS allocations, and pressure termination behavior.
- Certify parent-death behavior and process-group cleanup. A supervising XPC
  service or another OS-owned lifecycle mechanism may be needed for reliable
  orphan cleanup.
- Test that direct spawn preserves the required TCC responsibility chain and
  code-signing/library-validation behavior. This experiment makes no TCC claim.
- Audit all open descriptors immediately around spawn and test descriptor
  inheritance in the signed, hardened runtime artifact.

### Windows

- Replace the current explicit `unsupported` memory result with a launcher that
  creates the process suspended, assigns it to a Job Object, configures process
  and job memory limits plus `KILL_ON_JOB_CLOSE`, and resumes it only after the
  assignment succeeds. Assigning a running child leaves an unacceptable race.
- Use an explicit inherited-handle list (`PROC_THREAD_ATTRIBUTE_HANDLE_LIST`)
  and certify it across supported Windows versions. Clearing one owned handle
  is evidence for the primitive, not a complete handle policy.
- Define nested-Job behavior, restricted-token/AppContainer feasibility, child
  process policy, integrity level, desktop/session requirements, and UIAccess
  separation. None is implemented here.
- Apply and verify a private DACL on the working directory rather than relying
  on inherited temp-directory ACLs.

### Linux

- Add `PR_SET_PDEATHSIG` with a parent-PID race check, a private process group,
  and stronger process-tree ownership (or a cgroup-owned supervisor). The
  experiment's session/group prevents ordinary inherited-pipe retention but a
  descendant can deliberately escape it.
- Decide whether production requires network namespace, seccomp socket denial,
  Landlock, cgroup v2 memory/PID limits, mount namespace, or a container. The
  current environment clearing is not network or filesystem isolation.
- Certify the policy on each supported distribution, kernel, libc, display
  server, and packaging format. `RLIMIT_AS` alone does not cover cgroup pressure
  or GPU memory.
- Audit inherited descriptors atomically at spawn; a launcher-owned `CLOEXEC`
  sentinel does not prove closure of arbitrary third-party descriptors.

## Native certification gates

Before describing a worker as restricted or safe for sensitive perception
data, test the signed/package candidate on real supported macOS, Windows, X11,
and Wayland hosts. Required evidence includes:

1. exact inherited descriptor/handle inventory before and after spawn;
2. parent crash, forced termination, child crash, hung child, output flood,
   memory pressure, and descendant-process cleanup;
3. filesystem and network denial tests performed by an adversarial synthetic
   worker, using the final OS-specific containment policy;
4. private-directory ACL/mode inspection under each installer and service
   identity;
5. TCC, Windows session/integrity/UIAccess, Linux compositor/portal, code
   signing, and packaging behavior; and
6. proof that logs, crash reports, temporary files, and diagnostics cannot
   retain perception input or protocol payloads.

Until those gates pass, the useful conclusion from this spike is narrower: a
directly supervised child can have deterministic stdio IPC, a cleared
environment, a private temporary cwd, bounded output/time, explicit reap, and
some platform primitives, but process separation alone is not a sandbox.
