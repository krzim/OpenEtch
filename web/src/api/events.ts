// A small bus for /api/events messages, so components can follow events the engine hook does not handle itself
// (`job_progress`, the `machine` events about pause, resume and the job counter) without a second WebSocket.
// useEngine.ts publishes every parsed event here.

export type EngineMessage = { type: string; [key: string]: unknown };

const listeners = new Set<(ev: EngineMessage) => void>();

/** Listen to every event; returns the unsubscribe function. */
export function onEngineEvent(fn: (ev: EngineMessage) => void): () => void {
  listeners.add(fn);
  return () => {
    listeners.delete(fn);
  };
}

/** Called by useEngine for each event received. A failing listener never stops the others. */
export function publishEngineEvent(ev: EngineMessage): void {
  for (const fn of [...listeners]) {
    try {
      fn(ev);
    } catch {
      // A listener's bug must not break the event socket.
    }
  }
}
