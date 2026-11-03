// Messages between the page and the 3D relief worker (reliefWorker.ts).

import type { ReliefField, ReliefInput } from "./reliefModel.ts";

export type ToRelief =
  /** The job's arrays and which passes to draw (copied into the worker once per preview or option change). */
  | { type: "load"; input: ReliefInput; include: boolean[]; maxSide: number }
  /** Build the field up to segment `upTo` (null: the whole job). */
  | { type: "build"; id: number; upTo: number | null };

export type FromRelief =
  | { type: "field"; id: number; field: ReliefField | null; ms: number }
  | { type: "error"; id: number; error: string };
