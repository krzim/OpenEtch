// The request behind every Start (moved here from components/JobControls.tsx, which re-exports it, so the pure
// laser/startFlow.ts can call it without importing a component). With the operator PIN, "Request run" creates a
// pending run that still needs the PIN; without one (`--approval click`), Start plans and burns the job in one click,
// like LightBurn's Start, and the engine accepts that only from this page in a browser. Either way approve.rs arms the
// run: nothing here can bypass it.

import { ApiError } from "../api/client.ts";
import { runRequestBody } from "../api/job.ts";
import { jobApi } from "../api/jobApi.ts";
import { approvalErrorText } from "../api/runs.ts";
import { getRunOptions } from "../components/jobStore.ts";
import { approvalRefusalText } from "../libs/model.ts";
import { getPrefs } from "../workspace/prefs.ts";
import { tr } from "../workspace/tr.ts";

/**
 * Request a run with the Laser window's options (and the selection for Cut Selected Graphics). `revision`: the
 * document revision this page shows (for Start Here, the revision the preview and so the segment number are of); the
 * engine plans nothing when the document is at another one, so a stale segment never starts a newer document.
 */
export async function requestRunWithOptions(selection: readonly string[], fromSegment: number | null = null, revision: number | null = null): Promise<string> {
  // Beginner Mode hides Cut Selected Graphics and ignores it (LightBurn ignores hidden optional settings).
  const o = getRunOptions();
  const body = runRequestBody(fromSegment === null && !getPrefs().beginnerMode ? o : { ...o, cutSelected: false }, { selection, fromSegment });
  await jobApi.requestRun(revision === null ? body : { ...body, revision });
  return fromSegment === null ? tr("rest.runRequested") : tr("rest.runFromRequested", { n: fromSegment });
}

/**
 * Start (no-PIN engine: plan and burn now, with the Job Checklist ticks when it is on) or Request run (PIN engine: a
 * pending run to approve with the PIN), with the Laser window's options. Resolves to the toast text; a refused Start
 * throws an Error with a readable reason (door open, checklist, Require Framing, ...). `revision`: the document
 * revision this page shows; the engine starts nothing when the document is at another one (an agent's edit came in
 * just before the click), so Start burns only what the person was looking at.
 */
export async function runJob(
  noPin: boolean,
  selection: readonly string[],
  fromSegment: number | null = null,
  checklist: boolean[] | null = null,
  revision: number | null = null,
): Promise<string> {
  if (!noPin) return requestRunWithOptions(selection, fromSegment, revision);
  const o = getRunOptions();
  const body = runRequestBody(fromSegment === null && !getPrefs().beginnerMode ? o : { ...o, cutSelected: false }, { selection, fromSegment });
  try {
    await jobApi.start({ ...body, ...(checklist ? { checklist } : {}), ...(revision === null ? {} : { revision }) });
  } catch (e) {
    if (!(e instanceof ApiError)) throw e;
    const detail = e.detail || e.message;
    throw new Error(approvalErrorText(e.code, approvalRefusalText(detail) ?? detail));
  }
  return fromSegment === null ? tr("rest.jobStarted") : tr("rest.jobStartedFrom", { n: fromSegment });
}
