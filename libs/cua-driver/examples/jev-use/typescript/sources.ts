/**
 * Candidate sources: where a decision's executable candidates come from.
 *
 * A candidate source turns one Driver observation into addressable controls
 * and builds fully specified candidates for them. The model only ever sees a
 * candidate's ID and description; the tool name and exact arguments stay in
 * the runner. A task (see tasks.ts) decides which controls matter and which
 * candidate IDs and descriptions to offer; the source decides how to address a
 * control and which Driver tool acts on it.
 *
 * Implemented sources:
 *
 * - BrowserSemanticSource reads a get_browser_state semantic_v2 snapshot and
 *   acts through browser_type / browser_click on its refs.
 * - VisualRegionSource reads a validated parse_visual_regions result and acts
 *   through a capture-bound click on a region's center.
 *
 * Extension point (RFC #4268, Phase 1): a NativeAccessibilitySource will read
 * get_window_state elements, find controls by normalized role and label, and
 * act through the snapshot's element_token. It is deliberately not implemented
 * here; it must implement CandidateSource with kind 'ax'.
 */
import type {
  BrowserSnapshot,
  PageRef,
  VisualDelivery,
  VisualObservation,
  VisualRegion,
} from './core.js';

/** 'ax' is reserved for the future NativeAccessibilitySource. */
export type SourceKind = 'page' | 'visual' | 'ax';

export type Candidate = Readonly<{
  id: string;
  description: string;
  tool: string | null;
  arguments: Readonly<Record<string, unknown>>;
  captureId?: string;
  screenshotReference?: string;
}>;

export function immutableCandidate(candidate: Candidate): Candidate {
  return Object.freeze({ ...candidate, arguments: Object.freeze({ ...candidate.arguments }) });
}

export function asciiLower(value: string): string {
  return value.replace(/[A-Z]/g, (character) =>
    String.fromCharCode(character.charCodeAt(0) + 32)
  );
}

/**
 * One addressable element a source found in its observation. value is the
 * element's current value when the source can read it (never sent to a model;
 * tasks summarize it). handle is source-private: a page ref for the browser
 * source, a VisualRegion for the visual source.
 */
export type Control<Handle = unknown> = Readonly<{
  source: SourceKind;
  role: string;
  name: string;
  value: unknown;
  handle: Handle;
}>;

/**
 * The interface every candidate source implements. find returns the unique
 * control matching a role and accessible name, or undefined. click and
 * typeText return a fully specified candidate for that control, or undefined
 * when this source cannot perform the action in the current observation (for
 * example a visual region cannot receive text, and a visual click needs a
 * capture-bound Driver click).
 */
export interface CandidateSource {
  readonly kind: SourceKind;
  find(role: string, name: string): Control | undefined;
  click(control: Control, candidateId: string, description: string): Candidate | undefined;
  typeText(
    control: Control,
    text: string,
    candidateId: string,
    description: string
  ): Candidate | undefined;
}

/** Controls from a get_browser_state snapshot, acted on through its refs. */
export class BrowserSemanticSource implements CandidateSource {
  readonly kind = 'page' as const;

  constructor(readonly snapshot: BrowserSnapshot) {}

  /** The browser target every page candidate addresses. */
  target() {
    return { target_id: this.snapshot.target_id, tab_id: this.snapshot.tab_id };
  }

  find(role: string, name: string): Control<PageRef> | undefined {
    const ref = (this.snapshot.refs ?? []).find(
      (item) => item.role === role && item.name === name && item.ref
    );
    if (!ref) return undefined;
    return { source: 'page', role, name, value: ref.value, handle: ref };
  }

  click(control: Control, candidateId: string, description: string): Candidate {
    return immutableCandidate({
      id: candidateId,
      description,
      tool: 'browser_click',
      arguments: {
        ...this.target(),
        ref: (control.handle as PageRef).ref,
        input_route: 'dom_event',
      },
    });
  }

  typeText(control: Control, text: string, candidateId: string, description: string): Candidate {
    return immutableCandidate({
      id: candidateId,
      description,
      tool: 'browser_type',
      arguments: {
        ...this.target(),
        ref: (control.handle as PageRef).ref,
        text,
        replace: true,
      },
    });
  }
}

/**
 * Controls from validated OmniParser regions, clicked through their capture.
 * Regions carry no role, so find matches the visible text or icon label (ASCII
 * case-insensitive) of regions at or above MIN_CONFIDENCE and returns a
 * control only when exactly one region matches. click is offered only when
 * Driver advertises a capture-bound click (captureBound); it targets the
 * region's screenshot-pixel center with the exact capture_id and the source's
 * delivery mode.
 */
export class VisualRegionSource implements CandidateSource {
  static readonly MIN_CONFIDENCE = 0.8;
  readonly kind = 'visual' as const;

  constructor(
    readonly observation: VisualObservation,
    readonly delivery: VisualDelivery = 'background',
    readonly captureBound = false
  ) {}

  find(role: string, name: string): Control<VisualRegion> | undefined {
    const wanted = asciiLower(name);
    const matches = this.observation.regions.filter(
      (region) =>
        region.confidence >= VisualRegionSource.MIN_CONFIDENCE &&
        asciiLower(region.text ?? region.label ?? '') === wanted
    );
    if (matches.length !== 1) return undefined;
    const region = matches[0];
    return {
      source: 'visual',
      role,
      name: region.text ?? region.label ?? '',
      value: undefined,
      handle: region,
    };
  }

  click(control: Control, candidateId: string, description: string): Candidate | undefined {
    if (!this.captureBound) return undefined;
    const visual = this.observation;
    const region = control.handle as VisualRegion;
    return immutableCandidate({
      id: candidateId,
      description,
      tool: 'click',
      arguments: {
        pid: visual.pid,
        window_id: visual.windowId,
        x: region.x + region.width / 2,
        y: region.y + region.height / 2,
        capture_id: visual.captureId,
        delivery_mode: this.delivery,
      },
      captureId: visual.captureId,
      screenshotReference: visual.screenshotReference,
    });
  }

  typeText(): undefined {
    return undefined;
  }
}
