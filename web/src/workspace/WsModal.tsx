// A dialog (components/Dialog.tsx) whose key presses stay inside it: React's stopPropagation keeps them from App.tsx's
// window keyboard handler, so Delete or an arrow key on a dialog button never edits the document behind it. Escape
// goes through the dialog stack (components/dialogStack.ts), which closes only the top dialog.

import { Dialog, type DialogProps } from "../components/Dialog.tsx";
import "./workspace.css";

export function WsModal(props: DialogProps) {
  return (
    <div className="ws-modal-scope" onKeyDown={(e) => e.key !== "Escape" && e.stopPropagation()}>
      <Dialog {...props} />
    </div>
  );
}
