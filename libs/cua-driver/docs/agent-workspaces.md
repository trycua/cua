Implement driver-owned macOS workspaces and exact selected-window access through existing session, capability-manifest, SDK and experimental preview paths.

Scope requested for this implementation:
- Investigate automatic Space creation and exact-window movement on SIP-enabled macOS using Apple APIs and the existing private SkyLight bridge. Verify native postconditions; return explicit unsupported results when operations do not work.
- Bind approved exact windows to trusted session authorization. Discovery, AX, capture, input, browser operations and preview must remain within that selection. Workspace membership does not grant access.
- Keep ownership/lifecycle policy in common Rust code and native operations in platform-macos. Explicit reveal, release and restoration; no automatic application closure or deletion of pre-existing Spaces.
- Keep existing background-input refusals. No foreground or desktop-input fallback.

This is a proposed successor to #2429, selected by the requesting repository user for a narrower macOS implementation against the current session architecture. Relevant adapted code will credit Francesco Bonacci and injaneity. The original PR remains open; this issue does not claim upstream maintainer acceptance or close that contribution.

No new host application, picker, dashboard or preview UI. No merge or release is authorized.

Acceptance evidence: focused shared permission/lifecycle tests, macOS compilation, isolated fixture windows on the development Mac with SIP enabled, native creation/membership/movement/capture/input and focus/cursor oracles, revocation/reuse/session-isolation checks, generated-contract verification, and the applicable canonical desktop gates at the final candidate SHA. Native or environment gaps must remain explicit. The requested development environment excludes VM use, so the mandatory macOS Lume gate cannot be claimed from a local fixture run.

This issue records the public permission and workspace contract for review under rfcs/README.md. Implementation remains draft pending the upstream decision and native evidence.
