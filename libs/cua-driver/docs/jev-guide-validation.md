# Jev setup guide: scope and verification

## Decision and ownership

Refs #3915; extend the existing draft PR #3916 and its canonical branch. The maintainer requested a step-by-step public guide under `/docs` and a fresh-agent setup test using that guide alone. Preserve the existing example authorship and acknowledgment. Do not implement or depend on the separate policy-head proposal in #3914.

## Technical scope

Expand `docs/content/docs/how-to-guides/driver/use-typesafe-jev.mdx` rather than create another integration. The primary supported walkthrough is macOS with the existing Python example; TypeScript is an optional second route. Use the checked-in SDK dependency locks and existing fixture/runner. No changes to the Driver, permissions contract, installers, or model API are planned. Link the example README back to the complete setup guide.

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
