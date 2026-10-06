# Literal text-plan qualification, 2026-10-06

The actual Cua-native example completed both supplied text setters with independently verified values, Record A context, Subscribe unchanged and zero submissions. Removing redundant JEV choices produced a small measured gain on this bounded subset. Batching deterministic steps alone did not produce a gain. The full form remains an unqualified specialist task because its JEV checkbox controls failed.

| Matched actual Cua recipe | With JEV | Without JEV |
|---|---:|---:|
| Verified completion | 3/3 | 3/3 |
| False completion | 0 | 0 |
| Median wall time | 3.963 s | 3.603 s |
| Driver calls per trial | 6 | 6 |
| Provider requests per trial | 2 | 0 |
| Median sum of provider request time | 0.362 s | 0 |
| Median sum of Driver tool time | 3.620 s | 3.596 s |

The wall reduction is about 9.1%. Medians of separate components need not sum to median wall time. There were only three alternating pairs, no excluded warm-up pairs and no population significance claim. Wall time includes fresh observations, input, model requests where present and independent oracle checks; fixture launch/window discovery are excluded equally. The matched executor, selectors, literal values, budgets, context predicate and oracles are identical. Each JEV request chooses an already-resolved single allowed SET_VALUE action; the returned complete action must equal the caller's literal action before its candidate ID is accepted. JEV cannot expand authority. This baseline uses Arc's upstream TypeSafeJevPolicy, not Cua's normal v2 chooser request. No actual frontier model was timed or counted, and no token-cost saving was measured.

Latest-candidate preflight checked released, installed and running Cua Driver 0.34.0 (release commit `b0968e1b12834e485dda68789541a3cc57664a9f`) and current Arc 0.1.1/source `74ffae1108b1cb4b1f6b161084af12646f544ba5` immediately before the run; exact timestamps and running serverInfo are in the summaries. The Cua recipe base is upstream main `0b90b6f4af6885ecbe696a6b33a3ad63773183d4`. macOS was 27.0.1, evaluation Python 3.12.12. All six actual HTTP requests returned 200 and model `jev-1.13.0` (requested alias `jev-latest`). Credentials were retrieved at runtime and never printed or persisted. Only owned synthetic native fixtures were operated; no virtual display, GPU, VM or user app was used.

The measured recipe file SHA-256 is `4b136721d5b5908d7ee669ae88b94010d65ecc1f961956dcec93f34d21532060`. The subsequent production-file difference only clarifies the existing DriverLike error/refusal contract in a docstring; the executable recipe is unchanged. Added tests and these documents do not affect the live measurements.

## Retained negative controls

- `results-summary.json`: three original-form trials per arm. Stepwise deterministic and grouped deterministic both passed 3/3, each with six Driver calls. Medians were 4.673 s and 4.808 s; grouping did not speed the driver. The logged `caller_invocations` counters describe a conceptual delegation contract (three steps versus one plan), not measured client RPCs or frontier-model calls. Open-ended JEV failed 0/3 at a median 24.615 s with 10, 10 and 11 provider requests. Failed tasks are not counted as speed wins.
- `grounded-imprecise-goal-results-summary.json`: the same three-step form with singleton choices. Literal execution passed 3/3; JEV failed 0/3 at the checkbox. Its generated goal said “Enable Subscribe”; no decision-level trace was captured in this preparation comparison, so the exact refusal reason is not established.
- `grounded-results-summary.json`: specifying “Click the unchecked Subscribe checkbox once to check it” and a checked-value verification criterion did not fix the failure. Literal execution passed 3/3; JEV filled both fields, then returned BLOCKED in all three runs. No success-speed comparison is made for this broader task. The port intentionally covers text setters only.

## Reproduce

Ordinary use of `python/literal_text_plan.py` needs no Arc installation, TypeSafe SDK client or model credential. It uses the existing Cua native example's dependencies and supported Driver connection. The research comparison harness optionally uses Arc's policy and its owned AppKit fixture to preserve continuity with the earlier evaluation; those are external benchmark dependencies only.

Check out the OH comparison harness from `https://github.com/open-horizon-labs/computer-use` at `3a82afb` (the helper files live under `experiments/arc-cua-comparison-2026-10-05`). Prepare a private Python 3.12+ environment with the current Arc macOS source installed editable and HTTP/2 support. Set `OH_COMPARISON_SOURCE` to that checkout, `ARC_EVAL_SOURCE` to the current Arc source checkout, and `CUA_LITERAL_SOURCE` to this Cua checkout. Supply the JEV credential at runtime through `TYPESAFE_API_KEY` or `JEV_CREDENTIAL_COMMAND` (a JSON argv array for the configured secret-access mechanism); never place a secret in source or command arguments. Both candidate preflights intentionally fail if the installed/running Cua version or installed Arc source is stale.

From this evidence directory, run `run_port.py` with that evaluation interpreter. It launches and closes fresh owned synthetic windows, alternates three paired trials, runs the actual port module and checks application-owned state independently. `run_trial.py` and `run_grounded.py` reproduce the broader original and explicit-checkbox controls; the imprecise wording archive is retained historical preparation evidence. A single observer/actor MCP session is used sequentially; every mutation consumes the freshly rebuilt source-produced arguments.

Readable summaries omit full MCP calls. The four `*.json.gz` files preserve the original full JSON traces byte for byte. `TRACE-MANIFEST.json` records raw and compressed SHA-256 hashes and byte lengths. Decompress and inspect them when reviewing binding, refusals or individual timings. Reproduction runners accept external source/credential configuration; that change does not alter measured task or executor behavior.
