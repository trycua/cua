# Jev setup guide: scope and verification

## Decision and ownership

Refs #3915; extend the existing draft PR #3916 and its canonical branch. The maintainer requested a step-by-step public guide under `/docs` and a fresh-agent setup test using that guide alone. Preserve the existing example authorship and acknowledgment. Do not implement or depend on the separate policy-head proposal in #3914.

## Technical scope

Expand `docs/content/docs/how-to-guides/driver/use-typesafe-jev.mdx` rather than create another integration. The primary supported walkthrough is macOS with the existing Python example; TypeScript is an optional second route. Use the checked-in SDK dependency locks and existing fixture/runner. No changes to the Driver, permissions contract, installers, or model API are planned. Rehearsal failures later justified a small managed verification entry point beside the existing fixture; it does not change either agent's decisions or actions. Link the example README back to the complete setup guide.

The guide must explain the agent/driver boundary, host versus controller machine, human approvals, installation and PATH, Python provisioning, signed browser prerequisite, source checkout before the PR merges, credential handling without shell-history exposure, persistent MCP ownership, fixture readiness, mock versus live proofs, independent readback, failure handling, sequential execution, and cleanup. No dependency on private experiment files, remembered PIDs, old sessions, or conversation context is allowed.

## Acceptance evidence

1. Record the original guide's missing prerequisites as the initial failing documentation acceptance case.
2. A new agent session receives only the candidate guide, a blank working directory, and documented human-provisioned prerequisites. No benchmark scripts or conversation history. It must clone the example, install dependencies, run the mock and live Python paths, and prove each exact submitted token through the fixture's independent `/state` endpoint.
3. Record the source SHA, guide hash, prerequisite inventory, agent transcript hash, commands/outcomes, and any intervention. A mock run alone is not a live Jev pass. Pre-existing OS grants or installations are not claimed as freshly reproduced.
4. Run Python unit tests, TypeScript tests/typecheck, docs hygiene, internal links, applicable generator drift, and the docs production build. Keep logs private if they contain local paths; publish only sanitized evidence.
5. Preserve the unrelated staged work in the original workspace. Keep PR #3916's description current and leave it draft if acceptance evidence is incomplete.

## Worklog

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

## Completed fresh-agent verification

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

The only final change after this certified candidate is this evidence report;
the executable example, dependency locks, and tested public guide are unchanged.
No full Driver desktop matrix was repeated for this documentation/example-only
follow-up. The existing PR remains draft for its normal review, not because the
requested guide or fresh-agent verification is incomplete.

Non-blocking environment notices: the installed Driver advertises an update;
Node reports a deprecated `module.register()` path; npm reports unapproved
optional install scripts. The agent did not update the Driver, approve scripts,
or change policy, and both TypeScript runtime cases passed.
