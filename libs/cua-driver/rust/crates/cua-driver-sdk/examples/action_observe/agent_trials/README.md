# Local real-agent qualification, 2026-10-06

Five fresh alternating pairs ran locally after the Mac session was unlocked. Both arms used the same supported SDK connection to the existing signed Cua Driver 0.34.0 daemon. Fresh preflight recorded the latest release, exact SDK upstream source `aa7a31a5f3633249db69f766256b1749feb7a652`, compiled host digest, actual SDK daemon metadata/PID, and matching permission attribution. The Reference checkout supplies only the synthetic AppKit fixture/oracle; it delivers no input.

The actual agent was Codex CLI requested model `gpt-6-sol`, low reasoning, ephemeral, with only the fixture MCP server registered and shell/unified execution disabled. The gateway enforced owned PID/window, current observation tokens, the three allowed literal actions, and one attempt per field. Baseline and composite used the same goal and strict final `{ "completed": true }` response contract. Success required that response, a final fresh observation containing all three correct values, the independent fixture state, and no unrelated tool items. No inputs were replayed.

| Metric | Standalone | Action plus read |
|---|---:|---:|
| Strict task successes | 5/5 | 5/5 |
| Median entire CLI task time | 25.923 s | 25.391 s |
| Median gateway tool time | 4.867 s | 4.872 s |
| Agent-visible calls, median (range) | 7 (7–7) | 5 (5–7) |
| Canonical child dispatches, median (range) | 7 (7–7) | 8 (8–10) |
| CLI-reported input tokens, median | 171,448 | 138,171 |
| Cached input tokens, median | 155,776 | 116,096 |
| Uncached input tokens, median | 22,292 | 22,186 |
| Output tokens, median | 366 | 350 |
| Overall CLI turn events per task | 1 | 1 |
| Internal provider request/roundtrip count | unavailable | unavailable |

The demonstrated gain is fewer visible calls in four of five composite runs and lower median aggregate input tokens. There is no consistent task latency improvement: the composite was faster in only two of five pairs; median paired composite-minus-standalone time was **+0.603 s**. Uncached input was essentially unchanged. Cache state, cloud scheduling, and response-envelope complexity were not experimentally controlled; this is not a reliable speed multiplier or spending/compute saving. Overall CLI `turn.completed` events are not internal model request counts.

The agent chose an additional final read in every composite run and additional reads after both text actions in the last run. These calls are retained rather than replaced with the ideal four-call loop. Canonical counts come from helper dispatch receipts plus standalone calls; acknowledgments are not used to infer successful effects. Each arm also performed two common setup reads, excluded from visible-loop counts and gateway tool sums but included in whole CLI wall time. The raw summary field `driver_ms` is the sum of measured gateway request times, including SDK invocation, validation, and independent-oracle waiting; it is not pure OS execution latency.

All ten outcomes had Full name `Synthetic Person`, Email `synthetic@example.invalid`, Subscribe enabled, and zero submissions. There were zero false completions, gateway refusals, or unrelated tool items. Each gateway wrote its receipt after closing its SDK host and owned fixture; a final process check found no owned gateway/fixture processes. No VM, virtual display, grant change, installed-daemon replacement, real document, or authenticated browser was used.

The earlier nine locked/setup preparation attempts remain separately retained in `../preparation_failures.json`. An unlocked two-arm smoke with the original overly strict stop-on-unverifiable-receipt instruction is excluded from this scored packet: both oracle states were correct, but the baseline stopped before final observation. The scored prompt instead requires observing uncertain acknowledgment effects without replay.

`manifest.json` binds ten exact original CLI JSONL streams to their compressed copies and independent gateway receipts. The streams contain only synthetic fixture projections and actual usage; app menus, screenshots, credentials, private configuration, and private prompt/context are not retained. `results.json` preserves trial summaries and preflight source hashes; `statistics.json` is recomputed from those summaries. The final source differs from the host compilation only in scoring/runner/test files; `operation.rs` and the Rust host bytes match the tested compiled implementation. Current local checks passed SDK 102 tests, example 11 tests, gateway 7 tests, Python syntax, formatting, and diff checks. Windows/X11/Wayland/VM acceptance is not claimed.

Provider and fixture identifiers were anonymized after measurement; timings are unchanged. Historical hashes identify the original inputs, while publication hashes identify edited copies.
