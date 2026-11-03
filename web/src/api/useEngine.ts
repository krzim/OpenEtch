// The engine connection as a React hook: fetches the document, history, machine status, project and preview, listens on the
// /api/events WebSocket, and exposes `run` for edits. Edits are commands; the document follows the engine's change sets
// (docSync.ts): an edit's answer carries the changes from the revision held, a `document_changed` event from elsewhere
// fetches them (GET /api/document/sync), and the whole document is fetched only on first load, after a gap or when
// another document becomes active. Engines without change sets get the old whole-document refetch.

import { useCallback, useEffect, useRef, useState } from "react";
import { api, ApiError, batch, eventsUrl, execute, type Invocation } from "./client.ts";
import { publishEngineEvent, type EngineMessage } from "./events.ts";
import { EMPTY_TIMELINE, normalizeHistory, type TimelineModel } from "./history.ts";
import { changesPreviewContext, footSwitchToast, runFinishedToast, withApprovalLock } from "./runs.ts";
import type { CommandSpec, EngineEvent, GDocument, MachineStatus, Origin, Preview, ProjectInfo } from "./types.ts";
import { mutationsInFlight, onSyncPayload, setSyncPoint, syncStep, type SyncPayload } from "./docSync.ts";
import type { PreviewSummary } from "./packedPreview.ts";
import { useBrowserPreview } from "../wasm/useBrowserPreview.ts";

/** WebSocket state shown by the connection indicator. */
export type LinkState = "connecting" | "open" | "closed";

export interface Engine {
  doc: GDocument | null;
  /** The timeline (steps, states and marker). */
  timeline: TimelineModel;
  machine: MachineStatus | null;
  preview: Preview | null;
  /** Why the engine could not plan a preview (e.g. objects outside the field), or null. */
  previewError: string | null;
  /** The job's summary (time estimate...) planned in the browser, in either workspace; null without the worker. */
  previewSummary: PreviewSummary | null;
  /** Where the preview comes from: the browser's worker, or the engine (GET /api/preview). */
  previewSource: "browser" | "engine";
  commands: CommandSpec[] | null;
  /** Whether the engine has a command (false while the list is loading). */
  has: (id: string) => boolean;
  /** GET /api/project, or null until it answers (or when the engine has no project routes). */
  project: ProjectInfo | null;
  /** GET /api/project has answered at least once (with a project, or with an error on engines without the route). */
  projectKnown: boolean;
  link: LinkState;
  /** Last HTTP error talking to the engine, cleared on the next success. */
  httpError: string | null;
  /** The latest edit made by someone other than this UI (from `document_changed.origin`), for the timeline to flash. */
  remoteEdit: RemoteEdit | null;
  /** Counts `device_changed` events (another device became active), so the device list refetches. */
  deviceSeq: number;
  /** Run one command (or several as one undo step) and refetch. Resolves to the result, or throws ApiError. */
  run: (inv: Invocation | Invocation[]) => Promise<unknown>;
  undo: () => Promise<void>;
  redo: () => Promise<void>;
  /** Roll the timeline marker (newer engines). */
  historyGoto: (marker: number) => Promise<void>;
  /** Delete a timeline step and replay the later ones (newer engines). */
  historyDelete: (index: number) => Promise<void>;
  refreshDoc: () => Promise<void>;
  refreshMachine: () => Promise<void>;
  refreshPreview: () => Promise<void>;
  refreshProject: () => Promise<void>;
}

/** An edit announced by `document_changed` with a non-UI origin. `seq` changes on every event so repeats re-flash. */
export interface RemoteEdit {
  revision: number;
  origin: Origin;
  seq: number;
}

/** True for origins that are not this UI (agents and scripts). */
function isRemoteOrigin(o: Origin | null | undefined): o is Origin {
  if (o === undefined || o === null) return false;
  return typeof o === "string" ? o !== "ui" : o.kind !== "ui";
}

export interface EngineOptions {
  /** Keep the preview current (planned in the browser, or fetched from the engine as a fallback). */
  wantPreview: boolean;
  /** Called for asynchronous problems and notable events (e.g. a run became pending). */
  notify: (msg: string, kind?: "info" | "error") => void;
}

/** Coalesce concurrent calls: at most one request in flight, plus one queued rerun. */
function useCoalesced(fn: () => Promise<void>): () => Promise<void> {
  const fnRef = useRef(fn);
  fnRef.current = fn;
  const inFlight = useRef<Promise<void> | null>(null);
  const again = useRef(false);
  return useCallback(() => {
    if (inFlight.current) {
      again.current = true;
      return inFlight.current;
    }
    const loop = async () => {
      do {
        again.current = false;
        await fnRef.current();
      } while (again.current);
    };
    inFlight.current = loop().finally(() => {
      inFlight.current = null;
    });
    return inFlight.current;
  }, []);
}

export function useEngine({ wantPreview, notify }: EngineOptions): Engine {
  const [doc, setDoc] = useState<GDocument | null>(null);
  const [timeline, setTimeline] = useState<TimelineModel>(EMPTY_TIMELINE);
  const [machine, setMachine] = useState<MachineStatus | null>(null);
  const [preview, setPreview] = useState<Preview | null>(null);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [commands, setCommands] = useState<CommandSpec[] | null>(null);
  const [project, setProject] = useState<ProjectInfo | null>(null);
  const [projectKnown, setProjectKnown] = useState(false);
  const [link, setLink] = useState<LinkState>("connecting");
  const [httpError, setHttpError] = useState<string | null>(null);
  const [remoteEdit, setRemoteEdit] = useState<RemoteEdit | null>(null);
  const [deviceSeq, setDeviceSeq] = useState(0);

  const notifyRef = useRef(notify);
  notifyRef.current = notify;
  const wantPreviewRef = useRef(wantPreview);
  wantPreviewRef.current = wantPreview;

  // ---- browser preview (src/wasm/): the preview and its stats are planned in a Web Worker from `doc`; GET
  // /api/preview only in the "fallback" mode (no WebAssembly, or an engine without /api/preview/context).
  const [contextSeq, setContextSeq] = useState(0);
  const browser = useBrowserPreview({ doc, wantPreview, wantStats: true, contextSeq });
  const engineMode = browser.mode === "fallback";
  const engineModeRef = useRef(engineMode);
  engineModeRef.current = engineMode;
  // ---- end browser preview

  const fail = useCallback((e: unknown) => {
    setHttpError(e instanceof Error ? e.message : String(e));
  }, []);

  // --- document sync state (docSync.ts) ---
  /** The document held, ahead of React state (payloads are applied as they arrive). */
  const docRef = useRef<GDocument | null>(null);
  /** The engine run `docRef`'s revision belongs to. */
  const epochRef = useRef<string | null>(null);
  /** Whether the engine has GET /api/document/sync (null until the first answer). */
  const syncSupported = useRef<boolean | null>(null);
  /** Counts sync payloads applied (an edit's answer that brought one needs no refetch). */
  const payloads = useRef(0);
  /** The newest revision an event announced while an edit was in flight. */
  const pendingRevision = useRef(-1);
  /** The newest revision any event announced (this connection). */
  const announced = useRef(-1);
  /**
   * While GET /api/document/sync is in flight: the newest revision announced when it was sent. Its answer is of that
   * revision or a later one, so an event up to it (the `history_changed` after a `document_changed`) needs no fetch.
   */
  const syncCovers = useRef<number | null>(null);
  /** The revision the shown preview is of (-1: unknown, fetch again). */
  const previewRevision = useRef(-1);
  /** Whether the engine has GET /api/preview.bin (null until the first answer). */
  const binaryPreview = useRef<boolean | null>(null);

  /**
   * The engine's preview (the compact binary GET /api/preview.bin, JSON on older engines), only while the browser
   * cannot plan it (`engineMode`); otherwise the worker plans it from the synced document (useBrowserPreview).
   */
  const fetchPreview = useCoalesced(async () => {
    if (!engineModeRef.current) return;
    const revision = docRef.current?.revision ?? -1;
    if (revision >= 0 && previewRevision.current === revision) return;
    try {
      if (binaryPreview.current !== false) {
        try {
          const b = await api.previewBinary();
          binaryPreview.current = true;
          previewRevision.current = b.revision;
          setPreview(b.revision >= 0 ? { ...b.preview, revision: b.revision } : b.preview);
          setPreviewError(null);
          return;
        } catch (e) {
          if (!(e instanceof ApiError && e.status === 404 && binaryPreview.current === null)) throw e;
          binaryPreview.current = false;
        }
      }
      setPreview(await api.preview());
      previewRevision.current = revision;
      setPreviewError(null);
    } catch (e) {
      previewRevision.current = -1;
      // A document with nothing to output may legitimately have no preview; keep the old one off screen.
      setPreview(null);
      if (e instanceof ApiError && e.status === 422) {
        // A planning error (out of field, power above the limit...): the reason belongs on the Toolpath card, not
        // in the connection indicator, and the next status poll must not wipe it.
        setPreviewError(e.detail ?? e.message);
      } else {
        setPreviewError(null);
        if (!(e instanceof ApiError && e.status === 404)) fail(e);
      }
    }
  });

  /** Fetch the preview even if one of this revision is shown (callers that know something else changed). */
  const refreshPreview = useCallback(() => {
    previewRevision.current = -1;
    return fetchPreview();
  }, [fetchPreview]);

  const refreshProject = useCoalesced(async () => {
    try {
      const p = await api.project();
      setProject(p && typeof p === "object" && typeof p.dirty === "boolean" ? p : null);
    } catch (e) {
      // An engine without project routes answers 404: no indicator, and the File menu falls back to its own checks.
      setProject(null);
      if (!(e instanceof ApiError && e.status === 404)) fail(e);
    } finally {
      setProjectKnown(true);
    }
  });

  /** Take a sync payload (an edit's answer or GET /api/document/sync); "gap" when it does not apply to what is held. */
  const acceptSync = useCallback((p: SyncPayload): "ok" | "gap" => {
    const step = syncStep(docRef.current, epochRef.current, p);
    if (step.kind === "gap") return "gap";
    payloads.current++;
    if (step.kind === "replace" || step.kind === "apply") {
      // A whole document may repeat the revision of the preview shown (an engine restarted counts again).
      if (step.kind === "replace") previewRevision.current = -1;
      docRef.current = step.doc;
      epochRef.current = p.epoch;
      setSyncPoint(p.epoch, step.doc.revision);
      setDoc(step.doc);
      if (wantPreviewRef.current) void fetchPreview();
    }
    // The timeline and project status of an older payload (overtaken by a newer one) are stale.
    if (docRef.current && p.revision === docRef.current.revision) {
      setTimeline(normalizeHistory(p.history));
      const pr = p.project as ProjectInfo | null;
      setProject(pr && typeof pr === "object" && typeof pr.dirty === "boolean" ? pr : null);
      setProjectKnown(true);
    }
    setHttpError(null);
    if (pendingRevision.current > (docRef.current?.revision ?? -1) && mutationsInFlight() === 0) {
      pendingRevision.current = -1;
      queueMicrotask(() => void refreshDocRef.current());
    }
    return "ok";
  }, [fetchPreview]);

  const refreshDoc = useCoalesced(async () => {
    if (syncSupported.current !== false) {
      try {
        const held = docRef.current;
        const since = held && epochRef.current ? { epoch: epochRef.current, revision: held.revision } : null;
        syncCovers.current = announced.current;
        if (acceptSync(await api.documentSync(since)) === "gap") acceptSync(await api.documentSync(null));
        syncSupported.current = true;
        return;
      } catch (e) {
        if (!(e instanceof ApiError && e.status === 404 && syncSupported.current === null)) {
          fail(e);
          return;
        }
        syncSupported.current = false;
      } finally {
        syncCovers.current = null;
      }
    }
    // Engines without change sets: the whole document every time.
    try {
      const [d, h] = await Promise.all([api.document(), api.history()]);
      docRef.current = d;
      setDoc(d);
      setTimeline(normalizeHistory(h));
      setHttpError(null);
    } catch (e) {
      fail(e);
      return;
    }
    // The dirty flag follows the revision.
    void refreshProject();
    if (wantPreviewRef.current && engineModeRef.current) void refreshPreview();
  });
  const refreshDocRef = useRef(refreshDoc);
  refreshDocRef.current = refreshDoc;

  // Edits anywhere in the app (client.ts) hand their answers' change sets here.
  useEffect(() => onSyncPayload((p) => {
    if (acceptSync(p) === "gap") void refreshDoc();
  }), [acceptSync, refreshDoc]);

  /** After an edit: fetch the changes unless its answer already brought them (or for an engine without them). */
  const afterEdit = useCallback(
    (before: number) => {
      if (payloads.current === before || syncSupported.current === false) void refreshDoc();
    },
    [refreshDoc],
  );

  const refreshMachine = useCoalesced(async () => {
    try {
      setMachine(await api.machine());
      setHttpError(null);
    } catch (e) {
      fail(e);
    }
  });

  const run = useCallback(
    async (inv: Invocation | Invocation[]) => {
      const before = payloads.current;
      try {
        if (Array.isArray(inv)) return await batch(inv);
        return (await execute(inv)).result;
      } finally {
        // The answer normally carries the changes; otherwise (a failed request, an older engine) fetch them, which
        // also covers a missed WebSocket event.
        afterEdit(before);
      }
    },
    [afterEdit],
  );

  const undo = useCallback(async () => {
    const before = payloads.current;
    try {
      await api.undo();
    } finally {
      afterEdit(before);
    }
  }, [afterEdit]);

  const redo = useCallback(async () => {
    const before = payloads.current;
    try {
      await api.redo();
    } finally {
      afterEdit(before);
    }
  }, [afterEdit]);

  const historyGoto = useCallback(
    async (marker: number) => {
      const before = payloads.current;
      try {
        await api.historyGoto(marker);
      } finally {
        afterEdit(before);
      }
    },
    [afterEdit],
  );

  const historyDelete = useCallback(
    async (index: number) => {
      const before = payloads.current;
      try {
        await api.historyDelete(index);
      } finally {
        afterEdit(before);
      }
    },
    [afterEdit],
  );

  const has = useCallback((id: string) => !!commands && commands.some((c) => c.id === id), [commands]);

  // Initial load.
  useEffect(() => {
    void refreshDoc();
    void refreshMachine();
    api
      .commands()
      .then((c) => setCommands(Array.isArray(c) ? c : []))
      .catch(fail);
  }, [refreshDoc, refreshMachine, fail]);

  // Preview follows the workspace toggle: the engine's (fetched again only when the document changed since) when the
  // browser cannot plan it.
  useEffect(() => {
    if (wantPreview && engineMode) void fetchPreview();
  }, [wantPreview, engineMode, fetchPreview]);

  // Event socket with reconnect and backoff.
  /** The document changed to `revision` (an event): fetch the changes, or wait for an edit in flight to bring them. */
  const docChangedTo = useCallback(
    (revision: unknown) => {
      const held = docRef.current?.revision ?? -1;
      if (typeof revision !== "number" || revision < 0) {
        void refreshDoc();
        return;
      }
      announced.current = Math.max(announced.current, revision);
      if (revision === held) return;
      if (syncCovers.current !== null && revision <= syncCovers.current) return;
      if (mutationsInFlight() > 0 && syncSupported.current) {
        pendingRevision.current = Math.max(pendingRevision.current, revision);
        // Should the edit's answer not cover it (another client's edit landed after it), fetch once it is done.
        window.setTimeout(() => {
          if (pendingRevision.current > (docRef.current?.revision ?? -1) && mutationsInFlight() === 0) {
            pendingRevision.current = -1;
            void refreshDoc();
          }
        }, 1500);
        return;
      }
      void refreshDoc();
    },
    [refreshDoc],
  );
  useEffect(() => {
    let ws: WebSocket | null = null;
    let timer: number | undefined;
    let delay = 500;
    let stopped = false;
    /** Whether the socket was open before (a reconnect refetches the preview context). */
    let opened = false;
    const connect = () => {
      setLink("connecting");
      try {
        ws = new WebSocket(eventsUrl());
      } catch {
        schedule();
        return;
      }
      ws.onopen = () => {
        delay = 500;
        setLink("open");
        // Anything may have changed while we were away (also a restarted engine, whose revisions start again),
        // including the browser preview's planning context (a device change or a finished run while disconnected).
        announced.current = -1;
        if (opened) setContextSeq((n) => n + 1);
        opened = true;
        void refreshDoc();
        void refreshMachine();
      };
      ws.onmessage = (m) => {
        let ev: EngineEvent;
        try {
          ev = JSON.parse(String(m.data)) as EngineEvent;
        } catch {
          return;
        }
        if (ev && typeof ev === "object") publishEngineEvent(ev as EngineMessage);
        switch (ev.type) {
          case "document_changed":
            if (typeof ev.revision !== "number" || ev.revision !== (docRef.current?.revision ?? -1)) docChangedTo(ev.revision);
            else if (syncSupported.current !== true) void refreshProject();
            // An edit makes pending runs stale; fetch their flags now rather than at the next poll.
            void refreshMachine();
            if (typeof ev.revision === "number" && isRemoteOrigin(ev.origin)) {
              const { revision, origin } = ev;
              setRemoteEdit((r) => ({ revision, origin, seq: (r?.seq ?? 0) + 1 }));
            }
            break;
          case "machine": {
            void refreshMachine();
            // A run of a Repeat / Run Continuously job finished (or the counter was reset): the job counter and the
            // learned estimate correction the browser preview plans with moved on mid-job, not only at run_finished.
            if (changesPreviewContext(ev)) setContextSeq((n) => n + 1);
            // The foot switch cycle (no-PIN mode): why a press did not burn, or why the cycle ended.
            const foot = footSwitchToast(ev.event, ev.detail);
            if (foot) notifyRef.current(foot.msg, foot.kind);
            break;
          }
          case "run_pending":
            void refreshMachine();
            // This UI's own requests already say so; only runs requested elsewhere (an agent) get a toast.
            if (isRemoteOrigin(ev.origin as Origin | undefined)) notifyRef.current("A run is waiting for approval in the Laser workspace.", "info");
            break;
          case "approval_locked":
            setMachine((m) => withApprovalLock(m, ev.until_unix_s));
            void refreshMachine();
            break;
          case "project_saved":
            void refreshProject();
            break;
          case "history_changed":
            // The timeline comes with the document's changes; one of the revision held is already shown.
            if (syncSupported.current !== true) void refreshDoc();
            else docChangedTo((ev as { revision?: unknown }).revision);
            break;
          case "device_changed":
            // The profile, field size and controller changed: everything that shows them refetches.
            setDeviceSeq((n) => n + 1);
            setContextSeq((n) => n + 1);
            void refreshMachine();
            previewRevision.current = -1;
            void refreshDoc();
            if (wantPreviewRef.current) void fetchPreview();
            break;
          case "run_finished": {
            void refreshMachine();
            // The job counter and the learned estimate correction the browser preview plans with.
            setContextSeq((n) => n + 1);
            const t = runFinishedToast(ev);
            notifyRef.current(t.msg, t.kind);
            break;
          }
        }
      };
      ws.onclose = () => {
        setLink("closed");
        schedule();
      };
      ws.onerror = () => ws?.close();
    };
    const schedule = () => {
      if (stopped) return;
      timer = window.setTimeout(connect, delay);
      delay = Math.min(delay * 2, 10_000);
    };
    connect();
    return () => {
      stopped = true;
      window.clearTimeout(timer);
      if (ws) {
        ws.onclose = null;
        ws.close();
      }
    };
  }, [refreshDoc, refreshMachine, refreshProject, docChangedTo, fetchPreview]);

  // Poll machine status slowly as a safety net (and faster while the socket is down).
  useEffect(() => {
    const id = window.setInterval(() => void refreshMachine(), link === "open" ? 10_000 : 3_000);
    return () => window.clearInterval(id);
  }, [link, refreshMachine]);

  return {
    doc,
    timeline,
    machine,
    preview: engineMode ? preview : browser.preview,
    previewError: engineMode ? previewError : browser.previewError,
    previewSummary: browser.summary,
    previewSource: engineMode ? "engine" : "browser",
    commands,
    has,
    project,
    projectKnown,
    link,
    httpError,
    remoteEdit,
    deviceSeq,
    run,
    undo,
    redo,
    historyGoto,
    historyDelete,
    refreshDoc,
    refreshMachine,
    refreshPreview,
    refreshProject,
  };
}
