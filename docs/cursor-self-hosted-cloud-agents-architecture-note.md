# Cursor self-hosted cloud agents: architecture and implications for Cua

**Status:** Product and architecture note

**Date:** 2026-09-02

**Source date:** Cursor announcement observed 2026-09-02; Cursor launch post published 2026-03-25

## Executive summary

Cursor now lets customers run cloud-agent workers on infrastructure they control while Cursor retains the agent experience, orchestration, and model access. The customer-hosted workers can reach private code, caches, dependencies, build systems, and internal network endpoints without moving those assets into Cursor-managed compute.

The important development is not simply "self-hosted agents." It is the explicit separation of the agent control plane from the execution plane:

```text
Cursor control plane                         Customer execution plane

task intake and UX                          repositories and worktrees
agent loop and orchestration   <----------  outbound worker connection
model access                                tools, builds, and test suites
run status and coordination                 caches and internal services
                                             specialized CPU/GPU hardware
                                             worker pools and autoscaling
```

This validates a deployment model directly adjacent to Cua's strengths. Cua already supplies portable computer-use runtimes, full desktop sandboxes, reusable pools, and cross-platform control. The opportunity is to make Cua the execution substrate that any agent control plane can target, including coding agents that need GUI applications, non-Linux operating systems, specialized hardware, or infrastructure inside a customer's trust boundary.

## What Cursor announced

Cursor's launch post makes the following claims:

- Self-hosted cloud agents are generally available.
- The customer's codebase, tool execution, and build artifacts remain in the customer's environment.
- Workers can access existing caches, dependencies, and private network endpoints.
- Cursor continues to provide orchestration, model access, and the user experience.
- A lightweight worker connects outbound to Cursor, receives jobs, creates an isolated development environment, and runs the Cursor agent.
- Customers can run workers on their own machines or through integrations including AWS Lambda, Coder, Cloudflare, Daytona, E2B, Modal, Namespace, and Vercel.
- A fleet-management API exposes utilization information and supports customer-built autoscaling.

The announcement describes customer-controlled execution, not a fully self-hosted Cursor stack. Cursor still owns the agent loop and its control-plane services.

## Why this matters

### Enterprise agent adoption is becoming an infrastructure problem

The initial cloud-agent product assumed that a vendor-provided Linux environment was sufficient. This launch recognizes that serious software work depends on resources that are difficult or undesirable to copy into a vendor sandbox:

- private package registries and internal APIs;
- large build caches and monorepo dependencies;
- licensed tools and company-specific developer images;
- regulated source code, credentials, and build artifacts;
- unusual compute, attached devices, or accelerated hardware; and
- operating systems and desktop applications that cannot run in a generic Linux container.

For these customers, the execution environment is part of the product rather than an interchangeable implementation detail.

### The control-plane/execution-plane boundary is becoming a product surface

Cursor's worker protocol is now a strategic boundary. It must cover worker identity, job assignment, environment creation, secrets, logs, artifacts, cancellation, heartbeats, retries, capacity, and teardown. The fleet API adds scheduling and autoscaling concerns on top.

Once this boundary exists, customers will reasonably ask whether they can:

- use a different sandbox or fleet provider;
- place workers in several clouds, regions, or on-premises clusters;
- select workers by OS, architecture, GPU, network, or compliance attributes;
- retain their own telemetry and policy enforcement;
- bring their own model or agent loop; and
- use the execution plane independently of Cursor.

Cursor's current product answers the first question through a provider ecosystem while keeping the agent loop proprietary. Cua can differentiate by treating both sides of the boundary as composable.

## Relationship to Cua

Cua's existing layers map naturally onto this architecture:

| Need | Relevant Cua capability |
| --- | --- |
| Isolated task environment | Cua Sandbox images and lifecycle |
| Warm, reusable capacity | Sandbox pools and durable claims |
| Linux container execution | Container-backed sandboxes |
| Full OS fidelity | macOS, Windows, Linux, and Android VM backends |
| GUI and desktop application access | Cua Driver screenshots, accessibility, and input |
| Code and GUI in one environment | Shared shell, PTY, filesystem, processes, and desktop state |
| Customer-controlled infrastructure | Local runtimes, self-hostable components, and Fleet deployment primitives |
| Agent portability | SDK, MCP, and runtime-neutral action contracts |

The strongest Cua position is broader than "another place to run Cursor workers." Cua can provide a common execution plane for agents that need computers, with infrastructure and agent-loop choice kept independent.

```text
                  Agent control planes
        Cursor     Codex     Claude     custom agents
             \       |        |       /
              portable execution contract
                         |
                 Cua Sandbox + Fleet
                         |
       Linux containers / macOS / Windows / Android
       cloud / on-premises / developer-owned machines
```

## Product implications

### 1. Define the remote worker contract explicitly

Cua should have a crisp, documented contract for attaching an external agent or scheduler to customer-managed capacity. It should specify:

- worker registration and workload identity;
- capability advertisement such as OS, architecture, GPU, region, and image;
- claim, lease, heartbeat, cancellation, and retry semantics;
- ingress-free or outbound-only connectivity;
- secret delivery and redaction boundaries;
- artifact and trajectory ownership;
- policy enforcement and audit events; and
- cleanup guarantees after success, failure, cancellation, or lost connectivity.

The contract should work without requiring Cua to own the agent loop.

### 2. Present pools as an agent-platform primitive

Warm pools are not only a sandbox optimization. They are the capacity layer behind interactive and asynchronous agents. The product story should show how an agent platform asks for a computer with declared capabilities, receives a claim, reconnects when appropriate, and releases it safely.

### 3. Make heterogeneous scheduling visible

Cursor's announcement invites questions about GPUs and unusual environments. Cua can make this first-class by advertising and selecting capacity by:

- operating system and version;
- CPU architecture;
- GPU model and acceleration API;
- memory, storage, and attached devices;
- network placement and allowed destinations;
- image or snapshot lineage; and
- interactive desktop availability.

### 4. Emphasize full-computer workloads

Most named providers in Cursor's announcement are associated with Linux code sandboxes. Cua should clearly demonstrate the workloads that require more than a shell: native desktop builds, browser-plus-terminal workflows, Windows applications, macOS/Xcode automation, Android, GPU-backed graphical applications, and end-to-end tests across real OS interfaces.

### 5. Keep deployment ownership unambiguous

"Self-hosted" can hide several different ownership models. Cua documentation and APIs should name them precisely:

| Layer | Possible owner |
| --- | --- |
| Agent loop and model calls | Cua, another vendor, or the customer |
| Scheduling and control plane | Cua Cloud or customer-operated |
| Worker and sandbox runtime | Cua Cloud, customer cloud, on-premises, or developer machine |
| Source, secrets, tools, and artifacts | Customer-selected boundary |
| Telemetry and trajectories | Customer-selected storage and retention |

A deployment diagram should always show which network receives source code, prompts, screenshots, tool results, artifacts, and model traffic. "Runs in your infrastructure" alone does not answer those questions.

## Proposed follow-up

1. Write a public concept page describing control planes, execution planes, and the ownership options supported by Cua without framing it around a competitor.
2. Publish one reference integration that claims a Cua sandbox or pool from an external agent service through an outbound-only worker.
3. Define a capability schema for OS, architecture, GPU, region, network placement, and desktop availability.
4. Verify and document which Fleet components can be operated in customer infrastructure today, separating current support from intended architecture.
5. Build a representative demonstration in which a remote coding agent runs a task that needs both a terminal and a native desktop application on customer-managed macOS or Windows capacity.

## Confirmed constraints and remaining questions

Cursor's current documentation adds several details beyond the launch announcement:

- Team Pools require an Enterprise plan and authenticate with service-account API keys.
- Workers send Cursor the content needed by the agent, including file contents, terminal output, diffs, screenshots, local MCP results, and routing metadata.
- Cloud Agent artifacts can be uploaded to Cursor-managed storage unless the customer blocks the documented artifact endpoint.
- Team Pools can route work to named pools for GPU machines or Macs, and each pool worker serves one agent at a time.
- Cursor provides a worker controller and pending-request APIs, while the customer remains responsible for infrastructure, worker images, secrets, scaling policy, and production validation.
- Cursor documents private connectivity to GitHub Enterprise Server, GitLab Enterprise, and private source-control APIs.

The following integration questions still require validation for a Cua-backed deployment:

- Can Cursor's worker controller claim and release Cua Fleet capacity without a custom reconciliation layer?
- Which worker and claim identifiers should be shared so both systems recover cleanly after interruption?
- How should Cua capabilities such as OS, architecture, GPU, region, and desktop availability map to Cursor Team Pool selection?
- Does a GitLab CE deployment work through Cursor's documented source-control paths, or is the support limited to GitLab Enterprise?
- Can worker images pin and update the Cursor CLI through a supported non-interactive release channel?
- Can customers replace Cursor's model access or agent loop while retaining any part of the worker integration?

## Sources

- Cursor, [Run cloud agents in your own infrastructure](https://cursor.com/blog/self-hosted-cloud-agents), published 2026-03-25.
- Cursor, [Self-hosted cloud agents documentation](https://cursor.com/docs/cloud-agent/self-hosted).
- Cursor, [Cloud Agents](https://cursor.com/agents).
- Cursor announcement on [X](https://x.com/cursor_ai), observed 2026-09-02. The supplied transcript did not include the individual post URL.
- Cua, [How sandboxes work](content/docs/concepts/how-sandboxes-work.mdx).
- Cua, [SDK, MCP, and process hosting](content/docs/concepts/sdk-mcp-and-hosting.mdx).
- Cua, [Run sandboxes in parallel](content/docs/how-to-guides/sandbox/scale-out.mdx).
