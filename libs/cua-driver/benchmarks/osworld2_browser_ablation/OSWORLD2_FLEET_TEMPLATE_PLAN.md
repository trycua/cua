# OSWorld 2 + Cua Driver Fleet template plan

## Goal

Replace the disposable browser-ablation pilot with one reusable OSGym template
contract that runs the pinned OSWorld 2 release and supports Cua Driver without
leaking credentials, task contents, or evaluator data.

## Definition of done

- A named OSGym template can create one Fleet VM from an immutable OSWorld 2
  image reference.
- The VM can pass its readiness check, accept the required OSWorld 2 setup,
  and run Cua Driver at the recorded Cua source revision.
- The benchmark records every immutable input, exposes only required services,
  and cleans up claims and transient runtime material.
- A small pilot proves provisioning, driver connection, and one permitted
  evaluation path before any larger benchmark run.

## Plan

1. Freeze the OSWorld 2 release contract.

   Record one release identifier for the OSWorld code, task classes, gated
   assets, VM image, and mocked-web configuration. Use immutable image digests
   and reject a run if any supplied input differs from the recorded release.

2. Define the base image boundary.

   Start from the official OSWorld 2 Ubuntu image. Build a derived immutable
   Fleet image only for stable guest requirements: the Cua Driver runtime
   dependencies, the workspace readiness service, network configuration, and
   non-secret diagnostics. Keep task assets, model credentials, GitLab tokens,
   and run-specific state out of the image.

3. Add a first-class OSGym sandbox template.

   Define the VM image, four CPU cores, eight GiB memory, readiness probe, and
   required services in one OSGymSandboxTemplate. Start from the pilot's
   control, browser-debugging, noVNC, and VLC endpoints. Add an explicit Cua
   Driver access contract: either an authenticated MCP service or the existing
   authenticated loopback bridge, selected once and documented. Do not expose
   arbitrary guest ports.

4. Keep Cua Driver revision-specific.

   Build or upload the driver artifact from the recorded Cua revision at run
   time, then verify its version and connection before evaluation. This keeps
   the reusable OSWorld image stable while preserving benchmark provenance.

5. Move the pilot to the template contract.

   Change the pilot from creating an ad-hoc workspace pool to referencing the
   durable template and creating only a claim. Preserve its one-VM invariant,
   authenticated guest bridges, cleanup behavior, and no-secret-in-state rule.

6. Validate in gates.

   First validate schema rendering and service names locally. Then run a
   credential-free provisioning dry run. Next run one disposable Fleet VM and
   verify readiness, Cua Driver, browser control, and cleanup. Only after that
   run one permitted OSWorld 2 task and record the release and driver evidence.

## Risks to resolve before implementation

- Confirm which OSWorld 2 service ports must be reachable through Fleet rather
  than through the authenticated bridge.
- Confirm whether the official V2 image can be repackaged as a container disk
  without changing benchmark behavior.
- Keep gated task classes, assets, model keys, and GitLab credentials runtime
  injected and outside template, image, logs, and source control.

## Smallest first slice

Add the template manifest and a renderer test that proves the immutable image,
resources, readiness probe, and approved service list. Then adapt the existing
pilot to reference that template without changing its evaluation logic.

## Review note

An independent CloudCode review was requested. Its service was unavailable due
to repeated overload responses, so this plan is based on the local pilot and
the official OSWorld 2 release contract; obtain that review before finalizing
implementation.
