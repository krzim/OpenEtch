// One stack for everything Esc can close: dialogs (components/Dialog.tsx) and open menus and submenus
// (components/Menu.tsx). Esc reaches only the top entry, so two stacked dialogs close one at a time and Esc with a menu
// open closes only that menu (the selection behind it stays). docs/ux/ui-plan-2026-11.md P2 "Dialog" and "Menu".
//
// The pure stack is tested in dialogStack.test.ts; `installEscape` adds the one window listener (capture phase, so it
// runs before App's own Esc that clears the selection). Esc never stops a burn: STOP and Ctrl/Cmd+. do.

export type LayerKind = "dialog" | "menu";

export interface Layer {
  id: number;
  kind: LayerKind;
  /** Esc reached this layer (close it, or ask "Discard changes?" first). */
  onEscape: () => void;
}

export class LayerStack {
  private layers: Layer[] = [];
  private next = 1;
  private subs = new Set<() => void>();

  /** Add a layer on top; returns its id (pass it to `remove`). */
  push(kind: LayerKind, onEscape: () => void): number {
    const id = this.next++;
    this.layers.push({ id, kind, onEscape });
    this.emit();
    return id;
  }

  /** Remove a layer wherever it is (an unmount can happen out of order). */
  remove(id: number): void {
    const n = this.layers.length;
    this.layers = this.layers.filter((l) => l.id !== id);
    if (this.layers.length !== n) this.emit();
  }

  /** Replace a layer's Esc handler (it closes over fresh props on every render). */
  update(id: number, onEscape: () => void): void {
    const l = this.layers.find((x) => x.id === id);
    if (l) l.onEscape = onEscape;
  }

  top(): Layer | undefined {
    return this.layers[this.layers.length - 1];
  }

  isTop(id: number): boolean {
    return this.top()?.id === id;
  }

  /** The topmost dialog (Enter submits only that one). */
  topDialog(): Layer | undefined {
    for (let i = this.layers.length - 1; i >= 0; i--) if (this.layers[i].kind === "dialog") return this.layers[i];
    return undefined;
  }

  /** How many dialogs are open. */
  dialogCount(): number {
    return this.layers.filter((l) => l.kind === "dialog").length;
  }

  get size(): number {
    return this.layers.length;
  }

  /** Esc: the top layer handles it. Returns whether there was one. */
  escape(): boolean {
    const t = this.top();
    if (!t) return false;
    t.onEscape();
    return true;
  }

  subscribe(fn: () => void): () => void {
    this.subs.add(fn);
    return () => {
      this.subs.delete(fn);
    };
  }

  private emit(): void {
    for (const f of this.subs) f();
  }
}

/** The app's one stack. */
export const layers = new LayerStack();

let installed = false;

/**
 * Install the window Esc listener once (on the first dialog or menu). It is a capture listener added after the
 * canvas's own (which cancel a drag or a pen path first, and stop the event when they do), so it runs before App's
 * bubbling Esc handler and keeps that from also clearing the selection.
 */
export function installEscape(): void {
  if (installed || typeof window === "undefined") return;
  installed = true;
  window.addEventListener(
    "keydown",
    (e) => {
      if (e.key !== "Escape" || e.defaultPrevented || e.isComposing) return;
      if (layers.escape()) {
        e.preventDefault();
        e.stopImmediatePropagation();
      }
    },
    true,
  );
}
