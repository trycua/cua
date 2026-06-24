# Evaluating Gemini 3.5 Flash on Computer-Use with Cua-Bench

_Published on June 24, 2026 by Francesco Bonacci and Dillon DuPont_

<img width="940" height="527" alt="hero" src="https://github.com/user-attachments/assets/8f381915-b176-42bd-924c-0471e068e248" />

**TL;DR:**
- Google gave us **early access** to the Computer Use tool powered by **Gemini 3.5 Flash** through the Gemini API.
- We tested `gemini-3.5-flash` with the **native Computer Use API** on [Cua-Bench](https://cua.ai/cuabench)'s **KiCad EDA suite**: 25 real electrical-engineering tasks in KiCad, at a matched **200-step budget**.
- **Gemini 3.5 Flash posted the highest mean reward of any frontier model we tested (0.267)**, solving **5 of 25** tasks fully with 3 more partially.
- GPT-5.5 solved the most tasks outright (**6 of 25**) but earned no partial credit, so Gemini 3.5 Flash edged it on mean reward, _at Flash speed and cost_.
- The strongest signals were **pixel-accurate grounding** on zoomed-in targets and **real analog-design reasoning**.
- The main losses were **design-from-scratch** tasks that timed out inside the 200-step budget, plus a recurring bug: _after the second screenshot the model sometimes hallucinated screen state_.

---

Google gave Cua early access to the Computer Use tool powered by **Gemini 3.5 Flash** through the Gemini API. The tool supports Desktop and Mobile environments. During the EAP the tool calls are free, with standard model pricing still applying.

We wanted to measure it for the same reason we built [Cua-Bench](https://cua.ai/cuabench): computer-use demos are easy to overfit, and desktop work hides failure until the task gets precise. A model can look fluent clicking around a browser, then _fall apart when the target is a tiny CAD control, a zoomed-in schematic node, or a field it has to derive from the design rather than copy from the prompt_.

Cua-Bench is our computer-use benchmark, published with Snorkel, live at [cua.ai/cuabench](https://cua.ai/cuabench). It tests agents against real GUI work rather than screenshots or short web flows. For this run we used it to answer a narrower question: **how much does Gemini 3.5 Flash's native Computer Use API change what a small, fast model can do on desktop tasks?**

## How we tested

We evaluated `gemini-3.5-flash` with the native Computer Use API on Cua-Bench's **KiCad EDA suite**: 25 tasks inside a real electrical-engineering CAD environment. The suite is _hard on purpose_. It asks the agent to read schematic state, place and edit components, manipulate small targets, and sometimes reason about the circuit before it touches the GUI. (Cua-Bench has other suites too, including the public cua-basic-4k desktop set; here we focus on the EDA tasks.)

We ran a **frontier-model head-to-head at a matched 200-step budget**: Gemini 3.5 Flash against GPT-5.5, Claude Sonnet 4.5, Claude Haiku 4.5, Claude Opus 4.8, and Gemini 3.1 Pro and 3 Flash as generational baselines. The 3.1 Pro and 3 Flash runs used the function-calling agent, since native Computer Use is not yet enabled for those preview models.

Gemini 3.5 Flash was **not wrapped as a generic screenshot model**. We used the native Computer Use API, which evaluates the model in the shape Google intends: observe the environment, decide, call the tool, observe again, and continue until the task is done or the budget runs out.

<img width="2400" height="1350" alt="methodology" src="https://github.com/user-attachments/assets/d6fda36d-4c35-4c93-8f08-4d6783ecf0cb" />

## Results

On the Cua-Bench KiCad EDA suite, **Gemini 3.5 Flash posted the highest mean reward of any frontier model we tested: 0.267**, with 5 of 25 tasks solved fully and 3 more partially. GPT-5.5 solved the most tasks outright at 6 of 25, but earned no partial credit, so Gemini 3.5 Flash came out ahead on mean reward. _It did that at Flash latency and cost._

<img width="2400" height="1350" alt="mean-reward-bars" src="https://github.com/user-attachments/assets/4fcae296-ec16-4a8f-a13e-18713b2c8062" />

| Model | Full solves | Partial | Mean /25 |
| --- | ---: | ---: | ---: |
| GPT-5.5 | 6 / 25 | 0 / 25 | 0.240 |
| **Gemini 3.5 Flash** | **5 / 25** | **3 / 25** | **0.267** |
| Claude Sonnet 4.5 | 5 / 25 | 3 / 25 | 0.253 |
| Claude Haiku 4.5 | 5 / 25 | 3 / 25 | 0.253 |
| Gemini 3.1 Pro | 5 / 25 | 3 / 25 | 0.253 |
| Claude Opus 4.8 | 4 / 25 | 4 / 25 | 0.240 |
| Gemini 3 Flash | 1 / 25 | 7 / 25 | 0.200 |

_Gemini 3.1 Pro and Gemini 3 Flash were run via the function-calling agent, since native Computer Use is not enabled for those preview models, so they are not perfectly apples-to-apples with the native Computer Use entries._

No model solves the KiCad suite outright yet. The interesting gaps show up in the parts of the task that usually expose weak computer-use models: tiny targets, zoomed-in visual grounding, and actions where the model has to understand _why_ it is changing a circuit instead of following a UI recipe.

The first strong signal was **grounding**. On zoomed-in CAD targets, Gemini 3.5 Flash landed on the intended object instead of the nearby label, wire, or empty canvas. That is the difference between a model that can only use a desktop when controls are big and one that can work in tools built around precision.

<img width="2400" height="1350" alt="grounding" src="https://github.com/user-attachments/assets/fab9f0b8-aada-4268-9a0a-e4043d501ff6" />

The second strong signal was **reasoning**. In one KiCad task, the model derived a resistor value from a feedback equation, snapped that value to the E96 series, and checked a datasheet to confirm it. We care about this because it crosses the boundary between GUI operation and engineering work. The GUI is still the interface, but the task is not only "click the thing." _The model has to know what value to enter before it can use the tool correctly._

<img width="2400" height="1350" alt="signals-grounding-reasoning" src="https://github.com/user-attachments/assets/a82240d1-efec-45fb-a6d4-5c748cc8c110" />

## Where it struggles

The main losses were **design-from-scratch tasks that timed out inside the 200-step budget**.

That failure mode differs from a bad click or a missed button. On the harder KiCad tasks, the model often made reasonable local progress, but the full task needed too many observe-act cycles: create the design, place parts, wire them, edit values, check the result, and recover from small mistakes. Within 200 steps, some runs ran out of room.

We also saw a recurring limitation. **After the second screenshot, the model sometimes hallucinated screen state.** The benign symptom was `require_confirmation` prompts on forms it had hallucinated. The model acted as if a form or state existed when the current screen did not show it.

The prompts were benign here, so it did no damage in this setup. But it is the kind of bug that matters for computer-use: _the model has to stay anchored to the current observation_. Once it carries forward imagined UI state, the agent loop gets less reliable even when the individual clicks are accurate.

## What we've been using it for

https://github.com/user-attachments/assets/5e606799-6fe5-49d0-b252-51b113887ffa

Gemini 3.5 Flash driving a KiCad task via the Antigravity CLI on top of Cua Driver.

## Try it

You can run Gemini 3.5 Flash through Cua with [Cua Driver](https://cua.ai/docs/cua-driver) and the Cua Agent framework. The EAP setup depends on your Gemini API access and the Computer Use tool configuration.

### 1. Install Cua Driver

**macOS / Linux (pre-release backend)**

```bash
/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/trycua/cua/main/libs/cua-driver/scripts/install.sh)"
```

**Windows (PowerShell)**

```powershell
irm https://raw.githubusercontent.com/trycua/cua/main/libs/cua-driver/scripts/install.ps1 | iex
```

Confirm it resolved:

```bash
cua-driver --version
```

### 2. Start the driver daemon

**macOS**

```bash
open -n -g -a CuaDriver --args serve
```

**Linux (pre-release)**

```bash
cua-driver serve &
```

**Windows**

```powershell
cua-driver autostart enable
cua-driver autostart kick
```

Check it is up:

```bash
cua-driver status
```

On first run, grant the OS permissions the driver needs (macOS: Accessibility + Screen Recording via `cua-driver check_permissions`; Windows: run from an interactive session, see `cua-driver doctor`).

### 3. Drive your desktop with Gemini 3.5 Flash

Cua Driver speaks **MCP over stdio**, so any MCP client (the Antigravity CLI, Claude Code, Cursor, Codex) can call its tools to act on your real desktop in the background. The demo above uses the **Antigravity CLI** (the `agy` binary, the successor to Gemini CLI). Generate its MCP snippet:

```bash
cua-driver mcp-config --client antigravity
```

`agy` has no `agy mcp add` subcommand, so paste the printed JSON into `~/.gemini/config/mcp_config.json`, merging it under the top-level `mcpServers` object (on Windows, `%USERPROFILE%\.gemini\config\mcp_config.json`). Restart `agy` after saving - the same file is shared with the Antigravity IDE.

Then point the client at `gemini-3.5-flash` with the native Computer Use API and give it a task. The driver exposes the screen and an action layer; the model observes, decides, and calls the tools until the task is done. _The clip above is Gemini 3.5 Flash driving KiCad through exactly this path - the Antigravity CLI on top of Cua Driver._

You can also run Gemini 3.5 Flash with the [Cua Agent](https://github.com/trycua/cua) SDK (`pip install cua-agent`) against a Cua cloud sandbox; the SDK's Gemini Computer Use loop drives the native API directly.

If you try this against desktop apps, CAD tools, or any workflow where the model has to reason before it clicks, **send us the trajectory**. The most useful reports include the model, step budget, app, task, and where the observation stopped matching the screen.

Repo: [github.com/trycua/cua](https://github.com/trycua/cua)

Docs: [cua.ai/docs/cua-driver](https://cua.ai/docs/cua-driver)
