# MCP candidate qualification

The `CI: Driver MCP candidate` workflow builds the exact pull-request head
on hosted Linux and Windows runners. It runs the envelope receiver and service
tests, then uploads unsigned development binaries with their source SHA,
toolchain, build profile, version, and SHA256 hashes. It has read-only repository
permissions and no Fleet credentials. It does not publish a release, install a
product, change an image, or connect to a Fleet computer.

These artifacts support separately authorized disposable-guest tests. Check the
workflow repository, source SHA, successful jobs, and every binary hash before
using an artifact. Do not select an artifact by version alone: an unreleased
source build can report the same package version as a released binary.

## Live test gate

For each selected Linux and Windows Fleet image:

1. Record the immutable image reference and installed Driver, computer-server,
   and MCP-wrapper versions. Use an owned disposable claim and namespace with
   explicit resource limits and verified cleanup ownership.
2. Stage the exact candidate separately. Preserve computer-server. Start Driver
   in the guest's interactive desktop session through its existing socket or
   named pipe. Enable the envelope opt-in in both the daemon and MCP proxy.
3. Disable raw request/response logging before action-bearing tests. Reuse the
   existing named MCP service and access boundary; do not introduce a new
   network endpoint or substitute the operator's local desktop.
4. Use matching generated bindings with
   `sb.driver.connect(service="mcp", transport="mcp")`. Prove a useful guest
   effect with fresh before/after Driver state and an independent guest
   postcondition. A successful call alone is not the oracle.
5. Verify independent sessions, cancellation without claiming rollback, stale
   session rejection after replacement, access rejection, and continued
   computer-server functionality. Verify receiver and MCP-session cleanup
   independently from Fleet resource cleanup.
6. Delete only the task-owned resources and confirm their absence. Preserve
   failure and cleanup evidence, including any incomplete result.

A staged development-binary result is not packaged-image certification or the
full canonical desktop matrix. A release/image enablement decision still needs
the applicable exact-candidate desktop harness evidence, packaged-image replay,
and separate rollout approval. Keep existing defaults and images unchanged.
