# jev-use with Cua Driver and TypeSafe Jev

`jev-use` is a recipe that uses TypeSafe Jev to choose the next browser action
while Cua Driver observes the page, performs that action, and verifies the
result. Equivalent Python and TypeScript programs run the same bounded loop.

The runnable path prefers fresh browser DOM and semantic evidence. It also
contains an optional adapter for the public `cua.visual_regions_v1` contract.
The checked-in typed fixtures exercise that adapter without a model or secret.
At runtime the agents use `parse_visual_regions` only when Driver also
advertises the capture-bound `click.capture_id` contract. They capture the
browser window with `get_window_state`, then keep that exact `capture_id` in the
click arguments. Otherwise they continue through the semantic path.

The TypeSafe-specific code lives in `python/jev_adapter.py` and
`typescript/jev_adapter.ts`, outside Driver. Both adapters send Jev the bounded
candidate IDs and descriptions plus a compact observation. When visual regions
are present, that observation includes their typed bounds and the exact
`capture_id`; it never includes screenshot bytes or extension internals. The
observation also carries a `form` summary that the runner derives from the page
structure and any validated visual regions. The summary reports whether the
verification field is `empty`, holds the required token
(`contains_required_token`), or holds `contains_other_value`. It also reports
the Submit control: `available` for a page-structure button ref. When there is
no ref and the capture-bound visual path is enabled, it reports `visual_only`
for a unique validated visual Submit region, `visual_check_pending` before that
step's visual regions are parsed, or `not_found_visually`. Without a visual path,
it reports `not_in_page_structure`. The token itself is replaced
with `[verification token]` in the page, outline, and visual text. `history` lists
one `{step, selected_id, outcome}` item per earlier decision. Timings and model
probabilities stay in the JSONL log only. A live answer must be one of the supplied IDs before the runner resolves it to the
original immutable action.

You can run the complete deterministic proof without credentials or network
access to Jev. If you have a TypeSafe API key, you can separately verify the
same loop against the live Jev service.

Follow the [setup guide](../../../../docs/content/docs/how-to-guides/driver/jev-use.mdx)
for installation and your first run. The sections below cover the implementation,
standalone runners, and development checks.

## What the example proves

The local fixture contains a small browser task and exposes its submitted value
through an independent `/state` endpoint. Each runner:

1. opens one persistent `cua-driver mcp` connection;
2. creates a Driver-owned isolated Chromium profile with `browser_prepare`;
3. navigates to the loopback fixture with typed browser tools;
4. captures a browser snapshot before asking for or performing an action;
5. constructs immutable candidates plus reserved `reobserve` and `abstain`
   choices;
6. asks the selected provider for one typed choice;
7. uses fresh page references for typing and semantic clicking, or one unique
   validated capture-bound visual Submit region when available; and
8. checks the fixture's `/state` endpoint instead of treating the action
   response or a screenshot as proof of success.

### Candidate sources and task specs

The loop separates where candidates come from from what the task needs. A
candidate source (`python/sources.py`, `typescript/sources.ts`) finds controls
in one observation and builds fully specified candidates for them:
`BrowserSemanticSource` reads the `get_browser_state` snapshot and acts through
page refs, and `VisualRegionSource` reads validated `parse_visual_regions`
regions and acts through a capture-bound `click`. A task spec
(`python/tasks.py`, `typescript/tasks.ts`) holds the goal, the parameters with
the secret token marked for redaction, the step budget, the allowed Driver
action kinds, the `form` state summary, the candidate IDs and descriptions, and
the success oracle. The browser fixture above is `FixtureFormTask`, and its
oracle is the fixture's `/state` endpoint. Browser requests are unchanged: the
golden files `fixtures/jev-provider-request-golden-*-v1.json` pin the exact
provider requests and candidate tables for the page-structure and visual
fixtures, and browser tasks keep sending `cua.jev_choice_request_v1`.

### Native desktop tasks

`NativeAccessibilitySource` ([RFC #4268](../../../../rfcs/4268-jev-use-native-candidates.md))
builds candidates from the native elements of one `get_window_state` call that
returns the tree and the screenshot together, so element tokens and the
`capture_id` describe the same moment. `native_roles.py` / `native_roles.ts`
map raw macOS AX, Windows UIA, and Linux AT-SPI roles to a closed set of role
classes (`button`, `toggle`, `checkbox`, `radio`, `popup`, `menu_item`, `link`,
`text_input`), keyed by the same normalization as Driver's `verify_state`.
Only enabled, on-screen, labeled, native (not `in_web_content`) elements
become candidates; a label equal to the element's value counts as unlabeled.
Candidate IDs come from the role class, label, and actionable-ancestor path,
never `element_index`. At most 24 action candidates are offered, plus
`reobserve` and `abstain`. When more are eligible, the candidates that perform
the task's declared steps are kept first, then controls whose label shares a
word with the goal, then the rest in element order; the kept candidates are
still presented in element order, and the number dropped is logged. Labels that suggest deleting, sending, purchasing,
or closing are excluded unless the task allows that risk. Text comes only from
task parameters. Actions are element-bound `click`, `set_value`, or
`type_text` with background delivery first; a stale token leads to a fresh
observation. Native steps send `cua.jev_choice_request_v2`, which adds a
per-candidate `source` and compact value-free `elements`. A task that declares
its required steps also sends `progress`: each step with how often this run has
performed it, counted only from the runner's own successful actions and never
read from the app. A step's candidate also says which earlier step must be done
first, for example Save waits for the note to be entered.

Visual regions from OmniParser are parsed, from the same `capture_id`, only
when the tree offers nothing to act through: Driver reports `ax_tree_empty`, a
complete tree lacks a task's declared visual target, or a non-truncated tree,
observed again with a larger budget, holds only window roots and window chrome
(`no_application_elements`). The last case covers custom-painted surfaces,
for which Linux X11 returns only window metadata, Windows UIA only the title
bar, and macOS AX only the unlabeled window buttons and the application's
global menu bar (which on macOS are not window content). A partial tree with
any application element never falls back. Visual targets must reach an OCR
confidence of 0.8 unless a task opts into a lower bar; an exact, unique text
match is required either way.
A visual click is background first; after a structured background refusal,
the next step offers a separate `<id>:foreground` candidate when the task
allows foreground delivery.

The same three tasks run on four repository harnesses: `<harness>-counter`
(set the counter to 3), `<harness>-save-note` (set the Note field and save),
and `<harness>-choose-size` (select Large and check I agree). The harness is
`appkit` (macOS AX), `wpf` or `winui3` (Windows UIA), or `gtk3` (Linux
AT-SPI). In task mode, each harness shows the same labeled controls, so
candidate IDs and the mock provider's choices are identical on every platform.
Task mode is selected with `CUA_APPKIT_TASK_STATE`, `CUA_WPF_TASK_STATE`,
`CUA_WINUI3_TASK_STATE`, or `CUA_GTK3_TASK_STATE`. WPF, WinUI3, and GTK3 show
a small dedicated task window.
A fourth task, `canvas-cancel`, clicks the Cancel card on the cross-platform
visual-only canvas fixture (a custom-painted Tk window with no accessibility
tree), so it can only succeed through the visual fallback and needs the
cua-perception extension. It observes only the window's top level
(`max_depth: 1`), so macOS does not spend the walk budget in the menu bar. It
targets Cancel because OmniParser reads the Save label inconsistently on a 1x
macOS capture (`Save`, then `Saye`), and the exact text match then correctly
offers no candidate. Send is a risk phrase. Run it with `verify_native.py --harness canvas`; the
verifier serves the loopback journal the fixture publishes its state to and
writes that state to the task's state file. Each harness rewrites that
state file on every change, and the file is the independent oracle. On
Windows, the title bar's System menu, Minimize, Maximize, and Close buttons
are window chrome and never become candidates. WinUI3 reports the same UIA control
types as WPF, so both share the Windows role table; WinUI3's static `Text`
labels are not candidates. With Cua Driver 0.30.1 or
later, run from this directory:

```bash
# macOS
bash ../../tests/fixtures/build/macos.sh --only appkit
uv run --frozen python verify_native.py --harness appkit --typescript --output-dir /tmp/jev-native-proof
# Linux (X11 session with AT-SPI; the harness runs on the system python3 with PyGObject)
bash ../../tests/fixtures/build/linux.sh --only gtk3
.venv/bin/python verify_native.py --harness gtk3 --typescript --output-dir /tmp/jev-native-proof
```

```powershell
# Windows
..\..\tests\fixtures\build\windows.ps1 -Targets wpf
uv run --frozen python verify_native.py --harness wpf --typescript --output-dir $env:TEMP\jev-native-proof
# or the WinUI3 harness
..\..\tests\fixtures\build\windows.ps1 -Targets winui3
uv run --frozen python verify_native.py --harness winui3 --typescript --output-dir $env:TEMP\jev-native-winui3
```

`--capture-dir` also records sanitized `get_window_state` fixtures like the
ones under `fixtures/native/`. On Linux, Cua Driver 0.30.2 and earlier report
no `value` for a named text field
([#4291](https://github.com/trycua/cua/issues/4291)). With those Drivers,
`gtk3-save-note` cannot observe its own write, so it keeps offering the write
until the step budget runs out.

#### Measure accuracy at larger candidate sets

The task-mode harnesses show only a few controls, so each step offers 4 to 7
candidates. `CUA_APPKIT_TASK_DENSITY`, `CUA_WPF_TASK_DENSITY`,
`CUA_WINUI3_TASK_DENSITY`, or `CUA_GTK3_TASK_DENSITY` set to `12` or `24`,
together with the task-state variable, adds benign distractor controls
(toolbar-style buttons, labeled text fields, checkboxes, and radio groups, some
close to a task control such as "Save draft" or "Note title") before the task
controls, which fills the set to about 12 or to the 24-candidate cap.
`verify_native.py --density 12` or `--density 24` launches the harness that
way and checks that the harness reports the density. Each runner step logs
`candidate_count`, `expected_ids` (the declared steps that are due), and
`expected_offered`, next to the choice, confidence, and `decide_ms`.
`measure_native.py` turns those logs, or an offline replay of captured
fixtures through any provider, into an accuracy table by set size:

```bash
# Score the runner logs of live runs
uv run --frozen python measure_native.py logs /tmp/jev-native-proof --out /tmp/jev-native-accuracy
# Replay recorded window states through Cua-S1, comparing depth-first and relevance capping
uv run --frozen python measure_native.py replay --provider s1 --reps 5 \
  --fixture gtk3:24:fixtures/native/gtk3-window-state-density-24-v1.json \
  --cap-order depth_first --cap-order relevance --order element --order shuffled \
  --out /tmp/jev-native-replay --group-by density,cap_order,order,bucket
```

`fixtures/native/` has density 12 and 24 captures for all four harnesses
(`appkit`, `wpf`, `winui3`, and `gtk3`). Measured live with Cua Driver 0.30.4
on macOS, Windows, and Linux, with both the Python and TypeScript runners,
TypeSafe Jev and Cua-S1 chose correctly in all 1,680 decisions at about 4,
12, and 24 candidates. The per-harness tables are in the
[RFC 4268 decision record](../../../../rfcs/4268-jev-use-native-candidates.md).

#### Choose with Cua-S1

The native runners also accept `--provider s1`, and `verify_native.py` accepts
`--s1`. Loading Cua-S1 takes 10 to 35 seconds, so a per-step model load would
not fit inside Driver's 60-second capture lifetime. The runners therefore send
each validated `cua.jev_choice_request_v2` request to an already loaded model
behind a loopback HTTP endpoint, and read one `cua.decision_choice_v1`
response. `CUA_S1_DECISION_URL` names that endpoint. It must be a plain
`http://` URL on `127.0.0.1`, `localhost`, or `::1`; a model on another host is
reached through an SSH tunnel. The runner rejects a response whose schema,
`capture_id`, kind, selected ID, or probabilities do not match the request, and
it resolves the selected ID to its own candidate as with every other provider.

The service is not part of this example. Any server that loads the pinned
checkpoint from [`libs/cua-s1`](../../../cua-s1/README.md), validates the
request with `python/choose_action.py`, and answers with
`decision_models.choose(S1DecisionModel(...), request).to_wire()` fits the
contract. Run the service from the same revision as the runners: an older
validator rejects a newer optional field such as `progress`, and the runner
then stops with a logged `S1ServiceError` outcome. For example, to verify the native tasks with it:

```bash
export CUA_S1_DECISION_URL=http://127.0.0.1:8791/decide
uv run --frozen python verify_native.py --harness appkit --typescript --s1 --output-dir /tmp/jev-native-s1
```

The MCP connection stays open across the entire loop. This preserves the
explicit named Cua Driver session and avoids rebuilding tool state for every
step.
Page references are snapshot-bound, so the runners take another snapshot after
the page changes rather than reusing an older reference.
Visual candidates are also bound to the exact capture ID and screenshot
reference. Driver receives the original screenshot-space point and capture ID
atomically, so a retired, changed, or mismatched capture is refused before
native dispatch. The agents never retry that refusal as an unbound coordinate
click. Stale, malformed, out-of-bounds, duplicate, or ambiguous visual results
produce no executable visual candidate.

## Install the prerequisites

Install Cua Driver so that `cua-driver` is on `PATH`, or set `CUA_DRIVER_BIN`
to its absolute executable path. You need an unlocked desktop and a signed
Chromium-based browser that Cua Driver can prepare. Python with `uv` supplies
the fixture and Python agent. Node.js 22 or later and npm are needed only for
the optional TypeScript agent.

The agents start `cua-driver mcp` with the MCP SDK's minimal default
environment plus the desktop-session variables (`DISPLAY`, `WAYLAND_DISPLAY`,
`XAUTHORITY`, `XDG_RUNTIME_DIR`, `DBUS_SESSION_BUS_ADDRESS`,
`AT_SPI_BUS_ADDRESS`, `XDG_SESSION_TYPE`, `XDG_CURRENT_DESKTOP`) and any
`CUA_DRIVER_*` variables. Other variables, including `TYPESAFE_API_KEY`, are
not passed to the Driver. On Linux, run the agents from the logged-in desktop
session so those variables are set.

From this directory, install the locked Python dependencies:

```bash
uv sync --frozen --python 3.12
```

For the TypeScript agent, also run:

```bash
export npm_config_cache="$PWD/.venv/npm-cache"
npm ci
```

On Windows, use your logged-in desktop session and `npm.cmd ci` in
PowerShell. An elevated Driver, such as the installer's autostart on an
administrator account or the built-in Administrator account that some cloud
images create as the first user, launches the isolated browser with a derived
standard-user token. The managed verifier starts TypeScript through `node --import tsx`,
not an npm shell shim. On Linux, use a supported system browser and a desktop
session accessible to the same user as Driver; native Wayland has separate
compositor-specific requirements.

Development validation evidence and environment details are recorded in the
[validation report](../../docs/jev-use-validation.md).

## Verify setup in one agent command

The managed verifier starts the existing fixture on an unused loopback port,
runs the existing agent, requires both its verified event and an independent
HTTP state match, and closes the fixture even when a runner fails. This avoids
background-process lifetime and cross-call signal restrictions in agent shells.

```bash
uv run --frozen python verify_setup.py --output-dir proof-mock
uv run --frozen python verify_setup.py --live --output-dir proof-live
```

Use a new output directory for every attempt. `--live` adds live Jev after the
mock check; it reads `TYPESAFE_API_KEY`, prompts securely in an interactive
terminal, or stops before starting if an unattended run lacks a key. After
installing the TypeScript dependencies, add `--typescript` to verify both
languages. `summary.json` must say `complete: true`; partial results remain
false. Each runner has four decisions and a 180-second process timeout. These
are not provider billing caps.

### Prove which path acted

Every `step` event in a runner's JSONL log records the Driver tool that acted
(`browser_type`, `browser_click`, or `click`), its `delivery_mode` for a visual
click, and a redacted `visual` record:

```json
{"status": "skipped", "reason": "page_structure_candidate"}
{"status": "ok", "capture_id": "...", "region_count": 12}
{"status": "not_installed", "error_code": "not_installed"}
{"status": "error", "error_code": "worker_failed"}
{"status": "unavailable", "error_code": "tool_not_advertised"}
```

`skipped` means the runner did not capture or parse the window. By default
(`--visual-observation auto`) it parses visual regions only when the page
structure offers no executable candidate, because only then can a visual
region add one. This avoids several seconds of CPU parsing per step on the
default fixture. `--visual-observation always` restores the per-step parse,
which also sends regions to Jev alongside page refs, and `off` disables it
(reason `disabled`). `ok` means a validated `cua.visual_regions_v1`
observation was available for that decision. `not_installed` is Driver's stable code when the perception
extension is absent. `error` carries Driver's error code or a local
`capture_mismatch`, `invalid_visual_result`, `capture_missing`, or
`driver_error` code. `unavailable` means Driver did not advertise
`parse_visual_regions` or the capture-bound `click.capture_id` input. The
record never contains screenshots, screenshot references, region text, or
credentials. A failed visual observation never stops the run; the runner
continues on the page-structure path, and the log shows that it did.

The visual Submit candidate first uses `delivery_mode: "background"`. If
Driver refuses that click with a structured background refusal (a
`background_*` code such as `background_unavailable`, or
`escalation.recommended: "foreground"`), the runner does not retry background
delivery. The refused step is logged with `action_error` and
`escalation: {"from": "background", "to": "foreground", "reason": ...}`. The
next step takes a fresh capture and offers a distinct `submit-form-foreground`
candidate, which the chooser must select explicitly. Foreground delivery
activates the browser window. Other action errors still end the run as
`unknown`.

`verify_setup.py` copies this into each `summary.json` check as `submit_tool`,
`acted_path` (`page_structure` for DOM `browser_click`, `visual` for the
capture-bound `click`), `submit_delivery_mode`, the per-step
`visual_statuses`, and any `escalations`.

The default fixture always exposes a semantic `button "Submit"` ref, so its
Submit step uses `browser_click` even when visual regions are available. To
prove the capture-bound visual action path, serve the visual fixture, whose
Submit control is a presentational element with no button ref, and require
the visual path:

```bash
uv run --frozen python verify_setup.py --visual-fixture --require-visual-path \
  --output-dir proof-visual
```

This fails unless every runner verified the fixture through `click` with the
exact `capture_id`. It needs a Driver with the cua-perception extension
installed. Without the extension, the visual fixture cannot be submitted.
`--expect-visual-status not_installed` checks that fallback instead: every
step that attempted a visual parse must log `not_installed`, no runner may
submit, and the run must end without claiming success. Skipped steps do not
count as attempts, but at least one attempt is required. Continuous integration runs that form on Linux.
The standalone server also accepts `--visual-fixture`.

## Run the standalone fixture

For an application or terminal that already owns the server lifecycle, start
the loopback-only fixture in a separate persistent terminal:

```bash
uv run --frozen --python 3.12 python fixture_server.py
```

The fixture prints its local URL. Leave it running while you run either demo.

## Run the deterministic mock proof

The mock adapter returns deterministic typed choices through the same provider
boundary used by the live adapter. It proves the Cua Driver MCP lifecycle,
browser observation and action loop, snapshot-bound references, and independent
postcondition check. It does not prove that the TypeSafe service accepted the
request or made the same choices.

Run the Python example:

```bash
uv run python/run.py --provider mock
```

Run the TypeScript example:

```bash
npm run demo:mock
```

A successful run ends only after the exact submitted value appears at
`/state`.

Both runners also accept `--dry-run`, `--max-steps`, `--token`, and `--log`.
The optional log is JSONL and records the selected candidate, the full
probability vector, and decision/action timings. The candidate set always
includes `reobserve` and `abstain`; reobservation sends no Driver action. Final
outcomes are `verified`, `refuted`, `unknown`, `abstained`, or
`budget_exhausted`. An
uncertain action failure becomes `unknown` and is never retried blindly.

## Verify against live Jev

Live verification is optional and requires your own TypeSafe credential. Set
`TYPESAFE_API_KEY` in the environment, then run one of these commands:

```bash
uv run python/run.py --provider live
```

```bash
npm run demo:live
```

The live adapter sends the task state, any typed visual-region summary, and one
bounded choice question to TypeSafe. It rejects an answer outside the supplied
candidate table.
The runner still owns the control loop: Jev selects one bounded next action,
Cua Driver performs it, and the fixture's `/state` endpoint establishes the
postcondition. Keep sensitive page content out of live runs unless sending it
to the configured provider is appropriate.

The repository's credential-free checks do not establish live Jev behavior.
Record live verification separately when you run it with a valid key.

## Use the bounded chooser CLI

Applications that already own capture, candidate construction, execution, and
verification can call the provider adapter as a single-request process. The
required Python interface reads one `cua.jev_choice_request_v1` JSON document
from stdin and writes one `cua.jev_choice_v1` response to stdout:

```bash
uv run python/choose_action.py --mock < fixtures/jev-choice-request-v1.json
uv run python/choose_action.py < fixtures/jev-choice-request-v1.json
```

The second command is live and lets the TypeSafe SDK read `TYPESAFE_API_KEY`
from its normal environment. The TypeScript equivalent is:

```bash
npm run choose:mock -- < fixtures/jev-choice-request-v1.json
npm run choose:live -- < fixtures/jev-choice-request-v1.json
```

Requests contain a goal, one `capture_id`, compact typed regions, bounded
history, and at most 32 candidate IDs with descriptions. `reobserve` and
`abstain` are required. Tool names, action arguments, screenshot bytes, and
environment data are rejected. Responses contain only the schema, selected
allowlisted ID, provider model identity when available, confidence, and
probabilities. The chooser never executes an action or verifies completion.

## Automation environment and diagnostics

An automation host needs access to the network, loopback sockets, and writable
workspace, package-cache, installation, and temporary directories. On macOS,
`getconf DARWIN_USER_TEMP_DIR` identifies the system user-temp directory. If a
shell sandbox denies installation there, ask its operator to authorize the
specific path; do not disable the sandbox or change shared directory ownership.
For npm, the local cache export above avoids a root-owned global cache.

A run reports `verified` only after an independent fixture readback. If an action
fails with an uncertain outcome, inspect the retained log before retrying.
`--dry-run` on a standalone runner suppresses the candidate action but still
resets the fixture and prepares/navigates the browser; it is not side-effect-free.

The managed verifier bounds each child runner to 180 seconds. Provider SDKs may
retry network requests, so action and process limits do not guarantee a billing
cap. `decision_ms` remains the backward-compatible aggregate from the start of
the fresh semantic snapshot through validated provider choice. Step events also
report `semantic_observe_ms`, `visual_observe_ms`,
`candidate_build_ms`, and `provider_decision_ms`; `action_ms` remains
separate and `total_step_ms` measures through the end of the action (or a
reobserve decision). Lazy visual parsing is timed only when it actually runs.
The small residual between the aggregate and named phases is local
validation/bookkeeping, not another model or Driver call. Logs contain choices
and timings, not a complete request, token-usage, or billing audit. The fixture
tests integration rather than general agent capability.

## MCP, CLI, and perception boundaries

The Python and TypeScript programs are the two complete agent loops. They use a
persistent MCP transport and repeat one explicit session label because browser
targets and snapshot refs belong to that session. Use the Cua Driver CLI for
installation and diagnostics,
for example `cua-driver doctor` and `cua-driver status`, rather than maintaining
a third copy of the loop.

The default fixture exposes semantic browser refs, so the normal proof does
not need screenshot perception and never acts visually. The visual fixture
removes the Submit button ref so only the visual candidate can submit. The optional visual path consumes only the public
`parse_visual_regions` structured result and never adds a model, extension, or
Driver internals to this example. It validates capture identity, PNG geometry,
coordinate mapping, region IDs, bounds, content, confidence, and ambiguity
before constructing a `click` candidate containing the exact `capture_id`.
The coordinate mapping is `screenshot_pixels` for an identity capture or a
finite, invertible `affine` transform otherwise (for example, a Retina window).
The click still carries the original screenshot-pixel point; Driver applies the
capture's mapping itself, so the example never maps a point twice.
Visual evidence never replaces the semantic editable ref required by
`browser_type`.

The credential-free tests load `fixtures/parse-visual-regions-*-v1.json` to
exercise the same parser, candidate builder, and mock Jev boundary used by the
runtime adapter. Separate live-adapter contract tests use the official SDK with
a local fake transport to prove that the request contains the capture ID and
regions and that Jev can return only a supplied candidate ID.
They do not claim that an optional perception extension is installed or assess
its inference quality.

## Run the checks

The tests use the deterministic provider and do not require
`TYPESAFE_API_KEY`:

```bash
uv run python -m unittest discover -s python/tests
npm test
npm run typecheck
```

## Other decision models

For an optional Python-only closed-candidate interface spanning Jev and local
Cua-S1-4B, see [Closed-candidate decision models](decision-models.md). It does
not change the existing Python or TypeScript runners.

## Relationship to PR #3914

[PR #3914](https://github.com/trycua/cua/pull/3914) proposes an optional Jev
policy head inside the Cua Driver binary. This directory is a complementary
standalone example: it composes the public TypeSafe SDKs with the existing Cua
Driver MCP surface and keeps the decide-act-verify loop in application code.
It does not depend on the proposed `suggest_action` tool.

## Acknowledgments

The example was inspired by
[`awlevin/typesafe-computer-use`](https://github.com/awlevin/typesafe-computer-use),
an MIT-licensed early TypeSafe computer-use integration. This implementation
uses Cua Driver's current typed browser and MCP contracts and does not copy its
source code.
