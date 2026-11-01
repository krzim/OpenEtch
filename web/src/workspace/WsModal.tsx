// A Modal (components/ui.tsx) whose key presses stay inside it: React's stopPropagation keeps them from App.tsx's
// window keyboard handler, so Delete or an arrow key on a dialog button never edits the document behind it. Escape
// still goes through to close the dialog.

import type { ComponentProps } from "react";
import { Modal } from "../components/ui.tsx";
import "./workspace.css";

export function WsModal(props: ComponentProps<typeof Modal>) {
  return (
    <div className="ws-modal-scope" onKeyDown={(e) => e.key !== "Escape" && e.stopPropagation()}>
      <Modal {...props} />
    </div>
  );
}
