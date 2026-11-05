---
title: Browser compatibility behavior and existing-profile trust boundaries
authors:
  - HaniLabs
created: 2026-11-05
last_updated: 2026-11-05
status: review
discussion: https://github.com/trycua/cua/issues/6549
rfc_pr:
implementation: []
supersedes:
superseded_by:
---

# RFC: Browser compatibility behavior and existing-profile trust boundaries

## Summary

Propose a browser compatibility mode for authorized automation sessions with explicit, per-session opt-in before any behavior that changes page-visible automation indicators or browser properties is used on an approved existing profile. Request a maintainer decision on whether such a mode belongs in Cua, what specific behavior it may use, and how its boundaries differ for Driver-launched isolated browsers and existing profiles. This is a request for discussion, not a request to merge an implementation.

## Motivation

Some browser-based workflows encounter compatibility or sign-in obstacles when a site responds differently to an automated browser. These differences may prevent a user from completing an otherwise authorized workflow. The reasons for any particular site's behavior are not established by this proposal, and a compatibility mode cannot guarantee that a site will allow a session.

Cua Driver can launch a browser with an isolated profile or attach to an approved existing profile. The current existing-profile policy expressly refuses registration of document-start scripts. An [experiment on my fork](https://github.com/HaniLabs/cua/commit/86872e44351e2861e53e2bab34b4903d2d789480) explores different launch and page behavior. It applies the launch changes by default and provides an opt-out only for the attach-time script. It has not been accepted upstream and is not evidence of a supported or validated contract. Its existing-profile behavior would alter the current trust boundary and requires a separate decision.

Related [issue #3136](https://github.com/trycua/cua/issues/3136) addresses detecting browser challenges and rate limits and stopping retries. It explicitly excludes stealth fingerprint emulation. This RFC does not treat that issue as approval or as a request for this behavior.

## Goals

- Decide whether browser compatibility behavior that changes page-visible automation indicators or browser properties is appropriate for Cua at all.
- If appropriate, define a clear, explicitly selected contract that distinguishes Driver-launched isolated browsers from approved existing profiles.
- Make supported behavior, defaults, limitations, and errors observable to callers.
- Define evidence needed for every supported operating system and browser before any behavior is released.
- Preserve user control and make it clear that a delivered browser action does not imply a site accepted the session or that a task succeeded.

## Non-goals

- Solve CAPTCHAs, automate challenges, evade rate limits, or defeat a site's access controls.
- Guarantee access to a site or imply that an authorized user is entitled to automate a particular service.
- Conceal use of automation from a user or silently modify existing profiles.
- Infer that different browser or operating-system combinations support identical behavior without evidence.
- Collect, retain, or publish credentials, session data, browsing history, or page content for telemetry.

## Terminology

An **isolated browser** is launched by Cua Driver with a Driver-managed profile. An **existing profile** is a browser profile already belonging to a user and attached only after the applicable authorization. **Compatibility mode** here is a proposed, explicitly selected behavior; its allowed operations and supported surfaces are unresolved.

## Current state

The existing-profile protocol policy explicitly refuses registration of document-start scripts. The fork experiment described above changes that policy for its attach-time behavior. It is a historical experiment rather than an approved upstream design or a validated cross-platform contract.

## Proposal

Propose a limited compatibility mode that is disabled by default. Define its public API and supported behavior before implementation. For an approved existing profile, require explicit user opt-in for the specific session and clearly communicate the mode and its effects to the caller. A normal profile-attachment approval alone would not enable the mode. Maintainers may reject the proposal or define narrower boundaries during review.

Treat the two browser ownership cases as separate decisions:

1. **Driver-launched isolated browser:** determine whether any browser launch configuration affecting page-visible automation signals is allowed, with explicit documentation of effects and limitations.
2. **Approved existing profile:** require explicit opt-in for the specific session in addition to normal profile-attachment authorization. Retain the current restrictions unless a separate review authorizes a specific change. Registering code that runs in future documents changes page behavior and must not be described as a read-only operation. Review whether it is appropriate at all, how consent is conveyed, which targets and documents it may affect, and how its lifecycle and removal would be enforced.

Callers should receive a clear indication when a requested mode is unavailable or only partially supported. Browser blockers and rate limits should be surfaced and handled responsibly rather than retried automatically; detection behavior remains separate work. Do not assume the fork experiment's flags, page scripts, defaults, or browser-property substitutions are the proposed or accepted design.

## Alternatives considered

- **Keep the existing behavior and do not add compatibility mode.** This preserves the existing trust boundary and avoids representing altered browser properties to websites. Users can perform affected steps manually or use an available approved integration.
- **Detect and report blockers, then stop or request user handoff.** This follows the direction of #3136 and makes the failure visible, but does not change how a site evaluates a browser session.
- **Use a service's supported API or other authorized interface.** This is preferable when available and permitted; it may not cover every browser task.
- **Permit limited opt-in behavior only for Driver-launched isolated browsers.** This avoids changing an existing personal profile, but still needs review of website interactions, caller expectations, cross-platform support, and the appropriate boundary.
- **Permit behavior for existing profiles as well.** This has a broader trust and compatibility impact and would need separate consent, policy, lifecycle, and security review; it should not be inferred from approval of isolated-browser behavior.

## Compatibility and migration

Keep existing behavior and profile restrictions as the baseline while this RFC is undecided. If a new mode is accepted, make activation explicit, document platform and browser support, and provide a rollback path that restores the prior behavior. Review whether any public API, launch option, or session capability requires a compatibility notice. Unsupported combinations must report the limitation rather than silently claiming the mode is active.

No migration or automatic change to existing profiles is proposed. Acceptance of an RFC would be required before implementation work intended for upstream, and the accepted RFC would be merged before or with any implementation PR.

## Security, privacy, and telemetry

The Driver must honor its existing authorization model and target boundaries. Changes to an approved existing profile's permitted protocol surface and any code executed in documents need explicit review; an existing-profile attachment is not blanket consent for additional behavior. The design must address user disclosure, session scoping, cleanup, and what happens if setup or cleanup fails.

The mode must not solve challenges, automatically work around site blocks, or conceal a block from the caller. Telemetry must not contain credentials, cookies, profile contents, URLs with private data, page contents, or session identifiers. This RFC requests a design decision and does not report a vulnerability.

## Implementation plan

1. Seek a maintainer decision on whether any such mode should exist and, separately, whether approved existing profiles may support it with explicit per-session opt-in.
2. If accepted, record the concrete API, browser and platform support, consent, policy boundaries, failure behavior, and test plan before starting upstream implementation.
3. Implement only the approved scope in focused changes and verify the acceptance evidence before release. Preserve a rollback to current behavior.

## Test and acceptance plan

If the proposal is accepted, agree on the contract and test plan before implementation. Evidence should include:

- Contract tests for explicit activation, default behavior, unsupported combinations, setup errors, cleanup, and rollback.
- Existing-profile policy tests proving the agreed permitted and refused operations and that no unrelated targets or profiles are affected.
- Browser tests using local controlled pages and fixtures, including navigation and session lifecycle; no real account or third-party service is required.
- Focused tests and native verification on each affected supported operating system and browser, with concrete limitations documented when equivalent behavior is unavailable.
- Regression coverage showing that challenge and rate-limit handling remains observable and does not initiate automatic bypass or retry loops.

No cross-platform or upstream validation is claimed for the fork experiment.

## Unresolved questions

1. Should Cua support any modification to page-visible automation indicators or browser properties, even on an explicit opt-in basis?
2. If yes, which isolated-browser behaviors and platforms are acceptable, and how should callers discover and consent to them?
3. Is explicit per-session opt-in sufficient to consider existing profiles, or should they remain outside this feature? What additional authorization, protocol restrictions, scope, and cleanup would be necessary?
4. How should partial support or setup failure be surfaced without creating a misleading guarantee?
5. Is blocker detection and user handoff sufficient for the user problem, or is another supported integration more appropriate?

## Decision record

Pending maintainer review and decision. No acceptance, implementation approval, or release is implied.
