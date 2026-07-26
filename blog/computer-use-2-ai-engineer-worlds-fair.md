# Computer-Use 2.0

_Published on July 13, 2026 by Francesco Bonacci, Dillon DuPont, and Robert Wendt_

<img width="900" height="1200" alt="The Cua team outside AI Engineer World's Fair in San Francisco" src="https://pbs.twimg.com/media/HMFQNNuaUAApRcV.jpg" />

Earlier in May, [@swyx](https://x.com/swyx) invited Cua to speak at [AI Engineer World's Fair](https://www.ai.engineer/worldsfair/2026). We used the slot as an opportunity to demo some of the work we had just shipped and explain where we think computer-use is headed in 2026.

[Computer-Use 2.0](https://www.ai.engineer/worldsfair/schedule?track=Computer+Use) was the name of the session. It is also how we have been referring internally to a shift in the way people use Cua. Computer-use is becoming a tool inside coding agents like Claude Code and Codex, and general agents like Hermes, OpenClaw, and Pi.

That shift changes what a desktop session looks like. Several agents can work in separate windows on the same machine, each with its own session, synthetic cursor, and recording.

The talk followed that shift through three connected layers: Cua Driver gives agents a consistent way to operate desktop windows, enabling multi-player scenarios; Cua-Bench evaluates how well agents complete desktop tasks; and Cua Fleets provides the infrastructure to provision, manage, and scale computers for agent workloads.

<img alt="The Computer-Use 2.0 talk cover, introducing Cua Driver, Cua-Bench, and Cua Fleets" src="https://github.com/user-attachments/assets/1c6bac7a-69e7-4fc9-8673-cdda8de66f17" />

_The session followed Cua Driver, Cua-Bench, and Cua Fleets from interaction to evaluation to scale._

To recap where we are coming from, the team behind Cua has worked in this field since 2024, when Dillon and I were building [Windows Agent Arena](https://github.com/microsoft/WindowsAgentArena) at Microsoft. Before computer-use became the common term, we called these systems GUI agents. The name fit the architecture: the model took a screenshot, reasoned over the pixels, chose an action such as click, scroll, type, or wait, and repeated. [OSWorld](https://os-world.github.io/) and Windows Agent Arena made that loop measurable through reproducible desktop tasks.

These loops ran in the foreground and took over the system pointer and keyboard. Running one on your laptop meant waiting for it to finish. The practical answer was to move agents into virtualized desktops: full VMs for Windows or macOS, and GUI containers for Linux. For roughly the next two years, the common architecture remained one agent running a foreground loop inside one isolated desktop.

That architecture was useful. Agents could use existing software without a custom API for every app. But the screen contained nearly everything the agent knew about the task, and mouse and keyboard actions were nearly everything it could do. The GUI was the whole loop.

<img alt="The Computer-Use 1.0 loop: observe a screenshot, reason, and click a target" src="https://github.com/user-attachments/assets/75c46a85-a740-4559-92fa-bd683bef59e7" />

_Computer-Use 1.0: look at the screen, choose one action, and repeat in the foreground._

In many of those systems, the model was also the operator: provider APIs accepted a prompt and screenshot, then returned the next computer action. Computer-Use 2.0 reverses that relationship by letting the main agent own the task and call the desktop only when it needs it.

<img alt="A GUI-first operator receiving a screenshot and returning a click action" src="https://github.com/user-attachments/assets/934cd65a-d285-41bd-a1ee-797fe6b223b5" />

_Computer-Use 1.0 wired the model directly to a screenshot-and-action loop._

## Where the screenshot loop gets stuck

Suppose an agent changes a setting and nothing happens. The app may have rejected the value, written an error to a log, saved a malformed config file, or crashed a process in the background. A screen-only agent gets another screenshot. It can look for a visible clue and try another click.

An agent with computer-use as a tool has more options. It can inspect the file the app wrote, read the logs, restart the process, fix the code, and return to the app to check the result. It can still use the GUI whenever the GUI is the best interface. It no longer has to solve every part of the task through it.

For us, Computer-Use 2.0 means exposing the desktop through MCP or a CLI to the agent people are already running. That agent can move between the screen and the rest of the computer as the task requires.

[Cua Computer Server](https://cua.ai/docs/cua/guide/advanced/local-computer-server) remains our implementation of the Computer-Use 1.0 loop. We still maintain it, but our development focus has shifted entirely to Cua Driver. We will add a small compatibility layer so existing Computer-Use 1.0 clients can keep using the same interface with Cua Driver underneath.

## From foreground to background

Whole-desktop capture also assumes exclusive use of the machine. That is manageable in a disposable sandbox. It gets awkward on a developer's laptop, and it breaks down when several agent runs share a host.

The one-agent, one-foreground-desktop pattern held until April 2026, when [Codex introduced background computer use](https://openai.com/index/codex-for-almost-everything/). Alongside work from the [Sky team](https://openai.com/index/openai-acquires-software-applications-incorporated/), it showed that an agent could operate a window in the background while the person at the machine kept using the same desktop. We had explored the same idea before and put it aside, but their release showed us that we had been close. We were already building a CLI and skill for coding agents, so we spent the next few days adding background execution. On April 22, six days after the Codex announcement, we released Cua Driver as open source.

[Cua Driver](https://cua.ai/docs/tutorials/drive-your-first-app) is the first pluggable computer-use driver built around windows instead of the whole desktop. It gives agents the same window-level view and commands on macOS, Windows, and Linux while handling the platform-specific details underneath. We deliberately keep the CLI lean and leave the interaction policy with the coding model. The agent lists the current windows and asks for one window's state, which returns its accessibility tree and screenshot together.

Apps accept and refuse input in different ways, so action selection works as a ladder. When the accessibility tree represents the target well, the agent first tries an accessibility action. If the tree is sparse or the target is a canvas, it tries a coordinate-level pixel click. Only after those routes fail does it raise the window for the few milliseconds needed to land the click, then restore the prior focus. After each action, the driver reports whether the effect was confirmed, unverifiable, or likely a no-op so the agent knows whether to escalate.

On macOS, background clicks become difficult when an app exposes little accessibility state and the agent has to use coordinates. At a high level, Cua Driver uses Apple's undocumented SkyLight framework to make the target window behave as if it were foreground without raising it, then posts the event to that process. Chromium also needs an off-screen primer click before the actual click. We wrote up the [full implementation](https://cua.ai/blog/inside-macos-window-internals) separately.

<img alt="The macOS background input path through SkyLight, targeted events, and accessibility state" src="https://github.com/user-attachments/assets/80e32316-5b5a-44e1-bea7-0bcb775bea58" />

_Background input on macOS combines window activation, process-targeted events, a primer click for Chromium, and live accessibility state._

Since April, developing in the open has pushed the driver into Linux distributions and desktop stacks we would never have tested first ourselves. Users found the edge cases, and those reports shaped the Windows and Linux backends. With version 0.8, both are stable on the same Rust interface as macOS.

<img alt="Windows and Linux input backends behind a shared Cua Driver interface" src="https://github.com/user-attachments/assets/7af0f857-e727-470c-ad6c-d16a0bd5e85b" />

_Windows and Linux expose different input systems, while Cua Driver keeps the agent-facing commands consistent._

<img alt="Cua Driver returns window pixels and accessibility state, with accessibility, pixel, and foreground action paths" src="https://github.com/user-attachments/assets/55443a38-c6bb-4433-89e8-8b0f7ded8f9c" />

_Cua Driver reads window state once, then lets the agent choose the action path that fits the app._

Version 0.8 makes synthetic multi-cursor work the same way across macOS, Windows, and Linux. Each agent gets a session id tied to its cursor, actions, and recording while input stays within its assigned windows. Give agents non-overlapping window sets and they can work concurrently without fighting over the physical pointer or focus.

The colored cursors are visual records of those sessions and do not deliver input themselves. When something fails, the session id tells us whose cursor, recording, and actions to inspect.

Shipping that interface raises the next question: how do we keep a new release from breaking an action that worked before? Even on the same operating system, input can follow a different path depending on the window and application toolkit. We built repo-local test apps whose only job is to prove that an AX or pixel action reached its target. A successful Driver response is not enough. The harness must independently observe the state change, or prove that an unsupported route refused the action without stealing focus, changing the window order, or moving the physical cursor.

[The current test matrix](https://github.com/trycua/cua/blob/main/libs/cua-driver/docs/test-matrix.md) scores more than 500 behavioral checks across Windows, macOS, Linux X11, Sway, and GNOME. It covers Electron, Tauri, native application toolkits, and embedded web views, with the trajectory, video, and logs kept for each run. [H Company](https://hcompany.ai/) has been a partner in this work and helped us audit the matrix for gaps.

<img alt="A Cua Driver regression matrix testing accessibility, background pixel, and foreground actions" src="https://github.com/user-attachments/assets/4198a963-a8a7-4879-8e6c-e0cfb6d6dbc9" />

_The regression matrix checks whether each action changed the target app and whether a background window stayed in the background._

[Clicky](https://clicky-ai.com/) was the first product to adopt Cua Driver in April. [Hermes](https://hermes-agent.nousresearch.com/), [Qwen Code](https://github.com/QwenLM/qwen-code), [H Company](https://hcompany.ai/), and [Factory's Droid](https://factory.ai/product/droids) followed. Several of those teams have also brought their findings and fixes back to the [upstream repository](https://github.com/trycua/cua/tree/main/libs/cua-driver). Thank you for adopting a new interface early and sending back what broke.

<img alt="Cua Driver adopters including Clicky, Hermes, Qwen Code, H Company, and Factory" src="https://github.com/user-attachments/assets/23736162-bc74-4640-a4b5-d5844411ca93" />


_Teams use Cua Driver through MCP, the CLI, and the SDK, and several have contributed fixes upstream._

That was where I handed the talk to Dillon.

## Measure what the agent left behind

Cua Driver gives an agent hands. The harder question is whether it finished the job correctly and left nothing broken behind. We built Cua-Bench to measure that.

A moving cursor is useful when you want to watch an agent. It is a bad way to score one. An agent can look busy while going in circles, or appear quiet while it changes a file through another tool. The result lives in the state of the computer and the artifact the agent produced.

[Cua-Bench](https://cua.ai/cuabench) defines every task in code as three pieces: setup puts the machine in a known state, the agent attempts the task, and an evaluator checks the result. The evaluator can inspect the files and application state directly, which makes the score deterministic. When a task allows it, the evaluator can also give partial credit for correct work that stopped short of completion.

<img alt="A Cua-Bench task split into setup, agent execution, and evaluation" src="https://github.com/user-attachments/assets/bbb6ea7b-4316-42ec-b4f9-29af5631338d" />

_A Cua-Bench task separates setup, agent execution, and evaluation._

Building a benchmark environment usually takes desktop expertise and infrastructure. With [cua-bench-ui](https://github.com/trycua/cua/tree/main/libs/python/bench-ui), a small environment can live in one Python file. The file launches its interface as a desktop window on macOS, Windows, or Linux and exposes the application state through an embedded JavaScript bridge. Cua Driver can operate the window while the evaluator reads that state directly. A person or a coding agent can author the file.

<img alt="A one-file Cua-Bench UI environment with its window and inspectable state" src="https://github.com/user-attachments/assets/828225c1-d1ee-4d16-b24c-074e783e5289" />

_A Cua-Bench UI environment can define its window and inspectable state in one Python file._

At the time of the talk, the [Cua-Bench catalog](https://cua.ai/cuabench) contained 130 verifiable tasks across 42 environments and five platforms. The task code and evaluators are open, so anyone can inspect how a result was graded and reproduce the run through the `cb` CLI.

The newest set was [cua-bench-kicad](https://github.com/trycua/cua/tree/main/libs/cua-bench/datasets/cua-bench-kicad), 25 electronics tasks built with [Snorkel AI](https://snorkel.ai/). KiCad is an engineering tool used to design schematics and circuit boards. Each evaluator grades the saved work by exporting its circuit netlist and comparing the components and connections against a reference. The full set runs with one command:

```bash
cb run dataset cua-bench-kicad
```

Our recent [KiCad evaluation](https://cua.ai/blog/evaluating-gemini-3.5-flash-on-computer-use) showed how hard this still is. We ran seven frontier models on 25 electrical-engineering tasks with the same 200-step budget. The results were humbling. GPT-5.5 completed the most tasks, 6 of 25. Gemini 3.5 Flash had the highest mean reward, with 5 full solves and 3 partials. No agent reached a third of the full-completion ceiling; the leaderboard was flat across all seven.

Every full pass was an edit to a schematic or board that already had something on the canvas. The blank-canvas tasks scored zero across all seven models. Models often made reasonable local changes, then ran out of steps before the artifact was complete. That gap is easy to miss in a demo built around one successful run.

<img alt="Six of 25 KiCad tasks completed, with no model building a schematic from a blank canvas" src="https://github.com/user-attachments/assets/4918f027-2fd8-4442-8dbb-a7dec56a9a81" />

_Across 25 expert-authored KiCad tasks, the best model completed six. All six began from an existing design; blank-canvas tasks scored zero._

<img alt="The Cua-Bench KiCad leaderboard showing the leading models completing five or six of 25 tasks" src="https://github.com/user-attachments/assets/20840ded-1374-44d1-9f97-14b39da09270" />

_The leading agents clustered between five and six full passes, far below the 25-task ceiling._

That leaves another question: can we trust the evaluator itself? Before an environment enters a dataset, our QA pipeline runs each sampled task twice. One agent attempts the task normally; another tries to earn the reward while deliberately violating it. The scorer must give the correct run full credit and penalize the adversarial one.

A reviewer then reads both trajectories and posts a CodeRabbit-style PR comment with the evidence and a proposed fix for each failure. We use that report to catch evaluators that reward the wrong outcome before we publish the environment. It is evals all the way down.

<img alt="An evaluator QA review that identifies a reward-integrity failure and proposes a fix" src="https://github.com/user-attachments/assets/7be83e16-5705-46c2-bb4d-f9e7b488febd" />

_The QA review traces an evaluator failure back to the trajectory and proposes a fix before the task is published._

Dillon closed his section with one more use for recorded rollouts. At any recorded step, we can reconstruct the action and file state we captured, ask a model to predict the next observation, and score it against what happened. The first version works from that captured state and gives us a narrow test of whether the model understands the environment.

<img alt="A recorded trajectory forked at one step so a model can predict the next state" src="https://github.com/user-attachments/assets/dcd0efd3-126b-4b79-8784-5b86ddd70340" />

_A recorded trajectory can be reconstructed at one step, then used to score a model's prediction of the next state._

## Keep the fleet ready

<img alt="The transition from measuring agents to scaling evaluation and reinforcement-learning throughput" src="https://github.com/user-attachments/assets/0b2d24a1-3116-4d3f-a5f6-c3dbd4113f5d" />

_Once agent behavior can be measured, sandbox availability becomes part of evaluation throughput._

Robert closed the talk with the systems problem behind long training runs. Every rollout needs an isolated computer in a known starting state. We call that computer a sandbox. If a worker requests one only when the rollout begins, it can wait while the system downloads the machine image and boots the VM. Any trainer GPU reserved for that worker waits too.

Cold starts can only shrink so far. They also get harder as machine images grow or a run spans several operating systems and app configurations. For smaller sandboxes, keeping a few clean environments ready can cost less than leaving a GPU idle while another one starts.

<img alt="Why warm sandbox pools can be cheaper than waiting for every environment to start" src="https://github.com/user-attachments/assets/02446277-5860-40f4-a19b-3c89a57649f9" />

_Some runtimes have a startup floor, making a small warm pool cheaper than leaving a trainer GPU idle._

<img alt="Worker utilization timeline showing generation interrupted by idle waiting periods" src="https://github.com/user-attachments/assets/654104b3-8b6d-493a-a4f0-b0b57443fd77" />

_Each idle gap is time the GPU could have spent generating the next action._

[Cua Fleets](https://run.cua.ai) moves startup work into a warm pool, a small group of sandboxes that are already booted and waiting. When a rollout starts, it asks the pool for a sandbox. The pool reserves one and returns its connection details. Internally, we call this reservation a claim. Since the VM has already booted, the rollout only waits for that reservation.

<img alt="A warm pool tracks sandbox demand so rollout workers spend less time waiting for cold starts" src="https://github.com/user-attachments/assets/8fddd996-7561-4983-9daf-2c1457321588" />

_Warm sandboxes absorb startup time before a rollout asks for one._

The pool adjusts throughout the training run. Waiting requests make it grow, active reservations hold capacity, and released reservations let it shrink. You set a minimum number of spare sandboxes and a maximum to cap the cost. The pool also waits before scaling down, which prevents a short dip in demand from causing another round of shutdowns and cold starts.

<img alt="Worker utilization after warm pools remove idle gaps between rollout generations" src="https://github.com/user-attachments/assets/4f5a0e86-6418-4512-b608-b4438a0c4f24" />

_With ready capacity, rollout workers spend more of the training run generating instead of waiting for a cold start._

The same reservation works for longer jobs. A run can keep its sandbox for as long as it needs the machine. When the run releases it, the pool restarts the sandbox into a clean state and makes it available again.

<img alt="Cua Fleets platform support across Linux, Windows, Android, and macOS" src="https://github.com/user-attachments/assets/36df63bd-5162-4513-85d6-88bd17b0314d" />

_Cua Fleets spans Linux, Windows, and Android, with macOS in Labs preview on Cua's virtualization stack._

If your startup needs to run GUI sandboxes at scale for evaluations or RL, [join the Cua Fleets waitlist](https://cua.ai/signup?redirect_url=%2Fwaitlist).

The code and docs behind the talk are here:

- [Install Cua Driver](https://cua.ai/docs/how-to-guides/driver/install) or read the [Driver source](https://github.com/trycua/cua/tree/main/libs/cua-driver).
- Browse [Cua-Bench](https://cua.ai/cuabench) and its [task framework](https://github.com/trycua/cua/tree/main/libs/cua-bench).
- Join the [Cua Fleets waitlist](https://cua.ai/signup?redirect_url=%2Fwaitlist).
- Follow the main repository at [github.com/trycua/cua](https://github.com/trycua/cua).

[Browse the Computer-Use 2.0 slides from the talk.](https://github.com/user-attachments/files/30055625/computer-use-2-ai-engineer-worlds-fair-slides.pdf)
