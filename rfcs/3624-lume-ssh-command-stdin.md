---
title: Forward command stdin through lume ssh
authors:
  - injaneity
created: 2026-09-07
last_updated: 2026-09-07
status: review
discussion: https://github.com/trycua/cua/issues/3624
rfc_pr: https://github.com/trycua/cua/pull/3625
implementation:
  - https://github.com/trycua/cua/issues/1514
supersedes:
superseded_by:
---

# RFC: Forward command stdin through lume ssh

## Summary

Make command-mode `lume ssh` deliver caller-supplied stdin as bytes, then EOF,
through either existing SSH backend. The CLI owns the decision to forward input;
shared clients never discover or inherit process stdin themselves. Add
`--no-stdin` for explicit empty input. Keep this one vertical slice independent
of output-format, desktop-session, and SSH infrastructure changes.

This is a proposed contract and implementation plan, not completed behavior.
Implementation waits for a maintainer decision in the linked RFC issue.

## Motivation

[#1514](https://github.com/trycua/cua/issues/1514), reported by @pvulgaris,
demonstrates that piping a script into `lume ssh vm 'bash -s'` never delivers
its input. A timeout replaces the expected script execution. Users must stage
files or use system SSH directly for an ordinary remote-command workflow.

The smallest useful slice crosses the entire input path, not just a new API:

```text
pipe/file → CLI input selection → SSH backend → guest stdin → EOF → exit status
```

## Goals

- Execute a piped or redirected script and observe its guest-side effect and exit.
- Preserve binary input, byte order, and EOF through both transports without a PTY.
- Bound queued input independently of total input length.
- Preserve output/status reception after local EOF and clean up local input work
  after remote exit, error, timeout, or cancellation.
- Keep authentication and embedding-process stdin separate from command input.

## Non-goals

- Raw output fixes: #1513 and active contributor PR #1701 own that work.
- Live stdout/stderr streaming, separate output streams, or a new result format.
- Interactive-shell/PTY changes or an MCP stdin parameter.
- Desktop execution, GUI permissions, VM provisioning, or E2E orchestration.
- Event-loop lifetime changes owned by active PR #3272.
- Retrying remote commands or guaranteeing remote process-tree termination.

## Terminology

- **Command mode:** one remote command, rather than the existing interactive shell.
- **Closed input:** consume no host input and send EOF to the remote command.
- **Input source:** an explicitly supplied, borrowed readable descriptor, not a
  process-global lookup. An execution may own a duplicate for its local lifecycle.
- **Local cancellation:** stop input work and close owned transport resources;
  it does not prove that guest descendants have terminated.

## Current state

Source baseline: `25e578b02459955d5ac91c1276742fac9c6de972`.

- [CLI](../libs/lume/src/Commands/SSH.swift) selects NIO first and falls back to
  system SSH for `SSHError.connectionFailed`. It passes no command input.
- [NIO client](../libs/lume/src/SSH/SSHClient.swift) sends an exec request and
  collects output/status. `CommandExecHandler` has no stdin producer.
- [System client](../libs/lume/src/SSH/SystemSSHClient.swift) uses null stdin and
  forced askpass authentication. It waits for exit before draining output pipes.
- [MCP](../libs/lume/src/Server/MCPServer.swift), clipboard, and other internal
  callers also use command execution; none should acquire process stdin implicitly.
- `SSHResult.output` is text. Binary-input tests must not rely on binary stdout
  surviving a round trip while #1513 is unresolved.

Revalidation on 2026-09-07 found no linked #1514 implementation or competing stdin
PR. #1701 and #3272 overlap files but own different behavior. Recheck their state
before implementation and preserve their authorship if their changes are reused.

## Proposal

### CLI contract

Proposed usage, with options before the VM name to avoid remote-argument ambiguity:

```bash
printf 'printf "stdin-ok\\n"\n' | lume ssh my-vm 'bash -s'
lume ssh my-vm 'bash -s' < script.sh
lume ssh --no-stdin my-vm 'some-command'
```

| Invocation                     | Input behavior                                                                   |
| ------------------------------ | -------------------------------------------------------------------------------- |
| Command, default               | Forward fd 0, including pipes, files, and a terminal; no PTY or raw-mode changes |
| Command with `--no-stdin`      | No reads from fd 0; send EOF                                                     |
| No command                     | Existing interactive-shell behavior                                              |
| No command with `--no-stdin`   | Argument validation error before connection                                      |
| Existing library/MCP execution | Explicit closed input; never read process stdin                                  |

Use the long option only in this slice. Test parsing so a remote command's own
`--no-stdin` argument is not interpreted as a Lume flag after command parsing begins.
If inherited fd 0 is already closed, treat it as empty input. Other read failures
are execution errors, not successful EOF. Preserve remote exit status when the
remote command intentionally exits without reading the entire stream.

### Input ownership and API surface

Keep the existing public `execute(command:timeout:)` signatures and `SSHResult`
shape. Delegate through an internal overload with explicit closed/descriptor
input. Default existing callers to closed input. This standardizes EOF for the
NIO no-input case as well as avoiding unintended inheritance.

Only `Commands/SSH.swift` supplies `FileHandle.standardInput.fileDescriptor`.
The internal input representation must not contain a `.standardInput` case that
looks up a global descriptor. Tests inject pipes/files directly. Do not close
caller-owned descriptors or change their terminal mode or shared file flags;
`dup` alone does not isolate flags such as `O_NONBLOCK`.

### NIO lifecycle

1. Connect/authenticate and open the session channel without consuming input.
2. Send the exec request with a reply requested. Start reading only after request
   acceptance. Verify the pinned NIOSSH success/failure event contract; a completed
   outbound write promise alone must not be assumed to mean server acceptance.
3. Read bounded byte chunks off the event loop and write normal `SSHChannelData`.
   Start with a maximum 64 KiB read and one outstanding read/write cycle. Respect
   channel writability; do not enqueue an unbounded async sequence or read-to-end.
4. At input EOF, finish pending writes and close only the write half of the SSH
   session, preserving inbound output and exit status. Send EOF exactly once.
5. On request rejection, remote close/exit, timeout, or task cancellation, stop
   scheduling reads, wake/cancel a stalled reader, and close execution-owned
   resources. Keep result completion exactly once, including disconnect races.

Descriptor reading needs explicit cancellation. Do not put an indefinitely
blocking pipe read in a detached task and call that cancelled when its parent
returns. Use readiness-driven input with a cancellation wakeup; regular files
may need a separate bounded read strategy. Test both descriptor kinds. Any
cross-executor state must satisfy Swift 6 concurrency checking without broadly
marking the reader unchecked Sendable.

The normal sequence is `connecting → request pending → forwarding → input EOF →
awaiting result → finished`; error/cancellation may terminate any state. A remote
exit may finish directly from forwarding. No terminal transition restarts reads.

### System SSH lifecycle

Use the supplied descriptor as the subprocess input (or null for closed input),
retaining forced askpass as a separate authentication path. Let OpenSSH handle
input flow control and EOF; do not create a second user-space input pump here.
The process must not take ownership of the caller's original handle.

Drain stdout and stderr concurrently while the process runs, then assemble the
same existing text result. This is internal pipe draining, not live output or
binary-output redesign. It is necessary for commands that emit more than a pipe
buffer before finishing input. Existing aggregate output buffering remains a
known limitation outside this input-memory guarantee.

Tie timeout and cancellation to the subprocess lifecycle, cancel timeout work on
completion, and bound local termination. If an internal async wrapper is needed
for cancellation, preserve the existing synchronous entry point for other callers.
Do not introduce a general process framework or copy unrelated desktop machinery.

### Fallback and failure semantics

- Only fall back after a confirmed pre-execution connection failure, before any
  input was read. Test that the complete original byte sequence is still available.
- Once a remote exec request may have been sent, never automatically replay the
  command or input, even if the transport subsequently reports a connection error.
- Authentication refusal and exec rejection do not trigger fallback.
- Local EOF is not completion; wait for output/exit status using existing semantics.
- A remote command may legally exit early. Stop input delivery and report its exit
  status rather than hanging on the producer or crashing from a broken pipe.
- Timeout/cancellation stops local work; do not claim the guest process tree stopped.

### Platforms and file scope

Lume remains macOS-hosted. The command-input protocol is guest-OS independent;
verify macOS and Linux guests rather than tying it to desktop or TCC state.
Expected product files are the CLI and the two SSH clients, with at most one small
input-reader helper. Focused tests live beside `SSHClientTests.swift`, plus a
controlled SSH integration fixture. Update command help and its generated docs
only when implementation changes the CLI.

## Alternatives considered

- **Opt-in `--stdin`:** less default input consumption, but leaves the reported
  ordinary SSH pipeline broken unless users learn another flag.
- **Implicit stdin in shared clients:** concise but violates input ownership for
  MCP and embedded callers. Rejected.
- **Read everything or stage a temporary file:** simple, but unbounded memory or
  unnecessary persistence of possibly sensitive input. Rejected.
- **Replace NIO with system SSH:** substantially larger architectural change.
- **Combine binary/live output:** useful separately, but overlaps #1701 and hides
  whether this slice fixed input. Test input with guest-side digest oracles instead.

## Compatibility and migration

The forwarding default is deliberate and observable: a command inside a shell
read-loop can now consume the loop's remaining stdin, as ordinary SSH can.
Document `--no-stdin` and test that it leaves a shared input descriptor unread.
Do not change interactive shell behavior or silently enable input for MCP.

Require approval of the default, the escape hatch, and the no-input EOF behavior
before implementation. Proposed product PR title after approval:
`fix(lume): forward stdin to remote commands`. Confirm release metadata against
the final scope. This documentation-only PR does not release Lume.

The CLI option provides a per-call rollback to no input. Reverting the input slice
must not revert other contributors' output or event-loop changes.

## Security, privacy, and telemetry

Forwarded bytes are intentionally accessible to the selected remote command.
Input transport does not make a guest trustworthy or protect data from that guest.
Preserve existing authentication/host-trust mechanisms; do not expand their scope.

Do not log, trace, persist, interpolate into command arguments, or include input
payloads in telemetry. Authentication must not consume command stdin. Use synthetic
fixtures, not secrets, for tests and artifacts. No new credentials, permissions,
public endpoints, or privileged event dispatch are introduced.

## Implementation plan

One implementation workstream after the decision; review in these increments:

1. Add explicit input ownership and deterministic reader/state tests; keep existing
   public call signatures. Establish EOF/cancellation and descriptor guarantees.
2. Wire CLI selection and both transports. Add internal output-pipe draining only
   where required for full-duplex correctness. Preserve no-replay fallback.
3. Add transport parity tests, command documentation, and exact-candidate SSH VM
   evidence. Do not land a NIO-only version as a complete fix for #1514.

Keep the draft PR description current. Do not rebase or merge overlapping PRs
implicitly. If #3272's lifecycle defect blocks validation, record the dependency
and coordinate there instead of incorporating an unattributed replacement.

## Test and acceptance plan

| Layer            | Cases and observable evidence                                                                                                                                                                                  |
| ---------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| CLI/parser       | Default descriptor selection, `--no-stdin`, invalid no-command combination, remote flags preserved, unchanged interactive branch                                                                               |
| Input/handler    | NUL/invalid UTF-8 unchanged; ordered chunks; maximum outstanding input bounded; zero reads before acceptance; request rejection; EOF once after final write; output still received after EOF                   |
| Ownership        | Closed input leaves a shared pipe untouched; caller descriptor stays open with unchanged flags; library/MCP sentinel input is not consumed                                                                     |
| Lifecycle        | Stalled open producer, nonreading consumer, request failure, early remote exit, read/write error, disconnect, timeout, cancellation; no orphan reader, leaked owned descriptor, or double promise completion   |
| Transport parity | Run identical script/file/pipe/empty-input cases through each backend; wrong password does not consume command data; forced pre-execution fallback preserves all input; no retry after execution may begin     |
| Full duplex      | Guest emits more than a pipe buffer on each output stream before reading input; command completes without deadlock, retaining existing output semantics                                                        |
| Real guest       | Source-built CLI pipes a marker script with a chosen nonzero exit; empty input reaches EOF; deterministic 8 MiB binary payload yields matching guest-side length and SHA-256; repeat on macOS and Linux guests |

Use a local controlled SSH fixture for deterministic transport tests; select the
backend through internal test seams, not a new public backend flag. Exercise
actual subprocess/channel behavior, not only mocked `SSHResult` construction.
For binary input, have the guest emit a small ASCII digest/count. A successful
binary round trip through stdout is not an acceptance criterion for this slice.

Run focused Swift tests during development, then `swift test` and `swift build`
from `libs/lume`, affected formatting/generated-help checks, and ordinary PR CI.
Record exact candidate SHA, guest OS, selected backend, assertions, and cleanup
for focused VM smokes. No Cua Driver desktop E2E matrix or browser baseline is
needed for this SSH-only change. Missing a guest/backend lane is an explicit gap,
not evidence of parity.

Planning validation is source/history review and Markdown/diff checks only; no
product test or VM run has been performed for this RFC.

## Unresolved questions

- Approve default forwarding, command-only `--no-stdin`, and EOF for library calls
  that supply no input. If the default is rejected, revise compatibility and scope.
- Confirm the pinned NIOSSH request-acceptance hook and cancellation-safe input
  primitive before committing to a concrete helper type.
- Confirm available macOS/Linux SSH fixtures and sequencing with #1701/#3272.

## Decision record

Pending. The maintainer selected planning on #1514; this does not mark the new
public contract accepted. Record feedback, disposition, and remaining risks in
#3624 before product implementation.
