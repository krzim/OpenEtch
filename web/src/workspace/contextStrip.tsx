// The context strip (docs/ux/ui-plan-2026-11.md 1.1), Fusion-style: one row under the ribbon that shows the selection's
// X / Y / W / H, or the active tool's option bar, or the tool hint. A tool puts its options there with
// `<ContextStripPortal>{bar}</ContextStripPortal>`; the shell renders `<ContextStripHost/>` inside the strip and asks
// `useContextStripBusy()` whether a tool bar is showing (then it hides the numeric fields).
//
// Until a host is mounted the portal renders its children where it stands, so a bar wrapped in it looks and works as
// it did before.

import { useCallback, useEffect, useSyncExternalStore, type ReactNode } from "react";
import { createPortal } from "react-dom";

let host: HTMLElement | null = null;
let mounted = 0;
const listeners = new Set<() => void>();

function emit(): void {
  for (const l of listeners) l();
}

function subscribe(fn: () => void): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

const getHost = (): HTMLElement | null => host;
const getBusy = (): boolean => mounted > 0;

/** The slot inside the context strip that tool bars are moved into (one per window; the last mounted wins). */
export function ContextStripHost(props: { className?: string }) {
  const ref = useCallback((el: HTMLDivElement | null) => {
    if (el) host = el;
    else if (host && !host.isConnected) host = null;
    emit();
  }, []);
  return <div ref={ref} className={props.className ?? "context-strip-slot"} />;
}

/** Shows a tool's option bar in the context strip (or in place while there is no strip). */
export function ContextStripPortal({ children }: { children: ReactNode }) {
  const target = useSyncExternalStore(subscribe, getHost, () => null);
  useEffect(() => {
    mounted++;
    emit();
    return () => {
      mounted--;
      emit();
    };
  }, []);
  return target ? createPortal(children, target) : <>{children}</>;
}

/** Whether a tool bar is in the context strip now (at least one ContextStripPortal is mounted). */
export function useContextStripBusy(): boolean {
  return useSyncExternalStore(subscribe, getBusy, () => false);
}
