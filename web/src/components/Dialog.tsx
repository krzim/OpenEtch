// STAGE A STUB (owner: P2). The one dialog primitive of docs/ux/ui-plan-2026-11.md P2: a stack (Esc and Enter reach
// only the top dialog), focus trap and restore, sizes, a full-screen sheet below 600 px, and `--z-modal` below STOP.
// The signatures below are final; ui.tsx `Modal` becomes an alias of `Dialog` when P2 lands. Until then `Dialog`
// renders today's Modal (`size` lg / xl / full map to its wide variant; `dismissable` and `dirty` are ignored).

import type { ReactNode } from "react";
import { Modal } from "./ui.tsx";

/** Today's ui.tsx Modal props. */
export interface ModalProps {
  title: string;
  onClose: () => void;
  children: ReactNode;
  footer?: ReactNode;
  wide?: boolean;
  onSubmit?: () => void;
}

/** sm 360 / md 480 / lg 720 / xl 960 px wide, or full screen. */
export type DialogSize = "sm" | "md" | "lg" | "xl" | "full";

export interface DialogProps extends ModalProps {
  size?: DialogSize;
  /** A press on the backdrop closes it (default: true for dialogs without a form, false for form dialogs). */
  dismissable?: boolean;
  /** Unsaved edits: closing asks "Discard changes?" first. */
  dirty?: boolean;
}

export function Dialog(p: DialogProps) {
  const wide = p.wide || p.size === "lg" || p.size === "xl" || p.size === "full";
  return (
    <Modal title={p.title} onClose={p.onClose} footer={p.footer} wide={wide} onSubmit={p.onSubmit}>
      {p.children}
    </Modal>
  );
}
