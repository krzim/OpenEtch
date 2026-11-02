// Requests the toolbar answers wherever they come from: open command search (the S key, the toolbar's search
// button, the phone's More sheet) and open the engine's JSON command console (Help > Developer > Engine commands).
// The toolbar is always mounted, so it hosts both. No React.

export type CommandRequest = "search" | "engineConsole";

const subs = new Set<(r: CommandRequest) => void>();

export function onCommandRequest(fn: (r: CommandRequest) => void): () => void {
  subs.add(fn);
  return () => {
    subs.delete(fn);
  };
}

/** Whether anything listens (the toolbar is mounted). */
export function commandHostReady(): boolean {
  return subs.size > 0;
}

export function openCommandSearch(): boolean {
  for (const f of subs) f("search");
  return subs.size > 0;
}

export function openEngineConsole(): boolean {
  for (const f of subs) f("engineConsole");
  return subs.size > 0;
}
