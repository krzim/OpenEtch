// Laser workspace side panel: machine status, runs under way, pending runs with a deliberate two-step approval, recent
// runs, and preview stats. Approval is the only way the UI can make the laser fire: it needs two separate taps plus the
// operator PIN (set on the engine machine with `openetch set-pin`), shows what will fire, and is refused for stale runs,
// a changed PIN file and a PIN lockout. The PIN lives only in this component's state while the confirm step is open and
// goes only into the approve request body.

import { useEffect, useRef, useState } from "react";
import { api, ApiError } from "../api/client.ts";
import {
  activeRuns,
  approvalErrorText,
  approvalNotice,
  approveBlocker,
  isStale,
  lockSecondsLeft,
  pendingRuns,
  pinHint,
  recentRuns,
  runStatRows,
  stateLabel,
  validPin,
} from "../api/runs.ts";
import type { MachineStatus, PendingRun, Preview } from "../api/types.ts";
import { originBadge } from "./Timeline.tsx";

export function MachineCard({ machine, httpError }: { machine: MachineStatus | null; httpError: string | null }) {
  const known = new Set(["connected", "sim", "busy", "field_mm", "pending_runs", "runs", "approval"]);
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

/** Current time in Unix seconds, re-rendering every second while approvals are locked (for the countdown). */
function useLockClock(machine: MachineStatus | null): number {
  const [now, setNow] = useState(() => Date.now() / 1000);
  const locked = lockSecondsLeft(machine, Date.now() / 1000) > 0;
  useEffect(() => {
    setNow(Date.now() / 1000);
    if (!locked) return;
    const id = window.setInterval(() => setNow(Date.now() / 1000), 1000);
    return () => window.clearInterval(id);
  }, [locked, machine]);
  return now;
}

export function RunsCard(props: {
  machine: MachineStatus | null;
  /** Revision of the document shown; a pending run planned from another one cannot be approved. */
  docRevision: number | null;
  onChanged: () => void;
  report: (e: unknown) => void;
}) {
  const pending = pendingRuns(props.machine);
  const active = activeRuns(props.machine);
  const recent = recentRuns(props.machine);
  const approval = props.machine?.approval;
  const pedal = approval?.physical_confirm ?? null;
  const nowUnixS = useLockClock(props.machine);
  const notice = approvalNotice(props.machine, nowUnixS);
  return (
    <>
      {active.length > 0 && (
        <section className="card">
          <h3>Active run</h3>
          {active.map((r) => (
            <ActiveRunRow key={r.id} run={r} machine={props.machine} />
          ))}
        </section>
      )}
      <section className="card">
        <h3>Pending runs</h3>
        {approval && (
          <p className={notice ? "run-hint" : "muted"} role={notice ? "status" : undefined}>
            {notice ?? `Approval needs the operator PIN${pedal ? `, then the foot pedal (input ${pedal.input_bit})` : ""}.`}
          </p>
        )}
        {pending.length === 0 ? (
          <p className="muted">None. “Request run” creates one; nothing fires until it is approved here.</p>
        ) : (
          pending.map((r) => (
            <PendingRunRow
              key={r.id}
              run={r}
              machine={props.machine}
              docRevision={props.docRevision}
              nowUnixS={nowUnixS}
              notice={notice}
              onChanged={props.onChanged}
              report={props.report}
            />
          ))
        )}
      </section>
      {recent.length > 0 && (
        <section className="card">
          <h3>Recent runs</h3>
          {recent.map((r) => (
            <EndedRunRow key={r.id} run={r} />
          ))}
        </section>
      )}
    </>
  );
}

/** The confirm step reverts to idle after this long without typing. */
const CONFIRM_TIMEOUT_MS = 20_000;

/** Summary, stats, packet count and revision: what this run will do. */
function RunFacts({ run }: { run: PendingRun }) {
  const rows = runStatRows(run);
  return (
    <>
      {run.summary && <div className="run-summary">{String(run.summary)}</div>}
      {rows.length > 0 && (
        <dl className="kv run-stats">
          {rows.map(([k, v]) => (
            <FragmentKV key={k} k={k} v={v} />
          ))}
        </dl>
      )}
    </>
  );
}

/** Run id plus who requested it ("You" or the AI agent's name). */
function RunHead({ run }: { run: PendingRun }) {
  const by = run.origin !== undefined ? originBadge(run.origin) : null;
  return (
    <div className="run-head">
      <span className="run-id">{run.id}</span>
      {by && (
        <span className={`badge ${by.cls}`} title={by.cls === "origin-agent" ? "Requested by an AI agent" : undefined}>
          {by.label}
        </span>
      )}
    </div>
  );
}

function ActiveRunRow({ run, machine }: { run: PendingRun; machine: MachineStatus | null }) {
  const pedal = machine?.approval?.physical_confirm ?? null;
  return (
    <div className="run-row active">
      <RunHead run={run} />
      <div className="run-state">{stateLabel(run.state)}</div>
      {run.state === "awaiting_confirm" && (
        <p className="pedal">
          {pedal
            ? `Press the foot pedal (input ${pedal.input_bit}) within ${pedal.timeout_s} s to start. STOP cancels.`
            : "Waiting for the physical confirmation. STOP cancels."}
        </p>
      )}
      <RunFacts run={run} />
    </div>
  );
}

/** A run that has ended: finished, failed, aborted while streaming, or cancelled before it streamed. */
function EndedRunRow({ run }: { run: PendingRun }) {
  const state = typeof run.state === "string" ? run.state : "";
  return (
    <div className={`run-row ended ended-${state}`}>
      <RunHead run={run} />
      <div className="run-state">
        {stateLabel(run.state)}
        {state === "cancelled" ? " (stopped before it fired)" : ""}
      </div>
      {run.error && <p className="run-error">{String(run.error)}</p>}
      {run.summary && <div className="run-summary">{String(run.summary)}</div>}
    </div>
  );
}

function PendingRunRow(props: {
  run: PendingRun;
  machine: MachineStatus | null;
  docRevision: number | null;
  nowUnixS: number;
  /** Machine-wide blocker already shown on the card (not repeated per run). */
  notice: string | null;
  onChanged: () => void;
  report: (e: unknown) => void;
}) {
  const { run, machine, onChanged, report } = props;
  const [step, setStep] = useState<"idle" | "confirm" | "sending">("idle");
  const [pin, setPin] = useState("");
  const [error, setError] = useState<string | null>(null);
  const pinRef = useRef<HTMLInputElement>(null);
  const blocker = approveBlocker(run, machine, { docRevision: props.docRevision, nowUnixS: props.nowUnixS });
  const stale = isStale(run, props.docRevision);
  const hint = pinHint(pin);

  // Leaving the confirm step forgets the PIN.
  useEffect(() => {
    if (step === "idle") setPin("");
    if (step === "confirm") pinRef.current?.focus();
  }, [step]);
  // Revert after a while without typing.
  useEffect(() => {
    if (step !== "confirm") return;
    const t = window.setTimeout(() => setStep("idle"), CONFIRM_TIMEOUT_MS);
    return () => window.clearTimeout(t);
  }, [step, pin]);
  // A run that goes stale (or a PIN that gets unset) while confirming cannot be approved any more.
  useEffect(() => {
    if (blocker && step === "confirm") setStep("idle");
  }, [blocker, step]);

  const canFire = step === "confirm" && !blocker && validPin(pin);
  const approve = async () => {
    if (!canFire) return;
    const sent = pin;
    setPin("");
    setStep("sending");
    setError(null);
    try {
      await api.approveRun(run.id, sent);
      setStep("idle");
    } catch (e) {
      const code = e instanceof ApiError ? e.code : undefined;
      const detail = e instanceof ApiError && e.detail ? e.detail : e instanceof Error ? e.message : String(e);
      const text = approvalErrorText(code, detail);
      setError(text);
      // A mistyped PIN may be retried straight away; anything else goes back to the start.
      setStep(code === "pin_invalid" || code === "pin_required" ? "confirm" : "idle");
      report(new Error(`Approval refused: ${text}`));
    } finally {
      onChanged();
    }
  };

  return (
    <div className={`run-row${step !== "idle" ? " arming" : ""}${stale ? " stale" : ""}`}>
      <RunHead run={run} />
      <RunFacts run={run} />
      {blocker && blocker !== props.notice && <p className="run-hint">{blocker}</p>}
      {error && <p className="run-error">{error}</p>}
      {step === "idle" && (
        <button
          className="btn btn-warn"
          onClick={() => {
            setError(null);
            setStep("confirm");
          }}
          disabled={!!blocker}
        >
          Approve…
        </button>
      )}
      {step !== "idle" && (
        <div className="confirm">
          <p>
            <strong>This fires the laser.</strong> Enclosure closed, eyewear on, focus and stock checked?
          </p>
          <label className="pin-field">
            <span>Operator PIN</span>
            <input
              ref={pinRef}
              type="password"
              inputMode="numeric"
              pattern="[0-9]*"
              minLength={6}
              maxLength={12}
              placeholder="6-12 digits"
              aria-describedby={hint ? `pin-hint-${run.id}` : undefined}
              autoComplete="off"
              value={pin}
              disabled={step === "sending"}
              onChange={(e) => setPin(e.target.value.replace(/\D/g, "").slice(0, 12))}
              onKeyDown={(e) => {
                if (e.key === "Enter") void approve();
                else if (e.key === "Escape") setStep("idle");
              }}
            />
          </label>
          {hint && (
            <p className="run-hint" id={`pin-hint-${run.id}`}>
              {hint}
            </p>
          )}
          <div className="btn-row">
            <button className="btn" onClick={() => setStep("idle")} disabled={step === "sending"}>
              Cancel
            </button>
            <button className="btn btn-danger" onClick={() => void approve()} disabled={!canFire}>
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
