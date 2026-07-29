# Cursor modifier badge restructuring plan

**Status:** Draft for review
**Base:** `main` at `b249df857994ebaebf9f02248e8312b3bd2a5dd2`
**Scope:** Move delivery and target modifiers from around the agent pointer into the session badge while preserving action animations beside the pointer.

## Outcome

Cua Driver should present two kinds of cursor information in two clear places:

- The pointer shows the current action, such as click, drag, scroll, or text.
- The badge shows execution context:
  - delivery: background or foreground
  - target: AX, pixel, browser, or desktop

The badge remains lightweight. The session name fades using the existing two-second hold and 400-millisecond fade. Modifier chips appear only while their action is active, followed by a 400-millisecond trailing fade. When the name is hidden, the badge contracts to a compact session orb and active modifier chips. Hovering the pointer restores the full session name where hover observation is supported.

This is visual feedback only. It is not an authorization indicator, a security boundary, or evidence that an action succeeded.

## Definition of done

The work is complete when:

1. Action animations remain visually unchanged beside the pointer.
2. Pointer-relative delivery and target artwork is no longer painted.
3. Delivery and target appear as upright badge chips on macOS, Windows, X11, native Wayland, and the GNOME helper path.
4. A delivery chip and target chip can appear together without overlap.
5. Session-name and modifier visibility use independent, deterministic timing.
6. Badge geometry, timing, glyph semantics, and dirty extents live in shared code wherever the platform permits it.
7. The cursor theme source contract moves cleanly to v2 with twelve action animations and no modifier animation section.
8. Compiled v2 themes contain action artwork only and reject obsolete v1 artifacts with a clear rebuild message.
9. Cursor overlay tests run in CI instead of only on developer machines.
10. Unit, gallery, cross-platform E2E, documentation, and visual evidence are updated.

## Design decision

### One badge with independently fading content

Use one pill rather than adding a second persistent status capsule.

| Badge content             | Visibility                                                                              |
| ------------------------- | --------------------------------------------------------------------------------------- |
| Session orb and name      | Existing reveal, two-second hold, 400-millisecond fade; hover restores full opacity     |
| Delivery and target chips | Full opacity while modifiers are active; 400-millisecond trailing fade after they clear |
| Badge chrome              | Visible while either the name or modifier chips are visible                             |

Starting an action must not restart the session-name timer. Otherwise a long session name would flash on every tool call. Modifier changes reveal or update the chips only.

When the name has faded, the pill contracts to the session orb and active chips. If no public session label exists, modifier-bearing actions still get this compact session-colored capsule. This preserves context without requiring a display name.

### Host-owned modifier glyphs

The badge owns six small vector glyphs:

- background
- foreground
- AX
- pixel
- browser
- desktop

They are rendered through the shared Skia path at the live backing scale. Delivery uses a filled chip treatment. Target uses an outlined chip treatment. Shape, not color alone, communicates the difference.

Do not reuse theme-authored modifier frames inside the badge:

- Existing frames contain pointer-relative placement, not centered glyph geometry.
- Their authored colors and stroke weights cannot guarantee badge legibility.
- The background animation is a long gesture trail and does not fit a square chip.
- The GNOME helper is a separate renderer and cannot consume compiled `.cua-theme` artifacts.
- Reusing them would require an artifact version bump and a permanent compatibility decoder.

The badge and its modifier glyphs therefore remain outside the dotLottie theme contract.

### Preserve the current badge envelope

Keep:

- `BADGE_HEIGHT = 28`
- `BADGE_MAX_WIDTH = 188`
- `BADGE_CURSOR_GAP = 25`

Use 18-point modifier chips with a four-point gap. The text budget becomes dynamic:

- no chips: current text width
- one chip: reduced text width
- two chips: further reduced text width

The label must ellipsize to its measured width and never wrap. The current font layout can wrap a wide 28-character label outside the pill, so this restructuring must fix that existing defect before reducing the text budget.

## Breaking theme contract

This change deliberately replaces the recently introduced v1 theme contract rather than carrying an unused compatibility layer.

Use:

- `cua.cursor-theme/2`
- semantic vocabulary version `2`
- `cua-driver-actions-v2`
- compiled artifact version `3`
- `CUATHEM3`

The v2 source manifest contains only `actions`. A `modifiers` field is rejected with a focused migration message. The compiled `CompiledTheme` structure drops its modifier map entirely.

Existing v1 `.cua-theme` files are rejected with a clear instruction to rebuild the source as v2. No decoder shim, aliases, ignored fields, or transitional profile are added.

Regenerate `cua.default.lottie` and `cua.default.cua-theme` without modifier animations. The new source hash and artifact are checked in together and verified reproducibly.

## Shared renderer changes

### Badge geometry

Refactor `cursor-overlay/src/session_badge.rs` so geometry is computed before painting.

Add shared inputs and outputs similar to:

```rust
pub struct SessionBadgeInput<'a> {
    pub label: Option<&'a str>,
    pub delivery: Option<DeliveryModifier>,
    pub target: Option<TargetModifier>,
    pub cursor: (f32, f32),
    pub backing_scale: f32,
    pub label_alpha: f32,
    pub chip_alpha: f32,
    pub clip: Option<(f32, f32)>,
}

pub struct SessionBadgeLayout {
    pub rect: Rect,
    pub orb: Rect,
    pub label: Option<BadgeLabelLayout>,
    pub delivery_chip: Option<BadgeChip>,
    pub target_chip: Option<BadgeChip>,
    pub pill_alpha: f32,
    pub label_alpha: f32,
    pub chip_alpha: f32,
}

pub fn session_badge_layout(
    input: SessionBadgeInput<'_>,
) -> Option<SessionBadgeLayout>;

pub fn paint_session_badge(
    pixmap: &mut Pixmap,
    layout: &SessionBadgeLayout,
    session_fill: [u8; 4],
    alpha_scale: f32,
);

pub const fn session_badge_extents() -> BadgeExtents;
```

The shared layout function owns:

- chip ordering
- dynamic text width
- ellipsis
- edge clamping
- full and contracted pill widths
- modifier and badge hit geometry
- worst-case dirty extents

Platform adapters must derive their dirty regions from `session_badge_extents()` instead of copying badge widths.

### Modifier glyphs

Add `cursor-overlay/src/badge_glyphs.rs` with host-owned vector geometry and typed mapping from `DeliveryModifier` and `TargetModifier`.

Glyphs must:

- remain upright and unrotated
- use the session fill and white-outline visual language
- render cleanly at 1x, 2x, 3x, and fractional backing scales
- fit entirely inside an 18-point chip
- retain shape distinction in monochrome screenshots

### Badge state

Extend `RenderStateCore` with a short-lived modifier latch so chips can fade after `CursorVisualState` returns to idle:

```rust
pub badge_modifiers:
    Option<(Option<DeliveryModifier>, Option<TargetModifier>)>,
pub badge_modifier_clear_secs: Option<f64>,
```

Add shared methods for:

- modifier chip alpha
- whether any badge content is visible
- producing `SessionBadgeInput`
- hit testing the pointer plus current badge geometry

`BeginAction` updates the modifier latch without revealing the session name. `tick_idle` starts the trailing fade when semantic modifiers clear. Idle cursor opacity still multiplies the entire badge, so the badge cannot outlive the cursor.

### Theme painting

In `theme_artifact.rs`, stop compositing delivery and target animation layers with the pointer transform. Continue painting the selected action animation exactly as before.

Remove modifier animation storage and compositing from the compiled artifact.

## Platform work

### macOS

Use the common renderer and layout without platform-specific badge geometry. The existing full-screen overlay and hardware-pointer hover polling remain unchanged.

### Windows

Derive the layered-window dirty envelope from shared badge extents. Fix the current stale invariant where `CURSOR_PAD = 192` no longer covers `BADGE_MAX_WIDTH + 8`.

Run the invariant test in CI instead of compiling it without executing it.

### X11

Derive horizontal and vertical tile margins from shared badge extents. The current vertical tile margin has little spare room below the badge and should not depend on a copied constant.

Use shared badge visibility rather than `session_label.is_some()` when deciding whether the tile must include badge space.

### Native Wayland

Continue using the shared renderer. Modifier chips and timed visibility work identically. Hover reveal remains unavailable through the stock protocol because the client cannot observe another client's global pointer.

### GNOME helper

Remove pointer-relative modifier drawing. Render the same two badge chip slots in the Shell badge and mirror the shared timing constants.

Keep the D-Bus action payload unchanged. Bump the bundled helper version and its Rust-side version pin together. Add parity fixtures so glyph identity, ordering, and timing cannot drift silently.

## Implementation sequence

### Phase 0: make current tests trustworthy

1. Run `cursor-overlay` unit tests in Linux CI.
2. Execute pure renderer tests in Windows CI.
3. Expose and fix the stale Windows dirty-envelope assertion.
4. Add CI coverage for shared modifier and badge tests before behavior changes.

### Phase 1: extract shared geometry with no visual change

1. Add `SessionBadgeInput`, `SessionBadgeLayout`, and `session_badge_extents`.
2. Reimplement the existing label-only badge on that layout.
3. Add width-based ellipsis and prevent wrapping.
4. Prove label-only rendering and timing are unchanged.
5. Derive Windows and X11 envelopes from shared extents.

### Phase 2: add badge glyphs and modifier lifecycle

1. Add the six shared badge glyphs.
2. Add delivery and target chip slots.
3. Add the modifier latch and trailing fade.
4. Support compact chips-only layout for faded or unlabeled sessions.
5. Extend hover hit testing to the visible badge rect while keeping the overlay click-through.
6. Verify label reveal is not restarted by tool actions.

### Phase 3: move presentation away from the pointer

1. Temporarily render badge chips alongside existing pointer modifiers for visual comparison.
2. Approve glyph legibility at production size.
3. Remove modifier compositing from `paint_compiled_theme_with_tint`.
4. Replace pointer-position tests with tests proving modifiers no longer change pointer pixels.

### Phase 4: replace the theme contract

1. Introduce source schema v2, semantic version 2, profile v2, and artifact v3.
2. Remove `modifiers` from the source manifest and `CompiledTheme`.
3. Reject v1 sources and artifacts with focused migration messages.
4. Update CLI validation, inspection, and preview behavior.
5. Regenerate the default dotLottie source and compiled artifact without modifier animations.
6. Verify the regenerated artifact is reproducible and contains exactly twelve actions.

### Phase 5: complete GNOME parity

1. Add badge chip widgets and timing.
2. Remove pointer-relative modifiers.
3. Support compact badge content without a public label.
4. Bump helper version and tests.
5. Verify GNOME and shared-renderer state transitions against common fixtures.

### Phase 6: gallery, documentation, and evidence

1. Rebuild modifier gallery fixtures using complete `RenderStateCore` badge rendering.
2. Update the combined foreground and pixel example.
3. Add:
   - full label with one chip
   - full label with two chips
   - contracted orb with one chip
   - contracted orb with two chips
   - no-label modifier capsule
4. Regenerate the public delivery and target GIF.
5. Update cursor personalization documentation and theme author instructions.
6. Record native videos on macOS, Windows, X11, and Wayland.
7. Upload durable review media to the implementation PR.

## Test matrix

### Shared unit tests

- all delivery and target combinations
- no label, short label, and maximum-length label
- label ellipsis without wrapping
- full and contracted layouts
- stable delivery-then-target chip order
- chip and text non-overlap
- edge clamping on every screen edge
- backing scale at 1x, 2x, 3x, and a fractional scale
- reduced-motion still frames
- modifier fade after one-shot, held, and looping actions
- modifier changes while the session name is faded
- action start does not restart the name timer
- idle cursor fade hides all badge content
- hover restores the full name without changing modifier state
- v2 source manifests reject a `modifiers` section
- v1 source and compiled artifacts produce focused rebuild guidance
- compiled themes contain exactly the required action map and no modifier payload
- delivery and target state no longer changes pointer pixels

### Platform verification

| Platform       | Required evidence                                                                               |
| -------------- | ----------------------------------------------------------------------------------------------- |
| macOS          | Lume install from exact source SHA; full action sequence; hover reveal; edge placement; video   |
| Windows        | GitHub Actions plus interactive VM when available; layered-window edges; two-chip layout; video |
| Linux X11      | GitHub Actions or VM; tile bounds at all edges; two-chip layout; video                          |
| Native Wayland | Shared-renderer chips and timing; bottom-edge clipping; documented hover limitation; video      |
| GNOME helper   | Version handshake; label and chip timing; empty-label capsule; parity recording                 |

Use semantic and pixel oracles. A successful tool result alone is not visual proof.

## Documentation updates

Update:

- `libs/cua-driver/docs/cursor-themes.md`
- the public cursor personalization page
- `tools/cursor-gallery/README.md`
- gallery headings, descriptions, combined-state examples, and legends
- theme manifest examples
- post-install or upgrade notes only if the GNOME helper requires user action

Document clearly:

- action animations belong to the pointer
- delivery and target context belong to the badge
- modifiers are host-rendered and not theme-customizable
- v2 themes contain twelve action animations and no modifier animations
- v1 themes must be rebuilt for the v2 contract
- chips describe intended execution context only
- Wayland hover limitations do not affect modifier rendering

## Risks and mitigations

| Risk                                              | Mitigation                                                                                                                    |
| ------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------- |
| Chips are too small at 1x                         | Review exact production pixels before removing pointer modifiers; increase chip size only within the unchanged badge envelope |
| Long labels collide with two chips                | Measure and ellipsize text; never wrap                                                                                        |
| Badge becomes too noisy                           | Do not reveal the name on action start; chips exist only during actions plus one shared trailing fade                         |
| Existing v1 custom themes stop loading            | Provide clear rebuild guidance in the CLI and upgrade documentation                                                           |
| Platform geometry drifts                          | Centralize layout and extents; adapters consume shared outputs                                                                |
| GNOME renderer drifts                             | Share protocol semantics and fixtures; version the helper; verify recordings                                                  |
| Modifier chips are mistaken for permission status | Avoid security language and colors; explicitly document that they are informational                                           |
| Rollback crosses incompatible artifact versions   | Keep the v2 source archive reproducible and document that driver and theme artifacts must move together                       |

## Acceptance checklist

- [ ] Pointer action animations are unchanged.
- [ ] Pointer-relative modifier artwork is gone.
- [ ] Delivery and target chips can appear together in the badge.
- [ ] Session name and chips follow independent fade rules.
- [ ] Compact chip layout works with and without a public label.
- [ ] Badge height and maximum width remain unchanged.
- [ ] Platform dirty regions derive from shared extents.
- [ ] The v2 theme contract contains exactly twelve action animations.
- [ ] V1 source and compiled themes fail with clear rebuild guidance.
- [ ] Cursor overlay tests execute in CI.
- [ ] macOS, Windows, X11, native Wayland, and GNOME paths are verified.
- [ ] Gallery, public docs, personalization docs, and review media are updated.

## Rollback

The first two phases are additive and should remain even if the visual move is rejected because they fix test, padding, and label-layout defects.

The behavioral switch can be rolled back by restoring the v1 default source and artifact together with modifier layer compositing. Because the contract is intentionally breaking, the driver and installed theme artifact version must move together.

The GNOME helper and Rust helper-version pin must always roll forward or back together.

## Review decisions

The plan recommends:

1. Show compact session-colored badge chrome for modifier-bearing actions even when the session has no public label.
2. Replace the v1 theme contract instead of retaining unused modifier animations.
3. Upgrade the GNOME helper atomically with the driver.
4. Judge final chip size using the production renderer at 1x before removing pointer modifiers.

The breaking theme-contract choice is approved. The remaining visual decisions are reviewed against exact production pixels.
