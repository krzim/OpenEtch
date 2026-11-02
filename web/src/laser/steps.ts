// Beginner Mode's step bar (owner decision 10, docs/ux/ui-plan-2026-11.md P5: the step bar only in this pass):
// 1 Preview -> 2 Frame -> 3 Start, highlighting the next step. Pure, for tests.

export type StepId = "preview" | "frame" | "start";
export const STEPS: readonly StepId[] = ["preview", "frame", "start"];

export interface StepInput {
  /** The preview has been computed for this document (its estimate is known). */
  previewed: boolean;
  /** The red pointer frames now. */
  framing: boolean;
  /** The revision last framed from this page, and the document's revision now. */
  framedRevision: number | null;
  revision: number | null;
  /** A job runs. */
  running: boolean;
}

export interface StepState {
  done: Record<StepId, boolean>;
  /** The step to do next ("burning" while a job runs). */
  current: StepId | "burning";
}

export function beginnerSteps(i: StepInput): StepState {
  const framed = i.framing || (i.framedRevision !== null && i.framedRevision === i.revision);
  const done = { preview: i.previewed, frame: framed, start: i.running };
  if (i.running) return { done, current: "burning" };
  const current: StepId = !i.previewed ? "preview" : !framed ? "frame" : "start";
  return { done, current };
}
