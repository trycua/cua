# jev-use setup guide: scope and verification

## Decision and ownership

Refs #3915; extend the existing draft PR #3916 and its canonical branch. The maintainer requested a step-by-step public guide under `/docs` and a fresh-agent setup test using that guide alone. Preserve the existing example authorship and acknowledgment. Do not implement or depend on the separate policy-head proposal in #3914.

## Initial technical scope

The public guide now lives at `docs/content/docs/how-to-guides/driver/jev-use.mdx` and the example at `libs/cua-driver/examples/jev-use/`. Earlier entries below retain the historical paths and commit pins used when the work was still presented as `typesafe-jev`. The primary supported walkthrough is macOS with the existing Python example; TypeScript is an optional second route. Use the checked-in SDK dependency locks and existing fixture/runner. No changes to the Driver, permissions contract, installers, or model API are planned. Rehearsal failures later justified a small managed verification entry point beside the existing fixture; it does not change either agent's decisions or actions. Link the example README back to the complete setup guide.

The guide must explain the agent/driver boundary, host versus controller machine, human approvals, installation and PATH, Python provisioning, signed browser prerequisite, source checkout before the PR merges, credential handling without shell-history exposure, persistent MCP ownership, fixture readiness, mock versus live proofs, independent readback, failure handling, sequential execution, and cleanup. No dependency on private experiment files, remembered PIDs, old sessions, or conversation context is allowed.

## Initial acceptance evidence

1. Record the original guide's missing prerequisites as the initial failing documentation acceptance case.
2. A new agent session receives only the candidate guide, a blank working directory, and documented human-provisioned prerequisites. No benchmark scripts or conversation history. It must clone the example, install dependencies, run the mock and live Python paths, and prove each exact submitted token through the fixture's independent `/state` endpoint.
3. Record the source SHA, guide hash, prerequisite inventory, agent transcript hash, commands/outcomes, and any intervention. A mock run alone is not a live Jev pass. Pre-existing OS grants or installations are not claimed as freshly reproduced.
4. Run Python unit tests, TypeScript tests/typecheck, docs hygiene, internal links, applicable generator drift, and the docs production build. Keep logs private if they contain local paths; publish only sanitized evidence.
5. Preserve the unrelated staged work in the original workspace. Keep PR #3916's description current and leave it draft if acceptance evidence is incomplete.

## Initial implementation worklog

- Reviewed the existing guide and Python runner. The current guide cannot stand alone: it assumes Driver installation/PATH, uv and a suitable Python, browser availability, OS permissions, an unlocked desktop, and a source checkout that already contains an unmerged example. It also puts a placeholder secret in an export command and lacks exact expected results, readiness checks, and cleanup instructions.
- Searched GitHub for Jev/TypeSafe duplicates. Found active draft #3916 linked to assigned issue #3915; recorded this contribution's scope on the issue and checked out that existing branch in a separate worktree at `6bbeb001e60209eb6ce86bc88a2524947086a5ab`.
- Authenticated GitHub through the existing credential helper without exposing credentials. Verified canonical repository push access for `injaneity`. No fork or competing PR is needed.
- Host preflight: Cua Driver 0.23.2 is installed, its Accessibility and Screen Recording grants read true, and the desktop is currently unlocked. System Python is 3.9.6, so the guide must provision a compatible Python rather than assume `python3` is sufficient.
- The original guide's `main` checkout was independently checked: GitHub returned HTTP 404 for the unmerged example directory. This is the initial failing setup acceptance case, not a hypothetical missing prerequisite.
- Candidate `1c4125c1acc7a29dab1895304882941f428ca1a2`: a fresh Codex agent with no prior conversation verified the installed Driver, grants, and signed Chrome, then stopped at uv installation because the rehearsal sandbox denied macOS's per-user temp directory. No provider calls, fixture mutations, or success claim occurred. Its original transcript/report are retained privately. The next attempt explicitly permits normal dependency-install/temp/cache paths, and the guide now names those execution prerequisites. No host security settings or Cua policy were changed.
- Documentation hygiene, internal links, Cua Driver generator drift, and the production build passed on the first candidate.
- Candidate `fbd5fb3b76d9c5e5c50beedb779171fdc62c9c50`: the second fresh agent installed uv, cloned the public source using the PR fallback, provisioned Python 3.12, and passed all eight Python tests. It then stopped at optional `npm ci`: this host's global npm cache is root-owned. No live request or desktop proof was claimed. The guide and README now use a local npm cache under ignored `.venv`, without changing shared cache ownership, and explicitly sequence the optional route after the Python proof. A third fresh-agent attempt will use the revised guide.
- Candidate `052fe231975099a3e48bde1cfcecb20ead875a1b`: the third fresh agent passed Python setup/tests and fixture readiness, but its next shell call found the fixture gone. The `nohup` child did not survive the shell tool lifecycle, so the mock stopped before opening MCP or calling Jev. Controller-side inspection confirmed the fixture port was closed. Replaced the guide's background launch with an explicitly detached Python process session and documented a persistent-terminal alternative for environments that disallow detached children. Preserved the original failed run; a new agent must rerun the revised guide.
- Candidate `98fc46ea21aa0dda48ad0a5e6caf24e2de981e32`: the fourth fresh agent's detached fixture remained reachable, but the sandbox denied PID readiness checks and cross-call process management. The parent operator verified the exact recorded command and stopped only that retained fixture. No browser or provider verification occurred. This revealed that detached-process recipes still impose an unnecessary agent-harness dependency.
- Architectural simplification: added `verify_setup.py`, reusing the existing `FixtureServer` in a context-managed thread and the unmodified Python/TypeScript runners as child processes. One command now owns startup, independent HTTP verification, evidence, and cleanup. It defaults to mock, requires a key before unattended live execution, refuses existing output directories, and preserves an incomplete summary on failure. The guide no longer asks an agent to manage background PIDs across tool calls.
- TDD: seven new real-process/HTTP checks first failed because the verifier did not exist, then passed. They cover cleanup on success/failure, exact state plus final-event requirements, false-positive rejection, child failure, and missing unattended credentials. All 15 Python tests passed. No Driver/product code or provider decision logic changed.

## Initial fresh-agent verification

Tested candidate: `bebe5e7692c6347b6a090336eac3c0ea4911ec59`.
Guide SHA-256: `c965092c02949f1ca28a285445a28eca92f0233b8c1d04cb583c342a13102541`.

The fifth rehearsal used a new ephemeral Codex CLI 0.154.0 / `gpt-6-astra`
session, with user configuration and host skill discovery disabled. Its working
directory initially contained only the guide. The agent received no previous
conversation, benchmark scripts, private experiment paths, or fixes from earlier
attempts. It cloned the public repository, followed the guide's PR fallback,
and ran the checkout unchanged. The successful transcript contains no private
worklog access. There was no intervention during this attempt.

The human-provisioned prerequisites were the TypeSafe key in the execution
environment, installed CuaDriver/Chrome, approved OS permissions, an unlocked
desktop, and a shell sandbox with the documented dependency/temp/cache access.
This is a **zero-conversation-context setup proof on a provisioned Mac**, not a
claim to automate account creation, OS approval, or a factory-clean OS install.
The earlier second agent performed the previously missing uv installation.

Successful-run environment: Cua Driver 0.23.2 in standard mode; signed system
Chrome; uv 0.12.15; CPython 3.12.14 in a fresh virtual environment;
`typesafe-sdk` 0.6.0; Node 26.8.1; npm 11.19.0. Node/npm are the versions observed
inside the verifier agent, not the parent shell's separate Node installation.

| Proof directory | Independently verified cases | Result |
| --- | --- | --- |
| `proof-mock` | Python/mock | 1/1 |
| `proof-live` | Python/mock, Python/live | 2/2 |
| `proof-both` | Python/mock, Python/live, TypeScript/mock, TypeScript/live | 4/4 |

Every case had exactly two recorded decisions: type the value, then submit.
All seven runner logs ended with the expected verified event and exact token.
The verifier independently read each value from HTTP `/state`; all three
summaries reported `complete: true`. The three live cases are live service
proof, not mocked SDK responses. These logs do not establish exact HTTP retry
counts, resolved model version, token usage, or billing.

The agent and parent audit separately checked the summaries/logs, unchanged
tracked source, and closure of all three fixture ports. The parent confirmed
that the guide and executed example files match the candidate byte-for-byte,
and that credential values are absent from all five attempt transcripts.
Private evidence is retained rather than publishing host paths or raw logs.

Evidence fingerprints:

- Successful agent transcript: `0c54f16b0170445e4702cdb096a4855827329eebb9f342c788edea0c1a5bd117`.
- Fresh agent's final report: `7ad94c0a2479994bdca5200dde033bada6bd980d5e9ad190308c25d8d855ceca`.
- Python/live log in the four-case proof: `a3b9fda2f46b185a0a4dac5de315e55909bfdc0dc81c7bf70016f437ae6389a4`.
- TypeScript/live log: `4ff48370227dc80cae750806b2e93614a28213b6c2ebb02d5e8329609d19f2fa`.

Checks passed:

- 15 Python tests, including seven new verifier boundary/lifecycle tests.
- 7 TypeScript tests and `tsc --noEmit`.
- Public docs hygiene, internal links, Cua Driver generator drift, and production build.
- Unrelated original-workspace staged diff preserved; phase baseline SHA-256
  `5b57ea71dbb7c83df2ae82fd7e435cf1f6ce3a5a7c1c9abc963c4f291787926f`.

At `c4e7307db52c6958a9abbd94e9fa728440e612b9`, the only change after this
certified candidate was this evidence report. The later reader-first guide
revision below has separate validation.
No full Driver desktop matrix was repeated for this documentation/example-only
follow-up. The existing PR remains draft for its normal review, not because the
requested guide or fresh-agent verification is incomplete.

Non-blocking environment notices: the installed Driver advertises an update;
Node reports a deprecated `module.register()` path; npm reports unapproved
optional install scripts. The agent did not update the Driver, approve scripts,
or change policy, and both TypeScript runtime cases passed.

## Reader-first revision

The maintainer's review found that the public guide exposed too much of the
original conversation and verification process. The observable problems were
an opening framed around what Jev is not, unrelated model/video terminology,
PR-specific checkout instructions, raw diagram source in the local renderer,
and extensive process/sandbox/audit details before a first useful result.

Scope: rewrite the same public page around a concrete first task (fill and
submit a form), prerequisites, four short steps, expected results, and a small
troubleshooting section. Keep detailed execution-environment and measurement
notes in the example README and this contributor report. Preserve contributor
acknowledgments in the README. Do not change the agent or verifier code.

At that revision, the checkout pinned the already-tested example revision
`bebe5e7692c6347b6a090336eac3c0ea4911ec59`; readers no longer need to understand
an unmerged PR to obtain runnable code. The guide revision and example revision
are deliberately distinct. New evidence must identify both.

Acceptance: a fresh reader can identify what they will build, what to install,
which commands to run, and how to recognize success without knowing the prior
conversation. The revision reduces the page from 2,193 to 958 whitespace-delimited
words and removes the unrelated model/tool comparisons and PR-specific framing.

A sixth isolated agent followed guide candidate
`ec42a58a8b61824ccf0bacf8460d2fc3996e541f` without intervention. It downloaded the
pinned example, installed locked dependencies in a fresh checkout, and completed
`runs/connection-check` (1/1), `runs/jev-example` (2/2), and
`runs/both-languages` (4/4). Three of the seven cases used live Jev. All recorded
two real actions in the expected order and exact independently observed tokens.
The parent audit separately checked the seven retained logs, complete summaries,
closed fixture ports, unchanged tracked source, and absence of the credential
value from the transcript and report. Pre-provisioned host prerequisites remain
as described above; this was not a factory-clean installation test.

Bash-fence syntax, docs hygiene, internal links, Driver generator drift, and the
production build passed. The revised local preview was loaded in a new isolated
Chrome profile and captured from its exact native window. Native accessibility
readback confirmed the guide title and absence of the old literal Mermaid block.
No production-site rendering claim or visual-model inspection is implied.

| Retained artifact | SHA-256 |
| --- | --- |
| Revised guide | `73ffdd9bfea070d5180672214f70663cc93f7c65634e1d811e15588990a60608` |
| Fresh-agent transcript | `3b2bd7486fbfc76650eeb45d96f60a5a88f628f54eaae76203d7d967aca21866` |
| Fresh-agent report | `c972f02eac6194f54167225391a40832be66ebb3f7f72e14e6656e99729b2159` |
| Local preview screenshot | `eda3b4fea13b6a78d61e2e1b863d9dc56386463809bf5c427f33d59113b507b9` |

The final follow-up only records this evidence; the tested guide and executable
example are unchanged. The first parent audit used the wrong JSONL event name
(`action` instead of `step`); correcting that audit required no live rerun.
Existing Node/npm notices and Rust dead-code/linker warnings remained non-blocking.
No Driver runtime, dependency lock, installation policy, or security setting was
changed. Prior verification history stays in this report, outside the walkthrough.

## Platform-neutral follow-up

The maintainer selected a shared walkthrough with platform-specific setup and
honest native validation status. The guide now separates macOS, Windows, and
Linux desktop prerequisites while keeping download, Python setup, connection
checks, live execution, and results common. Shell-specific npm installation is
in tabs; summary inspection uses `uv run` instead of `.venv/bin/python`. The
Wayland section links canonical compositor requirements. At the maintainer's
request, verification-status labels and coverage tables are omitted from the
walkthrough; actual coverage and blockers remain here and in the PR.

### Launcher correction

The original verifier invoked `npm` directly with Python `subprocess.run`.
On Windows this raised `FileNotFoundError: [WinError 2]` because npm is a command
shim, not an executable resolved like `node.exe`. Extracted the existing runner
command construction and added real-process startup tests. The TypeScript test
reached the same native failure before the fix and reaches the real argument
validator afterward; it does not mock process creation or require a desktop.
The Python test invokes the actual runner's help path.

The fix uses the existing `node --import tsx` pattern already used by the npm
test script. It adds no dependency, shell evaluation, permission change, or
Driver runtime change. Optional Node checks skip only when Node/TypeScript
dependencies are absent; the recorded three-platform runs installed them and
had no skipped tests. Source commit `decc10f6c37168e061d3973dfd9cd21d18da3f68`
was moved with `git cherry-pick -x`, retaining contributor/coauthor credit.
At that phase, the guide pinned runnable revision
`b0e4b6feb0064cedc4dcf04d70a0d270d0568b99`.

| Native host | Python tests | TypeScript tests | Typechecking | Desktop/live evidence |
| --- | --- | --- | --- | --- |
| macOS, Python 3.12.14, Node 22.20.0 | 17 passed | 7 passed | Passed | Prior real browser/live proof; revised-guide rehearsal recorded below |
| Windows, Python 3.12.8 | 17 passed | 7 passed | Passed | Blocked before browser actions; no live requests |
| Linux x86_64, Python 3.12.3, Node 22.20.0 | 17 passed | 7 passed | Passed | Stopped at desktop preflight; no live requests |

### Windows native limitation

The existing interactive Session 1 desktop has Driver 0.19.3, whose preparation
API requires a browser PID. It was not replaced or stopped. Official 0.23.2 and
0.28.2 Windows binaries were downloaded separately and checked against their
component-release asset digests. Owned direct MCP processes exercised those
runtimes without changing the shared daemon or its user policy.

Microsoft Edge 153.0.4234.32 has a valid Microsoft signature. The host's elevated
token was correctly refused automatic protected-browser discovery. A restricted
token, and subsequently a medium-integrity non-administrator token, passed
attestation but the isolated Edge process did not expose a loopback DevTools
endpoint before timeout, on both tested Driver versions. No policy, UAC, ACL,
or browser-sandbox setting was weakened to force a pass.

All four managed attempts retained `complete: false`, no successful action
records, and closed fixture ports. The original daemon remained PID 8088 with
the same policy hash. The TypeSafe credential was not provisioned here and no
live provider request was attempted. This is an environment-specific unresolved
browser-startup result, not a claim that Windows cannot support the integration.
A usable non-elevated browser/Driver desktop run is still required.

Retained Windows evidence archive SHA-256:
`6f49886a0619263f39c72950c96532f18acefc0bc1efc9863e7cd7937cfa1161`.

### Linux native limitation

The first existing Linux sandbox failed workspace preflight for insufficient
free space. On the second, unprivileged dependency setup, both test suites, and
typechecking passed. Its installed Driver is 0.4.2, no supported system Chromium
browser is installed, and the existing XFCE desktop belongs to root. The test
user has no usable top-level windows on the observed X11 display. Reading the
other user's process environment was denied, and sudo requires a password.
No authentication material was copied and no display authorization or host
permission was weakened. Desktop verification stopped before any model call.
A current Driver, trusted system browser, and accessible same-user desktop are
required before claiming an X11 pass. No native Wayland run was attempted.

### Workflow integrity

Environment switching resynchronized the managed Mac workspace's Git metadata
and invalidated its sibling worktree reference. The guide files were compared
with the canonical PR head before recovering that checkout as a standalone
repository. The resynchronized original index was not reset or overwritten;
its checksum changed through the tooling, so the earlier preserved-staging
claim applies only to its original phase. This tooling issue, transfer limits,
and all native setup blockers were recorded separately.

The initial Mac test invocation lacked optional npm dependencies and skipped
the TypeScript launcher check. After the documented locked npm installation,
all 17 Python tests and seven TypeScript tests passed with typechecking. That
initial skip is not counted as launcher validation. The PR remains draft with
Windows/Linux desktop acceptance explicitly incomplete.

### Final macOS rehearsal and documentation checks

A seventh fresh, ephemeral agent followed guide candidate
`a213d0acc731c8041199cdc944014bd05cddec09` against pinned example
`b0e4b6feb0064cedc4dcf04d70a0d270d0568b99`, without prior conversation, host skill
discovery, code edits, or intervention. All seven cases passed, including three
live Jev cases and both languages. The parent independently audited exact
submitted tokens, two actual actions per case, final outcomes, closed fixture
ports, unchanged executable sources/locks, and absence of the credential value
from retained transcripts and reports. This remains a provisioned-Mac proof,
not a new Windows/Linux desktop certification.

Final guide changes after that rehearsal clarify the already-tested Driver
version floor and remove the verification-status wording at the maintainer's
request. Every executable command is unchanged; the parent audit allows only
those exact prose changes. The final guide hash is
`139ec94166234c91f8ca428eb620a404932e9175fe3f5946406134bcac1cfe9e`.

- Tested guide hash: `dc44915168f1486ec64ccd8c04f3753d835b4ce58df0173f23f0927947279c84`.
- Fresh-agent transcript: `123abe5a3da301095b9fffea71ee77f16c5515b5d0bc94648ab28ba4e6889472`.
- Fresh-agent report: `138c22ebb41ac4f6b045cc5a68f585eb9fed0a5eba75a926d601447d1d52baf5`.

Final docs hygiene, links, generated-contract drift, and production build passed.
The recovered shallow checkout initially lacked release tags; fetching the
component tags restored the generator's prerequisite without changing generated
files. A supplementary preview automation did not establish its expected tab
postcondition, so no successful interactive-tab screenshot check is claimed.

### Evidence retention update

Sandbox re-entry replaced ignored files inside the managed Mac workspace as
well as Git metadata. Earlier private launcher scripts and parent transcripts
stored there were lost; their historical fingerprints above must not be read
as a promise that all original raw files remain available. Surviving fresh-agent
reports, guide snapshots, and per-case logs were recovered from their separate
temporary checkouts. Desktop deliveries remain intact; the previously delivered
gameplay video still matches its recorded hash. No local filesystem snapshot
was available for broader recovery.

The latest fresh-agent transcript, report, seven case logs, audit, and harness
are retained outside the resynchronized workspace in the standalone guide
checkout. Native Windows/Linux evidence remains in their separate guest paths;
the Windows archive was also preserved as encoded bytes in the tool transcript.
No lost evidence was reconstructed or represented as retained.

### Credential-free Linux workflow follow-up

The branch remains based on `origin/main`
`aca1985ffc4061d580218f5f850ac57132d90e36`. Exact head
`c4b860515ae5c0cf31d7a36cd17cdb11f6163f2c` ran credential-free workflow
`CI: jev-use` as GitHub Actions run `35237096608`. Both unit jobs passed, and
the Linux job built that exact Driver and verified its advertised MCP tools.
The action loop then failed before its first navigation because the bound
browser tab had an unknown active state. This supersedes no earlier desktop or
live proof and is not recorded as a passing Linux action-loop run.

The pending candidate accepts the first bound tab when no tab is explicitly
active, matching the Driver contract's unknown-active-state behavior, while
still preferring an active tab when one is known. It applies the same selection
to Python and TypeScript. The workflow also gives the isolated bootstrap tab a
stable title and verifies that title over DevTools before starting the MCP
agents. Local verification passed 19 Python tests, nine TypeScript tests,
TypeScript typechecking, workflow YAML parsing, and `git diff --check`. No live
provider workflow was dispatched. A new exact-head GitHub run is still required
before claiming the Linux mock action loop passes.

The current guide uses `FETCH_HEAD` after its single-ref shallow fetch so the
detached checkout does not depend on a remote-tracking ref that a default
single-branch clone may omit. Its shown JSON output now matches `json.dumps`
spacing. Current guide SHA-256:
`35e8e78be21f589b8dc4b98ebb8a934e202c038b16f8794551d33e150430595f`.
