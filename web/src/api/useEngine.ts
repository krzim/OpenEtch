// The engine connection as a React hook: fetches the document, history, machine status, project and preview, listens on the
// /api/events WebSocket, and exposes `run` for edits. The document is only ever replaced by a fresh GET; edits are
// commands, followed by a refetch (on the `document_changed` event, and after each execute response as a fallback).

import { useCallback, useEffect, useRef, useState } from "react";
import { api, ApiError, batch, eventsUrl, execute, type Invocation } from "./client.ts";
import { publishEngineEvent, type EngineMessage } from "./events.ts";
import { EMPTY_TIMELINE, normalizeHistory, type TimelineModel } from "./history.ts";
import { runFinishedToast, withApprovalLock } from "./runs.ts";
import type { CommandSpec, EngineEvent, GDocument, MachineStatus, Origin, Preview, ProjectInfo } from "./types.ts";

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
  /** Fetch /api/preview whenever the document changes. */
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

  const fail = useCallback((e: unknown) => {
    setHttpError(e instanceof Error ? e.message : String(e));
  }, []);

  const refreshPreview = useCoalesced(async () => {
    try {
      setPreview(await api.preview());
      setPreviewError(null);
    } catch (e) {
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

  const refreshDoc = useCoalesced(async () => {
    try {
      const [d, h] = await Promise.all([api.document(), api.history()]);
      setDoc(d);
      setTimeline(normalizeHistory(h));
      setHttpError(null);
    } catch (e) {
      fail(e);
      return;
    }
    // The dirty flag follows the revision.
    void refreshProject();
    if (wantPreviewRef.current) void refreshPreview();
  });

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
      try {
        if (Array.isArray(inv)) return await batch(inv);
        return (await execute(inv)).result;
      } finally {
        // Fallback for a missed WebSocket event; also covers failed commands (the doc may be unchanged but cheap).
        void refreshDoc();
      }
    },
    [refreshDoc],
  );

  const undo = useCallback(async () => {
    try {
      await api.undo();
    } finally {
      void refreshDoc();
    }
  }, [refreshDoc]);

  const redo = useCallback(async () => {
    try {
      await api.redo();
    } finally {
      void refreshDoc();
    }
  }, [refreshDoc]);

  const historyGoto = useCallback(
    async (marker: number) => {
      try {
        await api.historyGoto(marker);
      } finally {
        void refreshDoc();
      }
    },
    [refreshDoc],
  );

  const historyDelete = useCallback(
    async (index: number) => {
      try {
        await api.historyDelete(index);
      } finally {
        void refreshDoc();
      }
    },
    [refreshDoc],
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

  // Preview follows the workspace toggle.
  useEffect(() => {
    if (wantPreview) void refreshPreview();
  }, [wantPreview, refreshPreview]);

  // Event socket with reconnect and backoff.
  const docRevision = useRef<number>(-1);
  docRevision.current = doc?.revision ?? -1;
  useEffect(() => {
    let ws: WebSocket | null = null;
    let timer: number | undefined;
    let delay = 500;
    let stopped = false;
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
        // Anything may have changed while we were away.
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
            if (typeof ev.revision !== "number" || ev.revision !== docRevision.current) void refreshDoc();
            else void refreshProject();
            // An edit makes pending runs stale; fetch their flags now rather than at the next poll.
            void refreshMachine();
            if (typeof ev.revision === "number" && isRemoteOrigin(ev.origin)) {
              const { revision, origin } = ev;
              setRemoteEdit((r) => ({ revision, origin, seq: (r?.seq ?? 0) + 1 }));
            }
            break;
          case "machine":
            void refreshMachine();
            break;
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
            void refreshDoc();
            break;
          case "device_changed":
            // The profile, field size and controller changed: everything that shows them refetches.
            setDeviceSeq((n) => n + 1);
            void refreshMachine();
            void refreshDoc();
            break;
          case "run_finished": {
            void refreshMachine();
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
  }, [refreshDoc, refreshMachine, refreshProject]);

  // Poll machine status slowly as a safety net (and faster while the socket is down).
  useEffect(() => {
    const id = window.setInterval(() => void refreshMachine(), link === "open" ? 10_000 : 3_000);
    return () => window.clearInterval(id);
  }, [link, refreshMachine]);

  return {
    doc,
    timeline,
    machine,
    preview,
    previewError,
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
