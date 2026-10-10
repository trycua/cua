# Protected debug mode reference

Protected debug mode is an opt-in, non-certifying diagnostic mode for frozen
production trials executed by `lume-macos-certifying`.

## Command option

| Option | Type | Default | Constraints |
| --- | --- | --- | --- |
| `--debug` | Boolean flag | Disabled | Requires a production system, `lume-macos-certifying`, and no `--apparatus-check` |

Debug mode does not alter the task timeout, reset procedure, network policy,
participation requirements, protected mediator, cleanup sequence, stopped-disk
collection, or pristine-state verification.

## Eligibility semantics

| Property | Debug-mode value |
| --- | --- |
| Execution-policy violation | `debug_mode` |
| Comparison eligibility | `false` |
| Final certification | `false` |
| Trial export | Rejected |
| Apparatus and participation receipts | Still produced and verified when available |

`config.json` and `result.json` contain `debug_mode: true`. `cdb explain`
requires those values to match and verifies the diagnostic artifact contract.

## Diagnostic artifact

The artifact path is `artifacts/agent.debug.json`.

| Field | Type | Meaning |
| --- | --- | --- |
| `schema_version` | Integer | Diagnostic schema version; currently `1` |
| `mode` | String | Always `protected-content-free` |
| `events` | Array | At most 1,024 normalized progress records |
| `parsed_event_count` | Integer | Number of retained normalized records |
| `skipped_line_count` | Integer | Oversized, malformed, or excess source lines |
| `token_totals` | Object or null | Greatest complete cumulative input, output, cache-read, and cache-write totals |
| `termination` | Object | Completion, failure, timeout, exit code, and bounded failure class |
| `provider_activity` | Object or null | Sealed connection and byte counters from the host provider proxy |
| `output` | Object or null | Raw stream byte counts, SHA-256 digests, and truncation flags |
| `raw_output_persisted` | Boolean | Always `false` |

Each event contains a one-based `sequence`, a closed normalized `event_type`,
and optionally a `tool_name`. A tool name is retained only when it matches the
frozen production tool inventory.

## Retention boundary

The diagnostic artifact may contain only:

- normalized event types and sequence numbers;
- frozen-inventory tool names;
- token totals;
- provider connection and byte counts;
- bounded termination and failure classes; and
- stream byte counts, hashes, and truncation flags.

It must not contain prompts, responses, reasoning text, tool arguments, raw
stdout, raw stderr, screenshots, credentials, provider payloads, or protected
application content.

The runtime does not add the artifact to `AgentOutcome.artifacts`, signed
apparatus evidence, comparison manifests, or exported trial evidence.

## Timeout recovery

After the normal timeout terminates the guest agent, the host asks the
privileged helper for the existing bounded output tail and full-stream
metadata. Normalization happens in host memory. Raw content is discarded after
the content-free document is written. A recovery failure produces the bounded
`debug_capture_unavailable` classification and does not replace the original
timeout.

## Related references

- [`cdb` command reference](cli.md)
- [Trial directory reference](trial-artifacts.md)
- [Outcome, participation, certification, and comparison](../explanation/outcome-participation-certification-and-comparison.md)
