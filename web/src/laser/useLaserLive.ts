// The P5 components' view of the laser (laser/laserContext.ts), plus the self poll: while at least one of them is
// mounted and no source reports the engine link (the shell has not published yet), GET /api/machine every 2 s and on
// machine events, so the chip, the banner and Start stay current on their own. A failed poll marks the link closed,
// which shows the connection banner within one poll.

import { useEffect } from "react";
import { api } from "../api/client.ts";
import { onEngineEvent } from "../api/events.ts";
import { getLaserFacts, linkPublished, setLaserSource, useLaserFacts, useLaserFlow, type LaserFacts, type LaserFlow } from "./laserContext.ts";

const POLL_MS = 2000;
let users = 0;
let timer: number | undefined;
let unlisten: (() => void) | null = null;
let inFlight = false;

async function poll(): Promise<void> {
  if (linkPublished()) {
    // The shell or the dock reports the link; the self source steps back (its machine would be stale).
    setLaserSource("self", null);
    return;
  }
  if (inFlight) return;
  inFlight = true;
  try {
    const machine = await api.machine();
    if (!linkPublished()) setLaserSource("self", { machine, link: "open", httpError: null });
  } catch (e) {
    if (!linkPublished()) {
      setLaserSource("self", { machine: getLaserFacts().machine, link: "closed", httpError: e instanceof Error ? e.message : String(e) });
    }
  } finally {
    inFlight = false;
  }
}

function start(): void {
  void poll();
  timer = window.setInterval(() => void poll(), POLL_MS);
  unlisten = onEngineEvent((ev) => {
    if (ev.type === "machine" || ev.type === "run_finished" || ev.type === "run_pending") void poll();
  });
}

function stop(): void {
  window.clearInterval(timer);
  timer = undefined;
  unlisten?.();
  unlisten = null;
}

/** The laser facts and the Start flow state, keeping them current while the caller is mounted. */
export function useLaserLive(): { facts: LaserFacts; flow: LaserFlow } {
  const facts = useLaserFacts();
  const flow = useLaserFlow();
  useEffect(() => {
    if (users++ === 0) start();
    return () => {
      if (--users === 0) stop();
    };
  }, []);
  return { facts, flow };
}
