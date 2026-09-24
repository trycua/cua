# Closed-candidate decision models

The optional Python `choose_decision.py` interface lets an application choose
between TypeSafe Jev and a local Cua-S1-4B model without giving either model
control of Cua Driver. It is an example-local integration, not a new Driver,
MCP, or cross-language contract. The existing Python and TypeScript `jev-use`
runners remain unchanged.

## Boundary

The application observes through Driver, constructs an immutable candidate
table, and sends only the goal, capture ID, typed visual regions, bounded
history, and candidate IDs and descriptions to a decision model. The model
returns a score for each supplied ID. The adapter rejects missing, extra,
non-finite, out-of-range, or non-normalized scores, then returns one of four
outcomes: `selected`, `reobserve`, `abstain`, or `error`.

`selected` is not an action result. The application must look up the selected
ID in its original candidate table, check that the capture is still current,
dispatch the prebuilt action through Driver, and verify the postcondition from
an independent source. `reobserve`, `abstain`, and `error` dispatch no action.
The decision response never contains tool names, tool arguments, coordinates,
or screenshot bytes.

The input is the existing `cua.jev_choice_request_v1` request accepted by
`choose_action.py`. The output is a separate
`cua.decision_choice_v1` response with `kind`, `capture_id`, `selected_id`, `model`,
`confidence`, `probabilities`, and `reason`. This output is not a drop-in
replacement for the older `cua.jev_choice_v1` response; callers must handle
all four outcome kinds and match `capture_id` to the current observation.
Tied top scores are non-actionable errors. For a `selected` outcome, callers
should also apply their own minimum score and margin before dispatching an
action. `confidence` preserves the provider's reported value when Jev supplies
one; with S1 and mock it is the selected option probability. For a
model-independent threshold, use `probabilities[selected_id]` instead.

Malformed model output and lazy model-loading failures return `kind: "error"`
with a non-secret `reason: "model_error"`. Missing local S1 paths or a failure
to initialize the TypeSafe client stop the CLI with a generic nonzero setup
error and no JSON response. Never treat a missing response as `selected`.

## Run a bounded request

The deterministic mock requires no model weights or credential:

```bash
uv run --frozen python/choose_decision.py --model mock \
  < fixtures/jev-choice-request-v1.json
```

To verify the expected fixture choice without printing the full response, run
`uv run --frozen verify_decision_cli.py --model mock`. The same verifier accepts
`--model jev` or `--model s1` after configuring that provider. It checks the
response schema, capture ID, complete candidate set, and selected ID. It does
not dispatch a Driver action or prove live desktop behavior.

The separate negative fixture has a `Save` observation but an action candidate
that requires `Send`. To check a live provider's abstention without executing
the candidate, run `uv run --frozen verify_decision_cli.py --model jev
--fixture negative --expected-id abstain` in an authorized local environment.
The mock intentionally chooses the first action candidate and is not an oracle
for this negative case.

The TypeSafe Jev path uses the example's official SDK and reads its credential
from the local environment. Run it only with data you intend to send to that
service. Do not put `TYPESAFE_API_KEY` in CI or GitHub Actions:

```bash
uv run --frozen python/choose_decision.py --model jev \
  < fixtures/jev-choice-request-v1.json
```

The S1 path runs locally and needs the separately installed `cua-s1` Python
package, PyTorch, PEFT, and a Transformers build that recognizes
`Qwen/Qwen3.5-4B`. The repository's `cua-s1[four-b]` dependency currently
pins Transformers below 5, which cannot load this base model; the isolated
proof used Transformers 5.17.0. Resolve and test that packaging constraint
before treating the command as a supported installation recipe.

The published `cua-ai/cua-s1-4b-0.2` artifact is a PEFT adapter, not a
standalone model. Pin and locally download both the base revision
`851bf6e806efd8d0a36b00ddf55e13ccb7b8cd0a` and adapter revision
`16818868b0cc7813808aae4e87b417657046ab79`, then set
`S1_BASE_MODEL_PATH` and `S1_ADAPTER_PATH` to those local directories:

```bash
S1_DEVICE=cpu S1_DTYPE=float16 \
  /path/to/s1-venv/bin/python python/choose_decision.py --model s1 \
  < fixtures/jev-choice-request-v1.json
```

The S1 text adapter renders OmniParser regions as text and labels the result
`Visual-region-derived observation`. It is not a genuine accessibility tree,
even though the underlying S1 prompt still titles this text field
`Accessibility tree:`. The reserved options are represented as closed-choice
decisions; their accuracy is not established by S1's element/action training.
S1's one-letter-per-option readout supports at most 26 candidates, including
`reobserve` and `abstain`; a larger validated request returns `error` with
`reason: "option_limit"` and never silently truncates the table. The Jev
request schema permits up to 32 candidates. The S1 multimodal adapter requires
an existing local screenshot path; that path and image are never added to a
TypeSafe request. The CLI currently exposes only the text adapter.

## Verification scope

The focused fake-model tests cover the selection contract, reserved outcomes,
malformed scores, the 26-option boundary, and absence of screenshot and action
arguments in the TypeSafe request. A local pinned-weight S1 text inference
selected the expected `submit-form` candidate from the synthetic fixture
request. That is a runtime smoke, not evidence that S1 matches Jev's decision
quality or that the S1 path has been run end to end on a desktop.

In a separate synthetic mismatch where the observation said `Save` but the
only action condition required `Send`, Jev selected `abstain` while S1
selected `reobserve`. Both choices prevent an action, but S1 did not follow
the fixture's expected outcome. Keep an independent fixture oracle and a
caller-owned score and margin policy for future desktop comparisons.
