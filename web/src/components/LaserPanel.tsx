// Laser workspace side panel: machine status, pending runs with a deliberate two-step approval, and preview stats.
// Approval is the only way the UI can make the laser fire, so it needs two separate taps and times out.

import { useEffect, useState } from "react";
import { api } from "../api/client.ts";
import type { MachineStatus, PendingRun, Preview } from "../api/types.ts";
import { originBadge } from "./Timeline.tsx";

/** Pending runs from the machine status: `pending_runs`, or `runs` filtered to state "pending". */
export function pendingRuns(m: MachineStatus | null): PendingRun[] {
  if (!m) return [];
  if (Array.isArray(m.pending_runs)) return m.pending_runs;
  const runs = (m as { runs?: unknown }).runs;
  if (Array.isArray(runs)) return (runs as PendingRun[]).filter((r) => r && (r.state === undefined || r.state === "pending"));
  return [];
}

export function MachineCard({ machine, httpError }: { machine: MachineStatus | null; httpError: string | null }) {
  const known = new Set(["connected", "sim", "busy", "field_mm", "pending_runs", "runs"]);
  const extra = machine ? Object.entries(machine).filter(([k, v]) => !known.has(k) && (typeof v !== "object" || v === null)) : [];
  return (
    <section className="card">
      <h3>Machine</h3>
      {!machine ? (
        <p className="muted">{httpError ? "Engine unreachable." : "Loading…"}</p>
      ) : (
        <dl className="kv">
          <dt>Link</dt>
          <dd>
            <span className={`dot ${machine.connected ? "ok" : "bad"}`} /> {machine.connected ? "Connected" : "Not connected"}
            {machine.sim ? " (simulator)" : ""}
          </dd>
          <dt>State</dt>
          <dd>{machine.busy ? "Busy" : "Idle"}</dd>
          {machine.field_mm !== undefined && (
            <>
              <dt>Field</dt>
              <dd>{machine.field_mm} mm</dd>
            </>
          )}
          {extra.map(([k, v]) => (
            <FragmentKV key={k} k={k} v={v} />
          ))}
        </dl>
      )}
    </section>
  );
}

function FragmentKV({ k, v }: { k: string; v: unknown }) {
  return (
    <>
      <dt>{k.replace(/_/g, " ")}</dt>
      <dd>{String(v)}</dd>
    </>
  );
}

export function RunsCard(props: { machine: MachineStatus | null; onChanged: () => void; report: (e: unknown) => void }) {
  const runs = pendingRuns(props.machine);
  return (
    <section className="card">
      <h3>Pending runs</h3>
      {runs.length === 0 ? (
        <p className="muted">None. “Request run” creates one; nothing fires until it is approved here.</p>
      ) : (
        runs.map((r) => <PendingRunRow key={r.id} run={r} onChanged={props.onChanged} report={props.report} />)
      )}
    </section>
  );
}

const CONFIRM_TIMEOUT_MS = 8000;

function PendingRunRow({ run, onChanged, report }: { run: PendingRun; onChanged: () => void; report: (e: unknown) => void }) {
  const [step, setStep] = useState<"idle" | "confirm" | "sending">("idle");
  useEffect(() => {
    if (step !== "confirm") return;
    const t = window.setTimeout(() => setStep("idle"), CONFIRM_TIMEOUT_MS);
    return () => window.clearTimeout(t);
  }, [step]);
  const by = run.origin !== undefined ? originBadge(run.origin) : null;
  const approve = async () => {
    setStep("sending");
    try {
      await api.approveRun(run.id);
    } catch (e) {
      report(e);
    } finally {
      setStep("idle");
      onChanged();
    }
  };
  return (
    <div className={`run-row${step === "confirm" ? " arming" : ""}`}>
      <div className="run-head">
        <span className="run-id">{run.id}</span>
        {by && <span className={`badge ${by.cls}`}>{by.label}</span>}
      </div>
      {run.summary && <div className="run-summary">{String(run.summary)}</div>}
      {step === "idle" && (
        <button className="btn btn-warn" onClick={() => setStep("confirm")}>
          Approve…
        </button>
      )}
      {step !== "idle" && (
        <div className="confirm">
          <p>
            <strong>This fires the laser.</strong> Enclosure closed, eyewear on, focus and stock checked?
          </p>
          <div className="btn-row">
            <button className="btn" onClick={() => setStep("idle")} disabled={step === "sending"}>
              Cancel
            </button>
            <button className="btn btn-danger" onClick={approve} disabled={step === "sending"}>
              {step === "sending" ? "Approving…" : "Confirm and fire"}
            </button>
          </div>
        </div>
      )}
    </div>
  );
}

export function PreviewStatsCard({ preview, enabled }: { preview: Preview | null; enabled: boolean }) {
  if (!enabled) return null;
  const segs = preview?.segments ?? [];
  const marks = segs.filter((s) => s.kind === "mark");
  const len = (k: "mark" | "jump") =>
    segs.filter((s) => s.kind === k).reduce((a, s) => a + Math.hypot(s.to[0] - s.from[0], s.to[1] - s.from[1]), 0);
  return (
    <section className="card">
      <h3>Toolpath</h3>
      {!preview ? (
        <p className="muted">No preview.</p>
      ) : (
        <dl className="kv">
          <dt>Marks</dt>
          <dd>
            {marks.length} · {len("mark").toFixed(1)} mm
          </dd>
          <dt>Jumps</dt>
          <dd>
            {segs.length - marks.length} · {len("jump").toFixed(1)} mm
          </dd>
          {Object.entries(preview.stats ?? {})
            .filter(([, v]) => typeof v !== "object" || v === null)
            .map(([k, v]) => (
              <FragmentKV key={k} k={k} v={typeof v === "number" ? Number(v.toFixed(3)) : v} />
            ))}
        </dl>
      )}
    </section>
  );
}
