# Stateless native element targeting

Native actions use the opaque `element_token` returned in `structuredContent.elements`. The Rust input contracts remain canonical; for example, `ClickPosition::Element` contains an `element_token`, not an index/snapshot pair.

```json
{"pid":123,"element_token":"<copy the complete observed token>"}
```

Pass that target to the corresponding native action with its required arguments. Do not parse or reconstruct tokens. `element_index` and `snapshot_id` are observation metadata: an index/snapshot pair without identity is refused with `element_identity_required`. Explicit process/window arguments must agree with the token.

## Resolution and delivery

A token authenticates its runtime generation, process, window, and observed accessibility description. Each action resolves that description against a complete current traversal. Missing, ambiguous, disabled, truncated, or unproven targets refuse rather than selecting an ordinal or another window. No snapshot registry, persistent native target cache, or caller journal is involved.

Tokens describe controls, not historical object continuity. Reordering does not change the intended description; a recreated control with the same unique description may match. A later observation does not itself evict an earlier token. A runtime restart invalidates its tokens.

The action retains the selected native object through live checks, geometry, and dispatch. Geometry-based operations use that retained target; failed semantic writes are not retried through another native pattern, typing route, global document lookup, or pointer fallback. Application-driven changes remain possible: neither a native acknowledgement nor retained ownership proves task success or guarantees focus against subsequent application behavior.

## Lifetime and evidence

Native workers inherit authenticated runtime context, cancellation/session state, input admission, and recording context. Cancellation blocks subsequent dispatch starts, while already-issued actions or batches may finish. Cleanup and release events remain possible, and detached work keeps its admission resources until cleanup. A resolution timeout bounds the caller's wait; it cannot terminate an operating-system accessibility RPC already in flight.

Uncertain native failures preserve an unknown-delivery action record and require fresh observation before any retry. Partial or unchanged text read-back is not evidence that a suffix can safely be replayed.

Recording preparation does not independently resolve an element. Pointer evidence is captured at delivery from the dispatched point. Semantic actions need honest non-spatial evidence rather than an invented pointer location. Recording finalization reports uncertainty if it fails after an action.

## Explicit adapter limitations

- **Windows MSAA:** the existing projection does not preserve the child identity needed for retained live validation. Semantic token targeting refuses; there is no automatic UIA or coordinate substitution.
- **Windows UIA:** exact HWND ownership, complete provider reads, and a control-view ancestry proof are required. Process-wide fallback trees cannot authorize a window-scoped action. Worker-held MTA usage keeps the apartment lifetime available while retained references exist.
- **Windows scroll:** an identity-bearing request uses only the retained element's `ScrollPattern`. Unsupported targets refuse rather than sending window-level scroll input. Provider failures and interrupted partial batches remain unknown; cancellation during provider lookup prevents the first mutation. The worker retains background protection through cleanup.
- **Windows text:** `ValuePattern.SetValue` replaces content; it is not caret-based insertion. `type_text` no longer appends through that pattern and then retries as character messages. Background delivery requires a proven keyboard destination and a supported character-message route; unsupported frameworks require an explicitly requested foreground route. `set_value` remains the replacement operation.
- **macOS:** application-global roots without a proven native window binding refuse. Popup value selection requires one matching native child option; it never searches Safari's front document for an unrelated select element. Numeric value writes choose their native representation before dispatch; stepping, when supported, is selected before any write, not after a failed write.
- **Linux/Wayland:** AT-SPI must prove the native-window/frame mapping, even when only one accessible frame exists. X11 correlation requires complete frame geometry and refuses when the selected frame also plausibly matches another native window; nearby or overlapping same-process windows can therefore be unsupported. Hyprland requires its attested unique-title mapping; other Wayland compositor mappings refuse. An unavailable or ambiguous mapping never borrows another frame or a foreground destination.

## Compatibility and release review

Rejecting identity-free index addressing is a behavioral compatibility change. Migrate callers to observed tokens before adoption. No new public target format or generated binding surface is introduced by this consolidation. The landing release metadata must acknowledge the removed addressing behavior; it must not be presented as a tests-only or non-releasing change.

Cross-platform native behavior must be certified by the repository CI/harnesses on the eventual candidate revision. Compilation and local unit/contract tests are not native desktop certification.
