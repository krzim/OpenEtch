// Laser workspace side panel: machine status, runs under way, pending runs with a deliberate two-step approval, recent
// runs, and preview stats. Approval is the only way the UI can make the laser fire: it needs two separate taps plus the
// operator PIN (set on the engine machine with `openetch set-pin`), shows what will fire, and is refused for stale runs,
// a changed PIN file and a PIN lockout. The PIN lives only in this component's state while the confirm step is open and
// goes only into the approve request body.

import { useEffect, useRef, useState, type ReactNode } from "react";
import { api, ApiError } from "../api/client.ts";
import { MACHINE_SHOWN_KEYS, formatDuration, previewLayers, segLen, statRows } from "../api/laser.ts";
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
import type { GDocument, MachineStatus, PendingRun, Preview } from "../api/types.ts";
import type { PreviewOptions } from "./Viewport.tsx";
import { Icon } from "./ui.tsx";
import { originBadge } from "./Timeline.tsx";

/** Machine status; `children` (the LightBurn devices section) render at the bottom of the card. */
export function MachineCard({ machine, httpError, children }: { machine: MachineStatus | null; httpError: string | null; children?: ReactNode }) {
  const extra = machine
    ? Object.entries(machine).filter(([k, v]) => !MACHINE_SHOWN_KEYS.has(k) && v !== null && v !== undefined && typeof v !== "object")
    : [];
  const sim = !!(machine?.simulated ?? machine?.sim);
  const state = typeof machine?.state === "string" ? machine.state : machine?.busy ? "busy" : "idle";
  const stateText = machine?.framing ? "Framing (red pointer)" : machine?.running ? "Running" : state.charAt(0).toUpperCase() + state.slice(1);
  const lastError = typeof machine?.last_error === "string" && machine.last_error ? machine.last_error : null;
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
            {sim ? " (simulator)" : machine.link === "usb" ? " (USB)" : ""}
          </dd>
          <dt>State</dt>
          <dd>{stateText}</dd>
          {typeof machine.profile === "string" && (
            <>
              <dt>Profile</dt>
              <dd>{machine.profile}</dd>
            </>
          )}
          {machine.field_mm !== undefined && (
            <>
              <dt>Field</dt>
              <dd>{machine.field_mm} mm</dd>
            </>
          )}
          {typeof machine.max_power_pct === "number" && (
            <>
              <dt>Power limit</dt>
              <dd>{machine.max_power_pct}%</dd>
            </>
          )}
          {lastError && (
            <>
              <dt>Last error</dt>
              <dd className="run-error">{lastError}</dd>
            </>
          )}
          {extra.map(([k, v]) => (
            <FragmentKV key={k} k={k} v={v} />
          ))}
        </dl>
      )}
      {machine && children}
    </section>
  );
}

function FragmentKV({ k, v, raw }: { k: string; v: unknown; raw?: boolean }) {
  return (
    <>
      <dt>{raw ? k : k.replace(/_/g, " ")}</dt>
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

/**
 * Toolpath card: why there is no preview (the engine's planning error, e.g. objects outside the field), the job time
 * estimate, marks and travel, and one row per layer with a show toggle and solo (display only; output is unchanged),
 * plus the shading and travel display options.
 */
export function PreviewStatsCard(p: {
  preview: Preview | null;
  error: string | null;
  enabled: boolean;
  doc: GDocument | null;
  options: PreviewOptions;
  onOptions: (o: PreviewOptions) => void;
}) {
  const { preview, options, onOptions } = p;
  if (!p.enabled) return null;
  const segs = preview?.segments ?? [];
  let markN = 0;
  let markLen = 0;
  let jumpLen = 0;
  for (const s of segs) {
    if (s.kind === "mark") {
      markN++;
      markLen += segLen(s);
    } else jumpLen += segLen(s);
  }
  const layers = previewLayers(preview, p.doc);
  const time = typeof preview?.stats?.estimated_time_s === "number" ? preview.stats.estimated_time_s : null;
  const rows = statRows(preview?.stats, ["estimated_time_s", "mark_length_mm", "jump_length_mm", "segment_count"]);
  const toggle = (id: string) => {
    const hidden = new Set(options.hidden);
    if (hidden.has(id)) hidden.delete(id);
    else hidden.add(id);
    onOptions({ ...options, hidden });
  };
  const solo = (id: string) => {
    const only = layers.every((l) => l.id === id || options.hidden.has(l.id)) && !options.hidden.has(id);
    onOptions({ ...options, hidden: only ? new Set() : new Set(layers.filter((l) => l.id !== id).map((l) => l.id)) });
  };
  return (
    <section className="card">
      <h3>Toolpath</h3>
      {p.error ? (
        <p className="run-error" role="alert">
          No preview: {p.error}
        </p>
      ) : !preview ? (
        <p className="muted">Nothing to preview yet.</p>
      ) : (
        <>
          {time !== null && <div className="big-stat">{formatDuration(time)}</div>}
          <dl className="kv">
            <dt>Marks</dt>
            <dd>
              {markN} · {(typeof preview.stats?.mark_length_mm === "number" ? preview.stats.mark_length_mm : markLen).toFixed(1)} mm
            </dd>
            <dt>Travel</dt>
            <dd>
              {segs.length - markN} · {(typeof preview.stats?.jump_length_mm === "number" ? preview.stats.jump_length_mm : jumpLen).toFixed(1)} mm
            </dd>
            {rows.map(([k, v]) => (
              <FragmentKV key={k} k={k} v={v} raw />
            ))}
          </dl>
          {(preview.warnings ?? []).map((w, i) => (
            <p key={i} className="warn-text">
              {w}
            </p>
          ))}
          {layers.length > 0 && (
            <div className="pv-layers">
              {layers.map((l) => (
                <div key={l.id} className={`pv-layer${options.hidden.has(l.id) ? " off" : ""}`}>
                  <label title="Show this layer's toolpath (display only)">
                    <input type="checkbox" checked={!options.hidden.has(l.id)} onChange={() => toggle(l.id)} />
                    <span className="swatch small" style={{ background: l.color }} />
                    <span className="pv-name">{l.name}</span>
                  </label>
                  <span className="pv-meta">
                    {l.mode === "fill" ? "Fill" : l.mode === "fill_then_line" ? "Fill+Line" : "Line"} · {l.markLengthMm.toFixed(0)} mm
                    {l.estimatedTimeS !== null ? ` · ${formatDuration(l.estimatedTimeS)}` : ""}
                  </span>
                  <button className="chip" title="Show only this layer" onClick={() => solo(l.id)}>
                    Solo
                  </button>
                </div>
              ))}
            </div>
          )}
          <div className="pv-opts">
            <label>
              <input type="checkbox" checked={options.shade} onChange={(e) => onOptions({ ...options, shade: e.target.checked })} /> Shade by energy
            </label>
            <label>
              <input type="checkbox" checked={options.jumps} onChange={(e) => onOptions({ ...options, jumps: e.target.checked })} /> Show travel
            </label>
          </div>
        </>
      )}
    </section>
  );
}

/** Draw-order animation: a slider over the job's segments with play/pause, like LightBurn's Preview time slider. */
export function PreviewPlayer({ preview, options, onOptions }: { preview: Preview; options: PreviewOptions; onOptions: (o: PreviewOptions) => void }) {
  const total = preview.segments?.length ?? 0;
  const [playing, setPlaying] = useState(false);
  const [speed, setSpeed] = useState(1);
  const at = options.upTo ?? total;
  const optsRef = useRef(options);
  optsRef.current = options;
  useEffect(() => {
    if (!playing || total === 0) return;
    let raf = 0;
    let last = performance.now();
    // About 8 seconds for the whole job at 1x.
    const perMs = (total / 8000) * speed;
    let pos = optsRef.current.upTo === null || optsRef.current.upTo >= total ? 0 : optsRef.current.upTo;
    const step = (t: number) => {
      pos += (t - last) * perMs;
      last = t;
      if (pos >= total) {
        onOptions({ ...optsRef.current, upTo: null });
        setPlaying(false);
        return;
      }
      onOptions({ ...optsRef.current, upTo: Math.floor(pos) });
      raf = requestAnimationFrame(step);
    };
    raf = requestAnimationFrame(step);
    return () => cancelAnimationFrame(raf);
  }, [playing, total, speed, onOptions]);
  if (total === 0) return null;
  return (
    <div className="pv-player" role="group" aria-label="Draw order">
      <button className="icon-btn small" title={playing ? "Pause" : "Play the burn order"} aria-label={playing ? "Pause" : "Play"} onClick={() => setPlaying((v) => !v)}>
        <Icon name={playing ? "pause" : "play"} size={16} />
      </button>
      <input
        type="range"
        min={0}
        max={total}
        value={at}
        aria-label="Segments shown"
        onChange={(e) => {
          setPlaying(false);
          const v = Number(e.target.value);
          onOptions({ ...options, upTo: v >= total ? null : v });
        }}
      />
      <select value={speed} onChange={(e) => setSpeed(Number(e.target.value))} aria-label="Playback speed" title="Playback speed">
        {[0.25, 0.5, 1, 2, 5, 10, 40].map((s) => (
          <option key={s} value={s}>
            {s}x
          </option>
        ))}
      </select>
      <span className="pv-count">
        {at}/{total}
      </span>
    </div>
  );
}
