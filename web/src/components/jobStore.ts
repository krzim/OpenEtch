// The Laser window's run and framing options, shared by the Laser dock, the framing popover, the preview timeline's
// Start Here and the request-run hotkey and toolbar button, and kept in this browser (localStorage, best effort) like
// LightBurn keeps them between sessions. Cut Selected Graphics is never restored: a stale "only the selection" would
// surprise the next run.

import { useSyncExternalStore } from "react";
import { DEFAULT_FRAME_OPTIONS, DEFAULT_RUN_OPTIONS, normalizeFrameOptions, normalizeRunOptions, type FrameOptions, type RunOptions } from "../api/job.ts";

const KEY = "openetch.laserOptions";

interface State {
  run: RunOptions;
  frame: FrameOptions;
}

function load(): State {
  try {
    const raw = JSON.parse(localStorage.getItem(KEY) ?? "null") as Partial<State> | null;
    if (raw && typeof raw === "object") {
      return { run: { ...normalizeRunOptions(raw.run), cutSelected: false }, frame: normalizeFrameOptions(raw.frame) };
    }
  } catch {
    // Storage unavailable or corrupt: defaults.
  }
  return { run: DEFAULT_RUN_OPTIONS, frame: DEFAULT_FRAME_OPTIONS };
}

let state: State = load();
const subs = new Set<() => void>();

function save(): void {
  try {
    localStorage.setItem(KEY, JSON.stringify(state));
  } catch {
    // Private mode: the options last for this visit.
  }
}

function emit(): void {
  for (const f of subs) f();
}

function subscribe(f: () => void): () => void {
  subs.add(f);
  return () => {
    subs.delete(f);
  };
}

export function getRunOptions(): RunOptions {
  return state.run;
}

export function getFrameOptions(): FrameOptions {
  return state.frame;
}

export function setRunOptions(p: Partial<RunOptions>): void {
  state = { ...state, run: normalizeRunOptions({ ...state.run, ...p }) };
  save();
  emit();
}

export function setFrameOptions(p: Partial<FrameOptions>): void {
  state = { ...state, frame: normalizeFrameOptions({ ...state.frame, ...p }) };
  save();
  emit();
}

export function useRunOptions(): RunOptions {
  return useSyncExternalStore(subscribe, getRunOptions, getRunOptions);
}

export function useFrameOptions(): FrameOptions {
  return useSyncExternalStore(subscribe, getFrameOptions, getFrameOptions);
}
