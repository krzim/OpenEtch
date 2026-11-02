// Where LightBurn's single-letter (canvas) keys may act: the canvas, nothing in particular, or a chrome button the
// mouse just pressed. A browser keeps the focus on a clicked button, so after picking a tool or a dropdown item with
// the mouse the letters (S search, R rectangle, Delete...) did nothing until the canvas was clicked; Qt tool buttons
// take no focus, so in LightBurn they keep working. A button reached with the keyboard (Tab) keeps its keys.
// (:focus-visible cannot tell them apart: Chrome turns it on for a clicked button as soon as a key is pressed.)

/** The rows of chrome whose buttons hand the letter keys back to the canvas. */
export const CHROME_ROWS = ".toolbar, .tool-rail, .topbar, .bottom-row, .palette-bar";

/** The part of an element `canvasKeysFor` looks at (an HTMLElement; a stub in tests). */
export interface FocusLike {
  tagName: string;
  closest(sel: string): unknown;
}

/**
 * Whether the canvas keys may act with `a` focused. `body`: the document's body; `pressed`: the button the last
 * pointer press landed on (null after Tab), so a button the keyboard focused keeps its keys.
 */
export function canvasKeysFor(a: FocusLike | null, body: unknown, pressed: unknown): boolean {
  if (!a || a === body) return true;
  if (a.closest(".viewport")) return true;
  return a.tagName === "BUTTON" && a === pressed && !!a.closest(CHROME_ROWS);
}

let pressed: Element | null = null;
let listening = false;

function listen() {
  if (listening || typeof document === "undefined") return;
  listening = true;
  document.addEventListener(
    "pointerdown",
    (e) => {
      pressed = e.target instanceof Element ? e.target.closest("button") : null;
    },
    true,
  );
  document.addEventListener(
    "keydown",
    (e) => {
      if (e.key === "Tab") pressed = null;
    },
    true,
  );
}
listen();

/** `canvasKeysFor` the document's focused element. */
export function canvasKeysFocused(): boolean {
  if (typeof document === "undefined") return true;
  return canvasKeysFor(document.activeElement as FocusLike | null, document.body, pressed);
}
